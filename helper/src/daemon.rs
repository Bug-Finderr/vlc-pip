use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU8, AtomicU32, Ordering::Relaxed};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    VK_F, VK_P,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetAncestor, GetForegroundWindow, GetMessageW,
    GetSystemMetrics, PostQuitMessage, PostThreadMessageW, SetTimer, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, KBDLLHOOKSTRUCT, MSG,
    MSLLHOOKSTRUCT, SM_CXDOUBLECLK, SM_CXDRAG, SM_CYDOUBLECLK, SM_CYDRAG, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_SYSKEYDOWN, WM_TIMER,
};

use crate::{geometry, native, options, request, state};

// Read replica of the state file for the LL hooks: disk I/O inside a hook callback risks
// the LowLevelHooksTimeout, after which Windows SILENTLY removes the hook and the
// fullscreen-block guarantee dies with no error. Refreshed only on the pump thread; hook
// callbacks dispatch on that same thread. 0 = not in PiP. Read-only by design: stale-file
// DELETION stays in native (toggle paths + maintain_region tick).
static CACHED_HWND: AtomicIsize = AtomicIsize::new(0);
static KB_HOOK: AtomicIsize = AtomicIsize::new(0);
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);
// click rate-limit bookkeeping: the last ALLOWED button-down (time in u32 hook ms, wraps)
static LAST_ALLOWED_TIME: AtomicU32 = AtomicU32::new(0);
static LAST_ALLOWED_X: AtomicI32 = AtomicI32::new(0);
static LAST_ALLOWED_Y: AtomicI32 = AtomicI32::new(0);
static SWALLOW_NEXT_UP: AtomicBool = AtomicBool::new(false);
// true while this process owns the heartbeat file; the panic hook checks it so a daemon
// crash deletes the heartbeat (v1's finally did) and pip.lua respawns immediately instead
// of treating the dead daemon as alive for up to 15s
static OWNS_ALIVE_FILE: AtomicBool = AtomicBool::new(false);

// Drag gesture state. Hook-owned; the pump reads mode from wParam and validates the
// generation from lParam, so a hook-side reset or rapid re-arm can never make a queued
// message mix stale deltas with fresh statics.
const DRAG_IDLE: u8 = 0;
const DRAG_ARMED: u8 = 1;
const DRAG_MOVING: u8 = 2;
const DRAG_RESIZING: u8 = 3;
const WM_APP_DRAG: u32 = WM_APP;
const WM_APP_DRAGEND: u32 = WM_APP + 1;
static DRAG_STATE: AtomicU8 = AtomicU8::new(DRAG_IDLE);
static DRAG_ZONE: AtomicU8 = AtomicU8::new(0);
static DRAG_GEN: AtomicU32 = AtomicU32::new(0);
static DRAG_HWND: AtomicIsize = AtomicIsize::new(0);
static DRAG_ORIGIN_X: AtomicI32 = AtomicI32::new(0);
static DRAG_ORIGIN_Y: AtomicI32 = AtomicI32::new(0);
static DRAG_START_L: AtomicI32 = AtomicI32::new(0);
static DRAG_START_T: AtomicI32 = AtomicI32::new(0);
static DRAG_START_R: AtomicI32 = AtomicI32::new(0);
static DRAG_START_B: AtomicI32 = AtomicI32::new(0);
static DRAG_VIS_L: AtomicI32 = AtomicI32::new(0);
static DRAG_VIS_T: AtomicI32 = AtomicI32::new(0);
static DRAG_VIS_W: AtomicI32 = AtomicI32::new(0);
static DRAG_VIS_H: AtomicI32 = AtomicI32::new(0);
static DRAG_HAD_RGN: AtomicBool = AtomicBool::new(false);
static LATEST_X: AtomicI32 = AtomicI32::new(0);
static LATEST_Y: AtomicI32 = AtomicI32::new(0);
static MOVE_PENDING: AtomicBool = AtomicBool::new(false);

pub fn owns_alive_file() -> bool {
    OWNS_ALIVE_FILE.load(Relaxed)
}

fn refresh_state() {
    // full owner-PID guard (not just IsWindow): pending heal records keep stale states
    // alive indefinitely, so a recycled HWND must never re-arm the guards - or drags -
    // on a foreign window
    let h = state::load(&state::state_path())
        .filter(native::owns_state)
        .map_or(0, |s| s.hwnd as isize);
    CACHED_HWND.store(h, Relaxed);
}

