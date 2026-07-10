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
    CallNextHookEx, DispatchMessageW, GA_ROOT, GetAncestor, GetForegroundWindow, GetMessageW,
    GetSystemMetrics, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, PostQuitMessage, PostThreadMessageW,
    SM_CXDOUBLECLK, SM_CXDRAG, SM_CYDOUBLECLK, SM_CYDRAG, SetTimer, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_HOTKEY,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_SYSKEYDOWN, WM_TIMER,
    WindowFromPoint,
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

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
enum DragState {
    #[default]
    Idle,
    Armed,
    Moving,
    Resizing,
}

// The drag gesture. The pump reads the mode from wParam and validates the generation
// from lParam, so a hook-side reset or rapid re-arm can never apply a stale delta.
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

fn refresh_state(s: Option<PipState>) {
    // full owner-PID guard (not just IsWindow): pending heal records keep stale states
    // alive indefinitely, so a recycled HWND must never re-arm the guards - or drags -
    // on a foreign window
    let s = s.filter(native::owns_state);
    PIP.set(Pip {
        hwnd: s.map_or(0, |s| s.hwnd),
        fs: s.is_some_and(|s| native::fs_origin(s.style)),
        pid: s.map_or(0, |s| s.pid),
    });
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

    let (hot, timer, kb, mouse) = unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        (
            RegisterHotKey(
                std::ptr::null_mut(),
                1,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                VK_P as u32,
            ) != 0,
            SetTimer(std::ptr::null_mut(), 0, 150, None) != 0, // WM_TIMER -> thread queue
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0),
            SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0),
        )
    };

    // Heartbeat, not a marker: a force-killed daemon can't delete the file, so consumers
    // (pip.lua) check the leading epoch-seconds for freshness. Also carries arming
    // diagnostics. Write failures are swallowed: NEVER let the heartbeat kill the pump.
    let alive = state::temp_path("vlc-pip-daemon.alive");
    let beat = |last: &mut Instant| {
        *last = Instant::now();
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let _ = std::fs::write(
            &alive,
            format!(
                "{epoch} pid={} hotkey={} timer={} kb={} mouse={}",
                std::process::id(),
                hot as i32,
                timer as i32,
                (!kb.is_null()) as i32,
                (!mouse.is_null()) as i32,
            ),
        );
    };
    OWNS_ALIVE_FILE.store(true, Relaxed);
    let mut last_beat = Instant::now();
    beat(&mut last_beat);
    refresh_state(state::load(&state::state_path())); // a daemon restarted while already in PiP must be guarded from the first message

    let mut tracker = native::RegionTracker::default();
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        if msg.message == WM_HOTKEY {
            native::toggle(&options::effective(argv));
            refresh_state(state::load(&state::state_path()));
        } else if msg.message == WM_TIMER {
            if last_beat.elapsed() > Duration::from_secs(3) {
                beat(&mut last_beat);
            }
            poll_request(argv);
            // one state snapshot per tick, shared by the hook cache and the converger;
            // it must reflect a request-triggered toggle within this same tick
            let s = state::load(&state::state_path());
            refresh_state(s);
            let pip = PIP.get();
            if pip.fs {
                // VLC still believes it is fullscreen under this PiP: keep its
                // controller strip off the screen (SPEC section 7)
                native::veil_fs_controller(pip.pid);
            }
            if DRAG.get().state >= DragState::Moving {
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
        UnhookWindowsHookEx(kb);
        UnhookWindowsHookEx(mouse);
        UnregisterHotKey(std::ptr::null_mut(), 1);
    }
    let _ = std::fs::remove_file(&alive);
    OWNS_ALIVE_FILE.store(false, Relaxed);
    0
}

// The coalesced drag apply. Snapshot the gesture BEFORE any Win32 call: SetWindowPos/
// SetWindowRgn can pump sent messages, and a hook re-arm mid-call must not be clobbered
// or half-read. The generation guard drops a queued message from a previous drag (a
// rapid release-and-repress re-arms the gesture; the stale delta must not apply).
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
        // live minimal look: clip to where the video will sit, using the per-side chrome
        // measured at drag start; convergence verifies the exact box after release
        let clip = d
            .had_rgn
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
            if pip.hwnd != 0 && GetForegroundWindow() as isize == pip.hwnd {
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
                        d.state = if d.zone == geometry::DragZone::Interior {
                            DragState::Moving
                        } else {
                            DragState::Resizing
                        };
                    }
                    if d.state >= DragState::Moving {
                        d.latest = (m.pt.x, m.pt.y);
                        if !d.move_pending {
                            d.move_pending = true;
                            PostThreadMessageW(
                                GetCurrentThreadId(),
                                WM_APP_DRAG,
                                d.state as usize,
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
                if st >= DragState::Moving {
                    PostThreadMessageW(
                        GetCurrentThreadId(),
                        WM_APP_DRAGEND,
                        st as usize,
                        d.generation as isize,
                    );
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
