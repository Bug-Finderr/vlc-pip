use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetDoubleClickTime, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey,
    UnregisterHotKey, VK_CONTROL, VK_ESCAPE, VK_F, VK_LWIN, VK_MENU, VK_P, VK_RWIN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GA_ROOT, GetAncestor, GetForegroundWindow, GetMessageW, GetSystemMetrics,
    HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, PostQuitMessage, PostThreadMessageW,
    SM_CXDOUBLECLK, SM_CXDRAG, SM_CYDOUBLECLK, SM_CYDRAG, SetTimer, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_SYSKEYDOWN, WM_TIMER, WindowFromPoint,
};

use crate::state::PipState;
use crate::{geometry, native, options, state};

// Everything the hooks touch lives on the pump thread - LL hook callbacks dispatch on
// the thread that installed them (SPEC R7) - so plain Cells hold all hook state, and
// reads and writes move WHOLE structs: a queued message can never mix stale fields with
// fresh ones. Hooks still never touch the disk (LowLevelHooksTimeout would silently
// remove them); the cells are refreshed by the pump, and stale-file DELETION stays in
// native. The one atomic is for the panic hook, which can fire on any thread:
// true while this process owns the heartbeat file, so a daemon crash deletes the
// heartbeat and pip.lua respawns immediately instead of treating the dead daemon as
// alive for up to 15s.
static OWNS_ALIVE_FILE: AtomicBool = AtomicBool::new(false);

const WM_APP_DRAG: u32 = WM_APP;
const WM_APP_DRAGEND: u32 = WM_APP + 1;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum DragState {
    #[default]
    Idle,
    Armed,
    Active,
}

// The pump validates the generation from lParam, so a hook-side reset or rapid re-arm
// can never apply a stale delta.
#[derive(Clone, Copy, Default)]
struct Drag {
    state: DragState,
    zone: geometry::DragZone,
    generation: u32,
    hwnd: isize,
    origin: (i32, i32),
    start: geometry::Rect,
    vis: geometry::Rect,
    latest: (i32, i32),
    move_pending: bool,
}

impl Drag {
    fn reset(self) -> Self {
        Self {
            generation: self.generation,
            ..Self::default()
        }
    }
}

/// The last ALLOWED button-down (time in u32 hook ms, wraps) for the click rate limit.
#[derive(Clone, Copy, Default)]
struct Click {
    time: u32,
    x: i32,
    y: i32,
    swallow_next_up: bool,
}

/// The current PiP (hwnd 0 = none). `fs` = fullscreen-origin (saved style has no
/// caption): the keyboard hook swallows Esc and the tick keeps VLC's strip hidden.
/// `pid` is the owner verified by owns_state - the tick reuses it rather than
/// re-deriving it from a handle that may have been recycled since.
#[derive(Clone, Copy, Default)]
struct Pip {
    hwnd: isize,
    fs: bool,
    pid: u32,
}

thread_local! {
    static DRAG: Cell<Drag> = Cell::new(Drag::default());
    static CLICK: Cell<Click> = Cell::new(Click::default());
    static PIP: Cell<Pip> = Cell::new(Pip::default());
}

pub fn owns_alive_file() -> bool {
    OWNS_ALIVE_FILE.load(Relaxed)
}

fn sync_session(hooks: &mut (HHOOK, HHOOK), s: Option<PipState>) {
    // full owner-PID guard (not just IsWindow): pending heal records keep stale states
    // alive indefinitely, so a recycled HWND must never re-arm the guards - or drags -
    // on a foreign window
    let previous = PIP.get();
    let pip = s
        .filter(native::owns_state)
        .map_or(Pip::default(), |s| Pip {
            hwnd: s.hwnd,
            fs: native::fs_origin(s.style),
            pid: s.pid,
        });
    PIP.set(pip);

    if previous.hwnd != 0 && (previous.hwnd != pip.hwnd || previous.pid != pip.pid) {
        DRAG.set(DRAG.get().reset());
        CLICK.set(Click {
            swallow_next_up: false,
            ..CLICK.get()
        });
    }

    if pip.hwnd != 0 {
        unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            if hooks.0.is_null() {
                hooks.0 = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
            }
            if hooks.1.is_null() {
                hooks.1 = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
            }
        }
    } else {
        if !hooks.0.is_null() && unsafe { UnhookWindowsHookEx(hooks.0) } != 0 {
            hooks.0 = std::ptr::null_mut();
        }
        if !hooks.1.is_null() && unsafe { UnhookWindowsHookEx(hooks.1) } != 0 {
            hooks.1 = std::ptr::null_mut();
        }
    }
}