pub fn run(argv: &[String]) -> i32 {
    unsafe {
        // single instance; second instance exits 0 before touching any file
        let name: Vec<u16> = "VlcPipDaemon\0".encode_utf16().collect();
        let mutex = CreateMutexW(std::ptr::null(), 1, name.as_ptr()); // held for process lifetime
        if mutex.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
            return 0; // already running, or the name is unobtainable: never double-run
        }

        // discard a stale pre-launch "stop" ('pip-helper stop' with no daemon alive leaves
        // one that would kill us on the first tick); only "stop", so a queued toggle survives
        let rp = request::request_path();
        if let Ok(c) = std::fs::read_to_string(&rp)
            && c.trim() == "stop"
        {
            let _ = std::fs::remove_file(&rp);
        }

        let hot = RegisterHotKey(std::ptr::null_mut(), 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_P as u32) != 0;
        let timer = SetTimer(std::ptr::null_mut(), 0, 150, None) != 0; // WM_TIMER -> thread queue
        let module = GetModuleHandleW(std::ptr::null());
        KB_HOOK.store(SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0) as isize, Relaxed);
        MOUSE_HOOK.store(SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0) as isize, Relaxed);

        // Heartbeat, not a marker: a force-killed daemon can't delete the file, so consumers
        // (pip.lua) check the leading epoch-seconds for freshness. Also carries arming
        // diagnostics. Write failures are swallowed: NEVER let the heartbeat kill the pump.
        let alive = state::temp_path("vlc-pip-daemon.alive");
        let beat = |last: &mut Instant| {
            *last = Instant::now();
            let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let _ = std::fs::write(&alive, format!(
                "{epoch} pid={} hotkey={} timer={} kb={} mouse={}",
                std::process::id(),
                hot as i32,
                timer as i32,
                (KB_HOOK.load(Relaxed) != 0) as i32,
                (MOUSE_HOOK.load(Relaxed) != 0) as i32,
            ));
        };
        OWNS_ALIVE_FILE.store(true, Relaxed);
        let mut last_beat = Instant::now();
        beat(&mut last_beat);
        refresh_state(); // a daemon restarted while already in PiP must be guarded from the first message

        let mut tracker = native::RegionTracker::default();
        let mut pending: Option<PendingEnter> = None;
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                toggle_deferred(argv, &mut pending);
                refresh_state();
            } else if msg.message == WM_TIMER {
                if last_beat.elapsed() > Duration::from_secs(3) {
                    beat(&mut last_beat);
                }
                poll_request(argv, &mut pending);
                tick_pending(argv, &mut pending);
                refresh_state(); // the hook cache must reflect a request- or handoff-triggered toggle within this tick
                if DRAG_STATE.load(Relaxed) >= DRAG_MOVING {
                    tracker = native::RegionTracker::default(); // gestures own the window while dragging
                } else if pending.is_none() {
                    // paused during a handoff: its heal path would SetWindowPos the
                    // still-fullscreen window, leaving a state that is neither
                    // fullscreen nor windowed - the Esc wait then dead-stalls cloaked
                    native::maintain_region(&mut tracker);
                }
            } else if msg.message == WM_APP_DRAG || msg.message == WM_APP_DRAGEND {
                MOVE_PENDING.store(false, Relaxed);
                let h = DRAG_HWND.load(Relaxed);
                // generation guard: a rapid release-and-repress re-arms the statics; a message
                // from the previous drag must not apply its stale delta to the fresh state
                if h != 0 && h == CACHED_HWND.load(Relaxed) && msg.lParam == DRAG_GEN.load(Relaxed) as isize {
                    let start = geometry::Rect {
                        left: DRAG_START_L.load(Relaxed),
                        top: DRAG_START_T.load(Relaxed),
                        right: DRAG_START_R.load(Relaxed),
                        bottom: DRAG_START_B.load(Relaxed),
                    };
                    let dx = LATEST_X.load(Relaxed) - DRAG_ORIGIN_X.load(Relaxed);
                    let dy = LATEST_Y.load(Relaxed) - DRAG_ORIGIN_Y.load(Relaxed);
                    let resizing = msg.wParam == DRAG_RESIZING as usize;
                    let target = if resizing {
                        let zone = geometry::DragZone::from_u8(DRAG_ZONE.load(Relaxed));
                        geometry::plan_resize(&start, zone, dx, dy, &native::work_area(h))
                    } else {
                        geometry::Rect {
                            left: start.left + dx,
                            top: start.top + dy,
                            right: start.right + dx,
                            bottom: start.bottom + dy,
                        }
                    };
                    if resizing {
                        // live minimal look: clip to where the video will sit, using the
                        // per-side chrome measured at drag start; convergence verifies the
                        // exact box after release
                        let clip = if DRAG_HAD_RGN.load(Relaxed) {
                            let (vl, vt) = (DRAG_VIS_L.load(Relaxed), DRAG_VIS_T.load(Relaxed));
                            let c = geometry::Rect {
                                left: vl - start.left,
                                top: vt - start.top,
                                right: (target.right - target.left) - (start.right - (vl + DRAG_VIS_W.load(Relaxed))),
                                bottom: (target.bottom - target.top) - (start.bottom - (vt + DRAG_VIS_H.load(Relaxed))),
                            };
                            (c.right > c.left && c.bottom > c.top).then_some(c)
                        } else {
                            None
                        };
                        native::drag_resize(h, &target, clip.as_ref());
                    } else {
                        native::drag_move(h, &target);
                    }
                    if msg.message == WM_APP_DRAGEND {
                        // finalize from OUR computed rect: the async SetWindowPos above has
                        // not landed in VLC yet, so a fresh GetWindowRect would be stale
                        let chrome_w = (start.right - start.left) - DRAG_VIS_W.load(Relaxed);
                        let chrome_h = (start.bottom - start.top) - DRAG_VIS_H.load(Relaxed);
                        native::finish_drag(&target, resizing, chrome_w, chrome_h);
                        tracker = native::RegionTracker::default(); // convergence re-clips from a clean debounce
                    }
                }
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let kb = KB_HOOK.load(Relaxed);
        if kb != 0 {
            UnhookWindowsHookEx(kb as _);
        }
        let mouse = MOUSE_HOOK.load(Relaxed);
        if mouse != 0 {
            UnhookWindowsHookEx(mouse as _);
        }
        UnregisterHotKey(std::ptr::null_mut(), 1);
        let _ = std::fs::remove_file(&alive);
        OWNS_ALIVE_FILE.store(false, Relaxed);
    }
    0
}

