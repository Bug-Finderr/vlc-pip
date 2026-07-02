use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering::Relaxed};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    VK_F, VK_P,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetAncestor, GetForegroundWindow, GetMessageW,
    GetSystemMetrics, IsWindow, PostQuitMessage, SetTimer, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    SM_CXDOUBLECLK, SM_CYDOUBLECLK, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_HOTKEY, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_SYSKEYDOWN, WM_TIMER,
};

use crate::options::PipOptions;
use crate::{native, request, state};

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

pub fn owns_alive_file() -> bool {
    OWNS_ALIVE_FILE.load(Relaxed)
}

fn refresh_state() {
    let h = state::load(&state::state_path())
        .map(|s| s.hwnd as isize)
        .filter(|&h| unsafe { IsWindow(h as HWND) } != 0)
        .unwrap_or(0);
    CACHED_HWND.store(h, Relaxed);
}

pub fn run(o: &PipOptions) -> i32 {
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
        if let Ok(c) = std::fs::read_to_string(&rp) {
            if c.trim() == "stop" {
                let _ = std::fs::remove_file(&rp);
            }
        }

        let hot = RegisterHotKey(std::ptr::null_mut(), 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_P as u32) != 0;
        let timer = SetTimer(std::ptr::null_mut(), 0, 150, None) != 0; // WM_TIMER -> thread queue
        let module = GetModuleHandleW(std::ptr::null());
        KB_HOOK.store(SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0) as isize, Relaxed);
        MOUSE_HOOK.store(SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0) as isize, Relaxed);

        // Heartbeat, not a marker: a force-killed daemon can't delete the file, so consumers
        // (pip.lua) check the leading epoch-seconds for freshness. Also carries arming
        // diagnostics. Write failures are swallowed: NEVER let the heartbeat kill the pump.
        let alive = std::env::temp_dir().join("vlc-pip-daemon.alive");
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

        let mut tracker = native::RegionTracker::new();
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                native::toggle(o);
                refresh_state();
            } else if msg.message == WM_TIMER {
                if last_beat.elapsed() > Duration::from_millis(3000) {
                    beat(&mut last_beat);
                }
                poll_request(o);
                refresh_state(); // the hook cache must reflect a request-triggered toggle within this tick
                native::maintain_region(&mut tracker);
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

fn poll_request(o: &PipOptions) {
    match request::consume(&request::request_path()).as_deref() {
        Some("toggle") => {
            native::toggle(o);
        }
        Some("enter") => {
            native::enter(native::find_player(), o);
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
                }
            } else if wparam as u32 == WM_LBUTTONUP && SWALLOW_NEXT_UP.load(Relaxed) {
                SWALLOW_NEXT_UP.store(false, Relaxed);
                return 1; // keep the input stream paired: drop the up of a dropped down
            }
        }
        CallNextHookEx(MOUSE_HOOK.load(Relaxed) as _, code, wparam, lparam)
    }
}