pub(crate) fn heartbeat_line(
    epoch: u64,
    pid: u32,
    hotkey: bool,
    timer: bool,
    kb: bool,
    mouse: bool,
) -> String {
    format!(
        "{epoch} pid={pid} hotkey={} timer={} kb={} mouse={}",
        hotkey as u8, timer as u8, kb as u8, mouse as u8
    )
}

pub fn run(argv: &[String]) -> i32 {
    // single instance; second instance exits 0 before touching any file
    let name: Vec<u16> = "VlcPipDaemon\0".encode_utf16().collect();
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) }; // held for process lifetime
    if mutex.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return 0; // already running, or the name is unobtainable: never double-run
    }

    // discard a stale pre-launch "stop" ('pip-helper stop' with no daemon alive leaves
    // one that would kill us on the first tick); only "stop", so a queued toggle survives
    let rp = state::request_path();
    if let Ok(c) = std::fs::read_to_string(&rp)
        && c.trim() == "stop"
    {
        let _ = std::fs::remove_file(&rp);
    }

    let (hot, timer) = unsafe {
        (
            RegisterHotKey(
                std::ptr::null_mut(),
                1,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                VK_P as u32,
            ) != 0,
            SetTimer(std::ptr::null_mut(), 0, 150, None) != 0, // WM_TIMER -> thread queue
        )
    };
    let mut hooks: (HHOOK, HHOOK) = (std::ptr::null_mut(), std::ptr::null_mut());

    // Heartbeat, not a marker: a force-killed daemon can't delete the file, so consumers
    // (pip.lua) check the leading epoch-seconds for freshness. kb/mouse report the
    // current session hook slots. Write failures never kill the pump.
    let alive = state::alive_path();
    let beat = |last: &mut Instant, hooks: &(HHOOK, HHOOK)| {
        *last = Instant::now();
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let _ = std::fs::write(
            &alive,
            heartbeat_line(
                epoch,
                std::process::id(),
                hot,
                timer,
                !hooks.0.is_null(),
                !hooks.1.is_null(),
            ),
        );
    };
    OWNS_ALIVE_FILE.store(true, Relaxed);
    let mut last_beat = Instant::now();
    sync_session(&mut hooks, state::load(&state::state_path()));
    beat(&mut last_beat, &hooks);

    let mut tracker = native::RegionTracker::default();
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        if msg.message == WM_HOTKEY {
            native::toggle(&options::effective(argv));
            sync_session(&mut hooks, state::load(&state::state_path()));
        } else if msg.message == WM_TIMER {
            poll_request(argv);
            // The normal tick shares one post-request snapshot between session sync and
            // maintenance. A terminal maintenance change alone triggers a second load.
            let s = state::load(&state::state_path());
            sync_session(&mut hooks, s);
            let pip = PIP.get();
            if pip.fs {
                // VLC still believes it is fullscreen under this PiP: keep its
                // controller strip off the screen (SPEC section 7)
                native::veil_fs_controller(pip.pid);
            }
            let state_dropped = if DRAG.get().state == DragState::Active {
                tracker.reset_debounce(); // gestures own the window while dragging
                false
            } else {
                native::maintain_region(&mut tracker, s)
            };
            if state_dropped {
                sync_session(&mut hooks, state::load(&state::state_path()));
            }
            if last_beat.elapsed() > Duration::from_secs(3) {
                beat(&mut last_beat, &hooks);
            }
        } else if msg.message == WM_APP_DRAG || msg.message == WM_APP_DRAGEND {
            on_drag_msg(&msg, &mut tracker);
        }
    }

    sync_session(&mut hooks, None);
    unsafe { UnregisterHotKey(std::ptr::null_mut(), 1) };
    let _ = std::fs::remove_file(&alive);
    OWNS_ALIVE_FILE.store(false, Relaxed);
    0
}