// Fullscreen handoff, deferred: leaving fullscreen is async in VLC and the pump must
// never block (LL-hook timeout), so the enter waits on timer ticks until the caption
// holds for TWO consecutive ticks (Qt's style AND rect restores both landed). Keyed by
// hwnd + owner PID. Rationale and contract: native.rs handoff section, SPEC §7.
struct PendingEnter {
    hwnd: isize,
    pid: u32,
    esc_tries: u8,
    quiet_ticks: u8,
    windowed_ticks: u8,
    deadline: Instant,
    // absolute, never refreshed: modifiers_held() reads GLOBAL keyboard state, so a
    // stuck modifier (RDP/VM artifact) or typing in another app after an alt-tab must
    // not hold the cloaked (invisible) VLC hostage via the refreshable deadline
    hard_deadline: Instant,
}

fn enter_deferring_fullscreen(argv: &[String], pending: &mut Option<PendingEnter>) {
    let h = native::find_player();
    if h != 0 {
        let was_iconic = native::restore_if_iconic(h);
        if native::is_fullscreen(h) || (was_iconic && !native::is_windowed(h)) {
            *pending = Some(PendingEnter {
                hwnd: h,
                pid: native::window_owner(h),
                esc_tries: 0,
                quiet_ticks: 0,
                windowed_ticks: 0,
                deadline: Instant::now() + Duration::from_secs(2),
                hard_deadline: Instant::now() + Duration::from_secs(6),
            });
            // blank the transition from the very keypress: the intermediate windowed
            // restore must never render (SPEC §7 cloak). No immediate tick here - a
            // request-file arm would tick twice in one timer pass and slip the Esc
            // through the quiet gate; the next tick is <=150ms away and cloaked.
            native::cloak(h);
            return;
        }
    }
    native::enter(h, &options::effective(argv));
}

fn toggle_deferred(argv: &[String], pending: &mut Option<PendingEnter>) {
    if native::in_pip() {
        // even with a pending armed (a one-shot enter can win the race): toggle means exit
        if let Some(p) = pending.take() {
            native::uncloak_owned(p.hwnd, p.pid);
        }
        native::exit_pip();
    } else if pending.is_none() {
        enter_deferring_fullscreen(argv, pending);
    }
    // pending armed and not in PiP: the enter is already in flight. The handoff is
    // invisible for its first ~0.5-1s, so an impatient repeat press MUST be a no-op -
    // cancelling made spam ping-pong between arm and cancel, and an even number of
    // presses never entered PiP (the 2s deadline is the way out of a stuck pending).
}

