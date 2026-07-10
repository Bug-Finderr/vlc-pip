use std::cell::Cell;
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
    TranslateMessage, UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, HHOOK, KBDLLHOOKSTRUCT,
    MSG, MSLLHOOKSTRUCT, SM_CXDOUBLECLK, SM_CXDRAG, SM_CYDOUBLECLK, SM_CYDRAG, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_SYSKEYDOWN, WM_TIMER,
};

use crate::state::PipState;
use crate::{geometry, native, options, state};

// Pump-thread hook dispatch: whole-struct Cells, no disk I/O in hooks (SPEC 3, R7).

const WM_APP_DRAG: u32 = WM_APP;
const WM_APP_DRAGEND: u32 = WM_APP + 1;

#[derive(Clone, Copy, Default, PartialEq)]
enum DragState {
    #[default]
    Idle,
    Armed,
    Moving,
    Resizing,
}

impl DragState {
    /// A gesture that owns the window (past the arm threshold, button still down).
    fn active(self) -> bool {
        matches!(self, Self::Moving | Self::Resizing)
    }
}

// The pump validates the generation from lParam: a hook-side reset or rapid re-arm can never apply a stale delta.
#[derive(Clone, Copy, Default)]
struct Drag {
    state: DragState,
    zone: geometry::DragZone,
    generation: u32,
    hwnd: isize,
    origin: (i32, i32),
    start: geometry::Rect,
    vis: geometry::Rect,
    had_rgn: bool,
    latest: (i32, i32),
    move_pending: bool,
}

/// The last ALLOWED button-down (time in u32 hook ms, wraps) for the click rate limit.
#[derive(Clone, Copy, Default)]
struct Click {
    time: u32,
    x: i32,
    y: i32,
    swallow_next_up: bool,
}

/// The current PiP (hwnd 0 = none); `fs` = fullscreen-origin; `pid` is owns_state-verified, never re-derived from a recyclable handle.
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

fn refresh_hook_cache(s: Option<PipState>) {
    // owns_state, not IsWindow: heal records keep stale states alive, and a recycled HWND must never re-arm guards on a foreign window
    PIP.set(s.filter(native::owns_state).map_or(Pip::default(), |s| Pip {
        hwnd: s.hwnd,
        fs: native::fs_origin(s.style),
        pid: s.pid,
    }));
}

/// Hooks exist only while a session is live (SPEC 7); only null slots install, so failed installs retry and live handles never leak.
fn sync_hooks(hooks: &mut (HHOOK, HHOOK)) {
    if PIP.get().hwnd != 0 {
        unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            if hooks.0.is_null() {
                hooks.0 = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
            }
            if hooks.1.is_null() {
                hooks.1 = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
            }
        }
    } else if !hooks.0.is_null() || !hooks.1.is_null() {
        for h in [hooks.0, hooks.1] {
            if !h.is_null() {
                unsafe { UnhookWindowsHookEx(h) };
            }
        }
        *hooks = (std::ptr::null_mut(), std::ptr::null_mut());
        // a gesture must not outlive its hooks; the click cell keeps its last-allowed-down so clicks cannot pair across sessions (SPEC 7)
        DRAG.set(Drag::default());
        CLICK.set(Click { swallow_next_up: false, ..CLICK.get() });
    }
}

pub fn run(argv: &[String]) -> i32 {
    // single instance; second instance exits 0 before touching any file
    let name: Vec<u16> = "VlcPipDaemon\0".encode_utf16().collect();
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) }; // held for process lifetime
    if mutex.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return 0; // already running, or the name is unobtainable: never double-run
    }

    unsafe {
        RegisterHotKey(std::ptr::null_mut(), 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_P as u32);
        SetTimer(std::ptr::null_mut(), 0, 150, None); // WM_TIMER -> thread queue
    }
    let mut hooks: (HHOOK, HHOOK) = (std::ptr::null_mut(), std::ptr::null_mut());

    // Heartbeat: pip.lua checks the leading epoch for freshness (a force-killed daemon
    // can't delete the file); write failures are swallowed - never kill the pump (SPEC 6.3)
    let alive = state::alive_path();
    let beat = |last: &mut Instant| {
        *last = Instant::now();
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = std::fs::write(&alive, epoch.to_string());
    };
    let mut last_beat = Instant::now();
    refresh_hook_cache(state::load(&state::state_path())); // a daemon restarted while already in PiP must be guarded from the first message
    sync_hooks(&mut hooks);
    beat(&mut last_beat);

    let mut tracker = native::RegionTracker::default();
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        if msg.message == WM_HOTKEY {
            native::toggle(&options::effective(argv));
            refresh_hook_cache(state::load(&state::state_path()));
            sync_hooks(&mut hooks); // armed before the user can physically click the fresh PiP
        } else if msg.message == WM_TIMER {
            if last_beat.elapsed() > Duration::from_secs(3) {
                beat(&mut last_beat);
            }
            poll_request(argv);
            // one snapshot per tick, loaded after poll_request so a request-triggered toggle lands this same tick
            let s = state::load(&state::state_path());
            refresh_hook_cache(s);
            sync_hooks(&mut hooks);
            let pip = PIP.get();
            if pip.fs {
                // VLC still believes it is fullscreen under this PiP: keep its strip unrenderable (SPEC 7)
                native::veil_fs_controller(pip.pid);
            }
            if DRAG.get().state.active() {
                tracker.reset_debounce(); // gestures own the window while dragging
            } else {
                native::maintain_region(&mut tracker, s);
            }
        } else if msg.message == WM_APP_DRAG || msg.message == WM_APP_DRAGEND {
            on_drag_msg(&msg, &mut tracker);
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        UnhookWindowsHookEx(hooks.0); // harmless when never installed (null)
        UnhookWindowsHookEx(hooks.1);
        UnregisterHotKey(std::ptr::null_mut(), 1);
    }
    let _ = std::fs::remove_file(&alive);
    0
}