// The coalesced drag apply. Snapshot the gesture BEFORE any Win32 call: SetWindowPos/
// SetWindowRgn can pump sent messages, and a hook re-arm mid-call must not be clobbered
// or half-read. Generation drops an old gesture's queued message; the fresh owner-PID
// check prevents a recycled HWND from targeting a foreign window.
fn on_drag_msg(msg: &MSG, tracker: &mut native::RegionTracker) {
    let mut d = DRAG.get();
    d.move_pending = false;
    DRAG.set(d);
    let pip = PIP.get();
    if d.hwnd != pip.hwnd
        || msg.lParam != d.generation as isize
        || !owner_is_current(pip.pid, native::window_owner(d.hwnd))
    {
        return;
    }
    let (dx, dy) = (
        pointer_delta(d.latest.0, d.origin.0),
        pointer_delta(d.latest.1, d.origin.1),
    );
    let resizing = d.zone != (0, 0);
    let target = if resizing {
        Some(geometry::plan_resize(
            &d.start,
            d.zone,
            dx,
            dy,
            &native::work_area(d.hwnd),
        ))
    } else {
        geometry::plan_move(&d.start, dx, dy)
    };
    let Some(target) = target else {
        if msg.message == WM_APP_DRAGEND {
            tracker.reset_debounce();
        }
        return;
    };
    if resizing {
        // live minimal look: clip to where the video will sit, using the per-side chrome
        // measured at drag start; convergence verifies the exact box after release
        let clip = (d.vis != d.start)
            .then(|| geometry::resize_clip(&d.start, &d.vis, &target))
            .flatten();
        native::drag_resize(d.hwnd, &target, clip.as_ref());
    } else {
        native::drag_move(d.hwnd, &target);
    }
    if msg.message == WM_APP_DRAGEND {
        // finalize from OUR computed rect: the async SetWindowPos above has not landed
        // in VLC yet, so a fresh GetWindowRect would be stale
        let chrome_w = (d.start.right - d.start.left) - (d.vis.right - d.vis.left);
        let chrome_h = (d.start.bottom - d.start.top) - (d.vis.bottom - d.vis.top);
        native::finish_drag(&target, resizing, chrome_w, chrome_h);
        tracker.reset_debounce(); // convergence re-clips from a clean debounce
    }
}

fn owner_is_current(cached_pid: u32, current_pid: u32) -> bool {
    cached_pid != 0 && cached_pid == current_pid
}

fn pointer_delta(current: i32, origin: i32) -> i64 {
    i64::from(current) - i64::from(origin)
}

fn poll_request(argv: &[String]) {
    match state::consume_request(&state::request_path()).as_deref() {
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
            let pip = PIP.get();
            if pip.hwnd != 0
                && GetForegroundWindow() as isize == pip.hwnd
                && owner_is_current(pip.pid, native::window_owner(pip.hwnd))
            {
                if k.vkCode == VK_F as u32 {
                    return 1; // swallow F -> no fullscreen while in PiP
                }
                // a fullscreen-origin PiP rides on VLC's live internal fullscreen state:
                // Esc would make Qt leave it underneath the reshape (SPEC 7). BARE Esc
                // only - Alt+Esc/Ctrl+Esc are OS shortcuts VLC doesn't bind
                if k.vkCode == VK_ESCAPE as u32
                    && pip.fs
                    && [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
                        .into_iter()
                        .all(|vk| GetAsyncKeyState(vk as i32) as u16 & 0x8000 == 0)
                {
                    return 1;
                }
            }
        }
        // CallNextHookEx's hhk parameter is documented as ignored
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }
}