fn tick_pending(argv: &[String], pending: &mut Option<PendingEnter>) {
    let Some(p) = pending.as_mut() else { return };
    let done = if native::window_owner(p.hwnd) != p.pid || native::in_pip() {
        // VLC died / handle recycled, or something else entered PiP first (a one-shot's
        // PiP on this very window must become visible again)
        native::uncloak_owned(p.hwnd, p.pid);
        true
    } else if native::is_windowed(p.hwnd) {
        if native::modifiers_held() && Instant::now() < p.hard_deadline {
            // keyboard busy: let the spam settle. A PiP materializing mid-spam gets
            // toggled right back out by the next press (observed live) - nothing may
            // appear until the user's hands are off the modifiers. Past the hard cap,
            // stop waiting for quiet and finish the enter.
            p.windowed_ticks = 0;
            false
        } else {
            p.windowed_ticks += 1;
            if p.windowed_ticks >= 2 {
                // enter() uncloaks right before its reshape; its guards bail before that
                if !native::enter(p.hwnd, &options::effective(argv)) {
                    native::uncloak_owned(p.hwnd, p.pid);
                }
                true
            } else {
                false
            }
        }
    } else {
        if native::modifiers_held() {
            p.quiet_ticks = 0;
            if p.esc_tries == 0 {
                // wait out the chord; the give-up countdown starts at the release
                p.deadline = Instant::now() + Duration::from_secs(2);
            }
        } else if p.esc_tries < 3 && native::is_fullscreen(p.hwnd) {
            // Esc only after TWO quiet ticks: spam's inter-press gaps are wide enough
            // for a single tick to slip the Esc out mid-burst. Re-posts capped (a
            // single post can fizzle; a modal dialog must not be Esc-spammed).
            p.quiet_ticks += 1;
            if p.quiet_ticks >= 2 {
                native::request_unfullscreen(p.hwnd);
                p.esc_tries += 1;
                p.quiet_ticks = 0;
            }
        }
        p.windowed_ticks = 0; // caption flickered: restart the stability count
        // Esc didn't take (e.g. a modal ate it), or the hard cap hit: give up
        let expired = Instant::now() >= p.deadline || Instant::now() >= p.hard_deadline;
        if expired {
            native::uncloak_owned(p.hwnd, p.pid); // giving up: put the fullscreen window back on screen
        }
        expired
    };
    if done {
        *pending = None;
    }
}

fn poll_request(argv: &[String], pending: &mut Option<PendingEnter>) {
    match request::consume(&request::request_path()).as_deref() {
        Some("toggle") => toggle_deferred(argv, pending),
        Some("enter") => {
            if pending.is_none() && !native::in_pip() {
                enter_deferring_fullscreen(argv, pending);
            }
        }
        Some("exit") => {
            // an armed enter is moot once an exit is requested
            if let Some(p) = pending.take() {
                native::uncloak_owned(p.hwnd, p.pid);
            }
            native::exit_pip();
        }
        Some("stop") => {
            // clear FIRST: PostQuitMessage is not preemptive and this tick still runs
            // tick_pending - a handoff completing on the way out strands a PiP'd VLC
            // (and an un-uncloaked one would strand an INVISIBLE VLC with no daemon)
            if let Some(p) = pending.take() {
                native::uncloak_owned(p.hwnd, p.pid);
            }
            unsafe { PostQuitMessage(0) }
        }
        _ => {}
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && (wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN) {
            let k = &*(lparam as *const KBDLLHOOKSTRUCT);
            let h = CACHED_HWND.load(Relaxed);
            if k.vkCode == VK_F as u32 && h != 0 && GetForegroundWindow() as isize == h {
                return 1; // swallow F -> no fullscreen while in PiP
            }
        }
        CallNextHookEx(KB_HOOK.load(Relaxed) as _, code, wparam, lparam)
    }
}

