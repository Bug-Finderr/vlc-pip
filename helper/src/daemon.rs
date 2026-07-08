use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU8, AtomicU32, Ordering::Relaxed};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetDoubleClickTime, RegisterHotKey, UnregisterHotKey, MOD_ALT,
    MOD_CONTROL, MOD_NOREPEAT, VK_CONTROL, VK_ESCAPE, VK_F, VK_LWIN, VK_MENU, VK_P, VK_RWIN,
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
// the cached PiP is fullscreen-origin (saved style has no caption): the keyboard hook
// swallows Esc and the tick keeps VLC's fullscreen controller strip hidden
static FS_PIP: AtomicBool = AtomicBool::new(false);
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
    let s = state::load(&state::state_path()).filter(native::owns_state);
    FS_PIP.store(s.as_ref().is_some_and(|s| native::fs_origin(s.style)), Relaxed);
    CACHED_HWND.store(s.map_or(0, |s| s.hwnd as isize), Relaxed);
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
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                native::toggle(&options::effective(argv));
                refresh_state();
            } else if msg.message == WM_TIMER {
                if last_beat.elapsed() > Duration::from_secs(3) {
                    beat(&mut last_beat);
                }
                poll_request(argv);
                refresh_state(); // the hook cache must reflect a request-triggered toggle within this tick
                if FS_PIP.load(Relaxed) {
                    // VLC still believes it is fullscreen under this PiP: keep its
                    // controller strip off the screen (SPEC section 7)
                    native::hide_fs_controller(native::window_owner(CACHED_HWND.load(Relaxed)));
                }
                if DRAG_STATE.load(Relaxed) >= DRAG_MOVING {
                    tracker = native::RegionTracker::default(); // gestures own the window while dragging
                } else {
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

fn poll_request(argv: &[String]) {
    match request::consume(&request::request_path()).as_deref() {
        Some("toggle") => {
            native::toggle(&options::effective(argv));
        }
        Some("enter") => {
            native::enter(native::find_player(), &options::effective(argv));
        }
        Some("exit") => {
            native::exit_pip();
        }
        Some("stop") => unsafe { PostQuitMessage(0) },
        _ => {}
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && (wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN) {
            let k = &*(lparam as *const KBDLLHOOKSTRUCT);
            let h = CACHED_HWND.load(Relaxed);
            if h != 0 && GetForegroundWindow() as isize == h {
                if k.vkCode == VK_F as u32 {
                    return 1; // swallow F -> no fullscreen while in PiP
                }
                // a fullscreen-origin PiP rides on VLC's live internal fullscreen
                // state: Esc would make Qt leave it UNDERNEATH the reshape, desyncing
                // Qt's window cache (SPEC section 7). BARE Esc only - Alt+Esc and
                // Ctrl+Esc are OS shortcuts, and VLC binds leave-fullscreen to plain
                // Esc alone
                if k.vkCode == VK_ESCAPE as u32
                    && FS_PIP.load(Relaxed)
                    && [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
                        .into_iter()
                        .all(|vk| GetAsyncKeyState(vk as i32) as u16 & 0x8000 == 0)
                {
                    return 1;
                }
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