// Rate-limit clicks over the PiP window: swallow every button-down within double-click
// time+rect of the last ALLOWED button-down, so no two clicks the OS actually receives
// can ever pair into a synthesized WM_LBUTTONDBLCLK (swallowing only the 2nd click lets
// a triple click through - the OS pairs clicks 1+3 and VLC fullscreens).
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            if wparam as u32 == WM_LBUTTONDOWN {
                let m = &*(lparam as *const MSLLHOOKSTRUCT);
                let pip = PIP.get();
                let h = pip.hwnd;
                if h != 0
                    && owner_is_current(pip.pid, native::window_owner(h))
                    && GetAncestor(WindowFromPoint(m.pt), GA_ROOT) as isize == h
                {
                    let mut c = CLICK.get();
                    let burst = m.time.wrapping_sub(c.time) <= GetDoubleClickTime()
                        && pointer_delta(m.pt.x, c.x).abs()
                            <= i64::from(GetSystemMetrics(SM_CXDOUBLECLK))
                        && pointer_delta(m.pt.y, c.y).abs()
                            <= i64::from(GetSystemMetrics(SM_CYDOUBLECLK));
                    if burst {
                        c.swallow_next_up = true;
                        CLICK.set(c);
                        return 1;
                    }
                    CLICK.set(Click {
                        time: m.time,
                        x: m.pt.x,
                        y: m.pt.y,
                        ..c
                    });
                    // arm a potential drag; the zone (interior vs 16px band) picks move vs
                    // resize; gen bump invalidates any queued message from a prior drag
                    let mut d = Drag {
                        generation: DRAG.get().generation.wrapping_add(1),
                        ..Drag::default()
                    };
                    if let Some((vis, wr)) = native::gesture_rects(h) {
                        d.zone =
                            geometry::classify_zone(m.pt.x, m.pt.y, &vis, native::drag_band(h));
                        d.vis = vis;
                        d.start = wr;
                        d.origin = (m.pt.x, m.pt.y);
                        d.hwnd = h;
                        d.state = DragState::Armed;
                    } // either probe failed: window vanished under the click - stay idle
                    DRAG.set(d);
                }
            } else if wparam as u32 == WM_MOUSEMOVE {
                // hot path for ALL system mouse movement: one Cell read when idle
                let mut d = DRAG.get();
                if d.state != DragState::Idle {
                    let m = &*(lparam as *const MSLLHOOKSTRUCT);
                    if d.state == DragState::Armed
                        && (pointer_delta(m.pt.x, d.origin.0).abs()
                            > i64::from(GetSystemMetrics(SM_CXDRAG))
                            || pointer_delta(m.pt.y, d.origin.1).abs()
                                > i64::from(GetSystemMetrics(SM_CYDRAG)))
                    {
                        d.state = DragState::Active;
                    }
                    if d.state == DragState::Active {
                        d.latest = (m.pt.x, m.pt.y);
                        if !d.move_pending {
                            d.move_pending = true;
                            PostThreadMessageW(
                                GetCurrentThreadId(),
                                WM_APP_DRAG,
                                0,
                                d.generation as isize,
                            );
                        }
                    }
                    DRAG.set(d);
                }
            } else if wparam as u32 == WM_LBUTTONUP {
                let mut d = DRAG.get();
                let st = d.state;
                d.state = DragState::Idle;
                DRAG.set(d);
                if st == DragState::Active {
                    PostThreadMessageW(
                        GetCurrentThreadId(),
                        WM_APP_DRAGEND,
                        0,
                        d.generation as isize,
                    );
                }
                let mut c = CLICK.get();
                if c.swallow_next_up {
                    c.swallow_next_up = false;
                    CLICK.set(c);
                    let pip = PIP.get();
                    if pip.hwnd != 0 && owner_is_current(pip.pid, native::window_owner(pip.hwnd)) {
                        return 1; // keep the input stream paired: drop the up of a dropped down
                    }
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }
}

#[cfg(test)]
mod internal_tests {
    use super::*;

    #[test]
    fn cached_owner_requires_nonzero_matching_pid() {
        assert!(owner_is_current(42, 42));
        assert!(!owner_is_current(42, 99));
        assert!(!owner_is_current(0, 0));
    }

    #[test]
    fn pointer_delta_spans_the_full_i32_domain() {
        assert_eq!(pointer_delta(i32::MIN, i32::MAX), -4_294_967_295);
        assert_eq!(pointer_delta(i32::MAX, i32::MIN), 4_294_967_295);
    }

    #[test]
    fn drag_reset_preserves_generation_while_clearing_gesture() {
        let d = Drag {
            state: DragState::Active,
            generation: u32::MAX,
            hwnd: 42,
            move_pending: true,
            ..Drag::default()
        };

        let reset = d.reset();

        assert_eq!(reset.generation, u32::MAX);
        assert!(reset.state == DragState::Idle);
        assert_eq!(reset.hwnd, 0);
        assert!(!reset.move_pending);
    }
}