// Rate-limit clicks over the PiP window: swallow every button-down within double-click
// time+rect of the last ALLOWED button-down, so no two clicks the OS actually receives
// can ever pair into a synthesized WM_LBUTTONDBLCLK. (v1 bug: swallowing only the 2nd
// click let a TRIPLE click through - the OS paired clicks 1+3 and VLC fullscreened.)
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            if wparam as u32 == WM_LBUTTONDOWN {
                let m = &*(lparam as *const MSLLHOOKSTRUCT);
                let h = CACHED_HWND.load(Relaxed);
                if h != 0 && GetAncestor(WindowFromPoint(m.pt), GA_ROOT) as isize == h {
                    let burst = m.time.wrapping_sub(LAST_ALLOWED_TIME.load(Relaxed)) <= GetDoubleClickTime()
                        && (m.pt.x - LAST_ALLOWED_X.load(Relaxed)).abs() <= GetSystemMetrics(SM_CXDOUBLECLK)
                        && (m.pt.y - LAST_ALLOWED_Y.load(Relaxed)).abs() <= GetSystemMetrics(SM_CYDOUBLECLK);
                    if burst {
                        SWALLOW_NEXT_UP.store(true, Relaxed);
                        return 1;
                    }
                    LAST_ALLOWED_TIME.store(m.time, Relaxed);
                    LAST_ALLOWED_X.store(m.pt.x, Relaxed);
                    LAST_ALLOWED_Y.store(m.pt.y, Relaxed);
                    // arm a potential drag; the zone (interior vs 16px band) picks move vs resize
                    DRAG_GEN.fetch_add(1, Relaxed); // invalidates any queued message from a prior drag
                    if let (Some(vis), Some(wr)) = (native::visible_rect(h), native::window_rect(h)) {
                        DRAG_ZONE.store(geometry::classify_zone(m.pt.x, m.pt.y, &vis, native::drag_band(h)) as u8, Relaxed);
                        DRAG_VIS_L.store(vis.left, Relaxed);
                        DRAG_VIS_T.store(vis.top, Relaxed);
                        DRAG_VIS_W.store(vis.right - vis.left, Relaxed);
                        DRAG_VIS_H.store(vis.bottom - vis.top, Relaxed);
                        DRAG_HAD_RGN.store(vis != wr, Relaxed); // visible == window means no region to preserve
                        DRAG_START_L.store(wr.left, Relaxed);
                        DRAG_START_T.store(wr.top, Relaxed);
                        DRAG_START_R.store(wr.right, Relaxed);
                        DRAG_START_B.store(wr.bottom, Relaxed);
                        DRAG_ORIGIN_X.store(m.pt.x, Relaxed);
                        DRAG_ORIGIN_Y.store(m.pt.y, Relaxed);
                        DRAG_HWND.store(h, Relaxed);
                        DRAG_STATE.store(DRAG_ARMED, Relaxed);
                    } // either probe failed: window vanished under the click - stay idle
                }
            } else if wparam as u32 == WM_MOUSEMOVE {
                // hot path for ALL system mouse movement: one atomic load when idle
                let mut st = DRAG_STATE.load(Relaxed);
                if st != DRAG_IDLE {
                    let m = &*(lparam as *const MSLLHOOKSTRUCT);
                    if st == DRAG_ARMED
                        && ((m.pt.x - DRAG_ORIGIN_X.load(Relaxed)).abs() > GetSystemMetrics(SM_CXDRAG)
                            || (m.pt.y - DRAG_ORIGIN_Y.load(Relaxed)).abs() > GetSystemMetrics(SM_CYDRAG))
                    {
                        st = if DRAG_ZONE.load(Relaxed) == geometry::DragZone::Interior as u8 {
                            DRAG_MOVING
                        } else {
                            DRAG_RESIZING
                        };
                        DRAG_STATE.store(st, Relaxed);
                    }
                    if st >= DRAG_MOVING {
                        LATEST_X.store(m.pt.x, Relaxed);
                        LATEST_Y.store(m.pt.y, Relaxed);
                        if !MOVE_PENDING.swap(true, Relaxed) {
                            PostThreadMessageW(GetCurrentThreadId(), WM_APP_DRAG, st as usize, DRAG_GEN.load(Relaxed) as isize);
                        }
                    }
                }
            } else if wparam as u32 == WM_LBUTTONUP {
                let st = DRAG_STATE.swap(DRAG_IDLE, Relaxed);
                if st >= DRAG_MOVING {
                    PostThreadMessageW(GetCurrentThreadId(), WM_APP_DRAGEND, st as usize, DRAG_GEN.load(Relaxed) as isize);
                }
                if SWALLOW_NEXT_UP.load(Relaxed) {
                    SWALLOW_NEXT_UP.store(false, Relaxed);
                    return 1; // keep the input stream paired: drop the up of a dropped down
                }
            }
        }
        CallNextHookEx(MOUSE_HOOK.load(Relaxed) as _, code, wparam, lparam)
    }
}