// Snapshot the gesture BEFORE any Win32 call (SetWindowPos/SetWindowRgn can pump sent messages); the
// generation guard drops queued messages from a previous drag.
fn on_drag_msg(msg: &MSG, tracker: &mut native::RegionTracker) {
    let mut d = DRAG.get();
    d.move_pending = false;
    DRAG.set(d);
    if d.hwnd != PIP.get().hwnd || msg.lParam != d.generation as isize {
        return;
    }
    let (dx, dy) = (d.latest.0 - d.origin.0, d.latest.1 - d.origin.1);
    let resizing = msg.wParam == DragState::Resizing as usize;
    let target = if resizing {
        geometry::plan_resize(&d.start, d.zone, dx, dy, &native::work_area(d.hwnd))
    } else {
        geometry::Rect {
            left: d.start.left + dx,
            top: d.start.top + dy,
            right: d.start.right + dx,
            bottom: d.start.bottom + dy,
        }
    };
    if resizing {
        // clip to where the video will sit (chrome measured at drag start); convergence verifies after release
        let clip = d.had_rgn.then(|| geometry::resize_clip(&d.start, &d.vis, &target)).flatten();
        native::drag_resize(d.hwnd, &target, clip.as_ref());
    } else {
        native::drag_move(d.hwnd, &target);
    }
    if msg.message == WM_APP_DRAGEND {
        // finalize from OUR computed rect: the async SetWindowPos has not landed, a fresh GetWindowRect would be stale
        let chrome_w = (d.start.right - d.start.left) - (d.vis.right - d.vis.left);
        let chrome_h = (d.start.bottom - d.start.top) - (d.vis.bottom - d.vis.top);
        native::finish_drag(&target, resizing, chrome_w, chrome_h);
        tracker.reset_debounce(); // convergence re-clips from a clean debounce
    }
}

fn poll_request(argv: &[String]) {
    match state::consume_request(&state::request_path()).as_deref() {
        Some("toggle") => {
            native::toggle(&options::effective(argv));
        }
        Some("enter") => {
            native::enter(native::find_player(), &options::effective(argv));
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
            if pip.hwnd != 0 && GetForegroundWindow() as isize == pip.hwnd {
                if k.vkCode == VK_F as u32 {
                    return 1; // swallow F -> no fullscreen while in PiP
                }
                // Esc would make Qt leave fullscreen underneath the reshape; BARE Esc only - modified Esc is an OS shortcut (SPEC 7)
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

// Swallow any down within double-click time+rect of the last ALLOWED down: no two delivered clicks can pair into a dblclick (SPEC 7).
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            if wparam as u32 == WM_LBUTTONDOWN {
                let m = &*(lparam as *const MSLLHOOKSTRUCT);
                let h = PIP.get().hwnd;
                if h != 0 && GetAncestor(WindowFromPoint(m.pt), GA_ROOT) as isize == h {
                    let mut c = CLICK.get();
                    let burst = m.time.wrapping_sub(c.time) <= GetDoubleClickTime()
                        && (m.pt.x - c.x).abs() <= GetSystemMetrics(SM_CXDOUBLECLK)
                        && (m.pt.y - c.y).abs() <= GetSystemMetrics(SM_CYDOUBLECLK);
                    if burst {
                        c.swallow_next_up = true;
                        CLICK.set(c);
                        return 1;
                    }
                    CLICK.set(Click { time: m.time, x: m.pt.x, y: m.pt.y, ..c });
                    // arm a potential drag; the gen bump invalidates any queued message from a prior drag
                    let mut d = Drag { generation: DRAG.get().generation.wrapping_add(1), ..Drag::default() };
                    if let Some((vis, wr)) = native::gesture_rects(h) {
                        d.zone = geometry::classify_zone(m.pt.x, m.pt.y, &vis, native::drag_band(h));
                        d.vis = vis;
                        d.had_rgn = vis != wr; // visible == window means no region to preserve
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
                        && ((m.pt.x - d.origin.0).abs() > GetSystemMetrics(SM_CXDRAG)
                            || (m.pt.y - d.origin.1).abs() > GetSystemMetrics(SM_CYDRAG))
                    {
                        d.state = if d.zone == (0, 0) {
                            DragState::Moving
                        } else {
                            DragState::Resizing
                        };
                    }
                    if d.state.active() {
                        d.latest = (m.pt.x, m.pt.y);
                        if !d.move_pending {
                            d.move_pending = true;
                            PostThreadMessageW(GetCurrentThreadId(), WM_APP_DRAG, d.state as usize, d.generation as isize);
                        }
                    }
                    DRAG.set(d);
                }
            } else if wparam as u32 == WM_LBUTTONUP {
                let mut d = DRAG.get();
                let st = d.state;
                d.state = DragState::Idle;
                DRAG.set(d);
                if st.active() {
                    PostThreadMessageW(GetCurrentThreadId(), WM_APP_DRAGEND, st as usize, d.generation as isize);
                }
                let mut c = CLICK.get();
                if c.swallow_next_up {
                    c.swallow_next_up = false;
                    CLICK.set(c);
                    return 1; // keep the input stream paired: drop the up of a dropped down
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }
}
