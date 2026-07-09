use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, CreateRectRgn, DeleteObject, GetMonitorInfoW, GetRgnBox, GetWindowRgn,
    MonitorFromRect, MonitorFromWindow, SetWindowRgn, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTONULL, NULLREGION,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetClientRect, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE, HWND_NOTOPMOST,
    HWND_TOPMOST, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSENDCHANGING, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, SW_RESTORE,
    WS_CAPTION, WS_EX_TOPMOST, WS_MAXIMIZE, WS_THICKFRAME,
};
use windows_sys::core::BOOL;

use crate::geometry::{self, RegionPlan};
use crate::options::PipOptions;
use crate::state::{self, PipState};

// Handles live in statics and the state file, so they travel as isize (windows-sys 0.61
// handles are *mut c_void: not Send/Sync). Cast at the call boundary only.
fn hw(h: isize) -> HWND {
    h as HWND
}

// Closure-based window enumeration (return false to stop). Only for EnumWindows /
// EnumChildWindows - the LL hook callbacks stay plain unsafe extern fns (SPEC R7).
unsafe extern "system" fn enum_tramp<F: FnMut(HWND) -> bool>(h: HWND, l: LPARAM) -> BOOL {
    unsafe { (*(l as *mut F))(h) as BOOL }
}

fn enum_windows<F: FnMut(HWND) -> bool>(mut f: F) {
    unsafe { EnumWindows(Some(enum_tramp::<F>), &raw mut f as LPARAM) };
}

fn enum_children<F: FnMut(HWND) -> bool>(top: isize, mut f: F) {
    unsafe { EnumChildWindows(hw(top), Some(enum_tramp::<F>), &raw mut f as LPARAM) };
}

fn class_starts_with(h: HWND, prefix: &str) -> bool {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), 128) };
    String::from_utf16_lossy(&buf[..n as usize]).starts_with(prefix)
}

/// Owner PID (0 when the window is gone).
fn window_owner(h: isize) -> u32 {
    let mut p = 0u32;
    unsafe {
        GetWindowThreadProcessId(hw(h), &mut p);
    }
    p
}

pub fn enable_dpi_awareness() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

// ---- find the VLC player window ----------------------------------------------------

fn vlc_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return pids;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case("vlc.exe") {
                    pids.push(e.th32ProcessID);
                }
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    pids
}

pub fn find_player() -> isize {
    let pids = vlc_pids();
    if pids.is_empty() {
        return 0;
    }
    let (mut best, mut biggest, mut biggest_area) = (0isize, 0isize, 0i64);
    enum_windows(|h| {
        if unsafe { IsWindowVisible(h) } == 0 || !pids.contains(&window_owner(h as isize)) {
            return true;
        }
        let mut buf = [0u16; 256];
        let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), 256) };
        if n == 0 {
            return true; // empty title: VLC's hidden/extension windows
        }
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        if title.to_ascii_lowercase().contains("vlc media player") {
            best = h as isize;
            return false; // stop enumeration
        }
        let area = window_rect(h as isize)
            .map_or(0, |r| (r.right - r.left) as i64 * (r.bottom - r.top) as i64);
        if area > biggest_area {
            biggest_area = area;
            biggest = h as isize;
        }
        true
    });
    if best != 0 { best } else { biggest }
}

// ---- state ownership ----------------------------------------------------------------

// Windows recycles HWND values: after VLC dies, the saved handle can belong to another
// app. Handle validity alone would pass and we'd reshape a foreign window; require the
// owner PID recorded at Enter (a destroyed or recycled HWND yields 0 or a foreign pid).
pub fn owns_state(s: &PipState) -> bool {
    s.pid != 0 && window_owner(s.hwnd) == s.pid
}

// Read-only: a stale state here may be a pending reopen-heal record whose lifecycle
// belongs to maintain_region - a mere status query must not destroy it.
pub fn in_pip() -> bool {
    state::load(&state::state_path()).is_some_and(|s| owns_state(&s))
}

// ---- fullscreen-origin PiP -----------------------------------------------------------
// PiP from a fullscreen VLC is the same instant reshape; VLC's internal fullscreen state
// stays ON for the whole session (Qt only restores its windowed geometry from an
// UNTOUCHED fullscreen window - leaving fullscreen first or after desyncs it, SPEC 7).
// Exit restores the saved fullscreen style+rect verbatim. Meanwhile the controller strip
// is kept veiled each tick and the keyboard hook swallows Esc/F.

/// Was this PiP taken from a fullscreen VLC? The saved pre-PiP style tells (caption
/// fully absent). Drives the Esc swallow, the strip veil, and the heal skip.
pub fn fs_origin(style: isize) -> bool {
    style & WS_CAPTION as isize != WS_CAPTION as isize
}

fn for_each_fs_controller(pid: u32, f: impl Fn(HWND)) {
    if pid == 0 {
        return;
    }
    enum_windows(|w| {
        // pid first: it is one cheap call, while the class read allocates per window
        if window_owner(w as isize) == pid && class_starts_with(w, "Qt5QWindowToolSaveBits") {
            f(w);
        }
        true
    });
}

/// Make VLC's fullscreen controller strip (separate topmost Qt window) unrenderable.
/// VLC re-shows it on any hover or refocus of a vout it believes fullscreen - faster
/// than any tick could re-hide, so hiding alone flashed the strip for up to one tick.
/// An EMPTY window region kills rendering no matter how often VLC shows the window and
/// survives its show/hide cycles (probed live; alpha-0 does NOT survive - Qt re-applies
/// its configured opacity on show). Hidden strips are veiled too: pre-armed before
/// their first show. Runs in enter() and every tick (a recreated strip gets caught).
pub fn veil_fs_controller(pid: u32) {
    for_each_fs_controller(pid, |w| unsafe {
        if IsWindowVisible(w) != 0 {
            ShowWindow(w, SW_HIDE);
        }
        let probe = CreateRectRgn(0, 0, 0, 0);
        let veiled = GetWindowRgn(w, probe) == NULLREGION;
        DeleteObject(probe);
        if !veiled {
            let empty = CreateRectRgn(0, 0, 0, 0);
            if SetWindowRgn(w, empty, 1) == 0 {
                DeleteObject(empty); // the system owns the region only on success
            }
        }
    });
}

/// Give the strip back when a session ends (exit, dissolve, rollback, record cleanup):
/// drop the veil. Inert when nothing was veiled, so callers need no fs_origin gate.
pub fn unveil_fs_controller(pid: u32) {
    for_each_fs_controller(pid, |w| unsafe {
        SetWindowRgn(w, std::ptr::null_mut(), 1);
    });
}

// ---- window / region primitives -------------------------------------------------------

// windows-sys RECT <-> geometry::Rect, converted only here (geometry stays FFI-free)
fn from_win(r: &RECT) -> geometry::Rect {
    geometry::Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

fn to_win(r: &geometry::Rect) -> RECT {
    RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

pub fn window_rect(h: isize) -> Option<geometry::Rect> {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hw(h), &mut r) == 0 {
            return None;
        }
        Some(from_win(&r))
    }
}

fn styles(h: isize) -> (isize, isize) {
    unsafe { (GetWindowLongPtrW(hw(h), GWL_STYLE), GetWindowLongPtrW(hw(h), GWL_EXSTYLE)) }
}

pub fn work_area(h: isize) -> geometry::Rect {
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(hw(h), MONITOR_DEFAULTTONEAREST), &mut mi);
        from_win(&mi.rcWork)
    }
}

// VLC 3.x hosts the video in a native child whose class starts with "VLC video main";
// the minimal look clips the top-level window to it via SetWindowRgn.
fn find_video_child(top: isize) -> isize {
    let mut found = 0isize;
    enum_children(top, |c| {
        if unsafe { IsWindowVisible(c) } != 0 && class_starts_with(c, "VLC video main") {
            found = c as isize;
            return false;
        }
        true
    });
    found
}

// Every region this program sets has a nonempty box, so presence == nonempty box.
fn has_region(h: isize) -> bool {
    region_box(h).is_some()
}

fn region_box(h: isize) -> Option<geometry::Rect> {
    unsafe {
        let probe = CreateRectRgn(0, 0, 0, 0);
        let mut b: RECT = std::mem::zeroed();
        let r = if GetWindowRgn(hw(h), probe) != 0 && GetRgnBox(probe, &mut b) > NULLREGION {
            Some(from_win(&b))
        } else {
            None
        };
        DeleteObject(probe);
        r
    }
}

// Apply a rectangular region (window-relative); the system owns rgn only on success.
fn set_region(h: isize, r: &geometry::Rect) {
    unsafe {
        let rgn = CreateRectRgn(r.left, r.top, r.right, r.bottom);
        if SetWindowRgn(hw(h), rgn, 1) == 0 {
            DeleteObject(rgn);
        }
    }
}

// ---- drag gesture primitives (hook arms, pump applies) --------------------------------

// The minimal-look region clips painting AND hit-testing, so the gesture surface is the
// region box (offset to screen coords by the window origin), not the window rect. One
// call returns (visible, window) so had_rgn compares a coherent snapshot.
pub fn gesture_rects(h: isize) -> Option<(geometry::Rect, geometry::Rect)> {
    let wr = window_rect(h)?;
    let vis = region_box(h).map_or(wr, |b| geometry::Rect {
        left: wr.left + b.left,
        top: wr.top + b.top,
        right: wr.left + b.right,
        bottom: wr.top + b.bottom,
    });
    Some((vis, wr))
}

pub fn drag_band(h: isize) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hw(h)) };
    if dpi == 0 { 16 } else { 16 * dpi as i32 / 96 }
}

pub fn drag_move(h: isize, r: &geometry::Rect) {
    unsafe {
        SetWindowPos(
            hw(h), std::ptr::null_mut(), r.left, r.top, 0, 0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | SWP_NOSENDCHANGING,
        );
    }
}

pub fn drag_resize(h: isize, r: &geometry::Rect, clip: Option<&geometry::Rect>) {
    unsafe {
        match clip {
            Some(c) => {
                // keep the minimal look live through the resize (region is window-relative)
                set_region(h, c);
            }
            None => {
                if has_region(h) {
                    SetWindowRgn(hw(h), std::ptr::null_mut(), 1); // no clip context: show it all
                }
            }
        }
        SetWindowPos(
            hw(h), std::ptr::null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | SWP_NOSENDCHANGING,
        );
    }
}

/// Adopt the drag result: corner = nearest as of `fin`; a resize also adopts the new
/// video size (`fin` minus chrome measured at drag start). Works from the CALLER's
/// computed rect - the final async SetWindowPos may not have landed in VLC yet.
pub fn finish_drag(fin: &geometry::Rect, resized: bool, chrome_w: i32, chrome_h: i32) {
    let path = state::state_path();
    let Some(mut s) = state::load(&path) else { return };
    if !owns_state(&s) {
        return; // VLC died mid-drag: next tick's maintain_region cleans up
    }
    let work = work_area(s.hwnd);
    s.corner = geometry::nearest_corner(fin, &work);
    if resized {
        let (tw, th) = (fin.right - fin.left - chrome_w, fin.bottom - fin.top - chrome_h);
        if tw > 0 && th > 0 {
            s.target_w = tw;
            s.target_h = th;
        }
    }
    let _ = state::save(&s, &path); // failure swallowed: the gesture already holds on screen
    crate::options::save_config(s.target_w, s.target_h, s.corner);
}

// ---- enter / exit / toggle ------------------------------------------------------------

// Client-relative chrome around the video child (menu above, controller below): Qt
// widgets in the CLIENT area, so the offsets survive the border strip and predict where
// the child lands after the reshape. None when not playing or mid-relayout garbage.
fn client_chrome(h: isize) -> Option<(i32, i32, i32, i32)> {
    let child = find_video_child(h);
    if child == 0 {
        return None;
    }
    let cr = window_rect(child)?;
    let mut client: RECT = unsafe { std::mem::zeroed() };
    let mut origin = POINT { x: 0, y: 0 };
    if unsafe { GetClientRect(hw(h), &mut client) == 0 || ClientToScreen(hw(h), &mut origin) == 0 } {
        return None;
    }
    let l = cr.left - origin.x;
    let t = cr.top - origin.y;
    let r = (origin.x + client.right) - cr.right;
    let b = (origin.y + client.bottom) - cr.bottom;
    // no side negative, axis sums in plan_region's envelope: a rect the converger
    // would forever Skip must never land
    if l >= 0 && t >= 0 && r >= 0 && b >= 0 && geometry::chrome_ok(l + r, t + b) {
        Some((l, t, r, b))
    } else {
        None
    }
}

pub fn enter(h: isize, o: &PipOptions) -> bool {
    if h == 0 || in_pip() {
        return false;
    }
    // overwriting a stale record consumes it, like exit and heal do - it may have left a
    // veiled strip on a still-running VLC (recycled hwnd): unveil it
    if let Some(old) = state::load(&state::state_path()) {
        unveil_fs_controller(old.pid);
    }
    // restore FIRST: the off-screen iconic rect must never become the restore state
    if unsafe { IsIconic(hw(h)) } != 0 {
        unsafe { ShowWindow(hw(h), SW_RESTORE) };
    }
    let r = window_rect(h).unwrap_or_default();
    let (style, ex) = styles(h);
    let pid = window_owner(h);
    // save state FIRST, so a failed save can never leave a mutated window with no restore data
    let s = PipState {
        hwnd: h,
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
        style,
        ex_style: ex,
        target_w: o.w,
        target_h: o.h,
        corner: o.corner,
        margin: o.margin,
        min: o.min,
        pid,
    };
    if state::save(&s, &state::state_path()).is_err() {
        return false; // nothing mutated yet: fail cleanly, retry next toggle
    }
    // chrome measured pre-strip lets enter land in ONE SetWindowPos at the final
    // chrome-compensated rect with the region already applied (no grow-then-clip flash)
    let chrome = if o.min { client_chrome(h) } else { None };
    if fs_origin(style) {
        // the user was likely just hovering the fullscreen video, so the strip is on
        // screen RIGHT NOW: veil it before the PiP lands, not a tick later
        veil_fs_controller(pid);
    }
    // WS_MAXIMIZE too: a zoomed window keeps IsZoomed, and Aero snap would then
    // bounce the PiP back to Qt's normal placement rect
    unsafe { SetWindowLongPtrW(hw(h), GWL_STYLE, style & !((WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZE) as isize)) };
    let wa = work_area(h);
    let (vx, vy) = geometry::compute_corner(&wa, o.w, o.h, o.corner, o.margin);
    let (x, y, tw, th) = match chrome {
        Some((cl, ct, cr, cb)) => (vx - cl, vy - ct, o.w + cl + cr, o.h + ct + cb),
        None => (vx, vy, o.w, o.h), // not playing: converger takes over once a child exists
    };
    let ok = unsafe { SetWindowPos(hw(h), HWND_TOPMOST, x, y, tw, th, SWP_FRAMECHANGED | SWP_SHOWWINDOW) != 0 };
    if ok {
        if let Some((cl, ct, _, _)) = chrome {
            set_region(h, &geometry::Rect { left: cl, top: ct, right: cl + o.w, bottom: ct + o.h });
        }
    } else {
        // e.g. UIPI vs elevated VLC: don't claim in-PiP
        unsafe { SetWindowLongPtrW(hw(h), GWL_STYLE, style) };
        unveil_fs_controller(pid); // the rollback ends the session: no veil may outlive it
        state::try_delete(&state::state_path());
    }
    ok
}

// Shared exit/dissolve prefix: drop the minimal-look clip BEFORE restoring the saved
// styles. WS_EX_TOPMOST only changes via SetWindowPos, so the returned after-handle
// (honoring the user's own always-on-top) goes into the caller's SetWindowPos.
fn restore_frame(h: isize, style: isize, ex_style: isize) -> HWND {
    unsafe {
        SetWindowRgn(hw(h), std::ptr::null_mut(), 1);
        SetWindowLongPtrW(hw(h), GWL_STYLE, style);
        SetWindowLongPtrW(hw(h), GWL_EXSTYLE, ex_style);
    }
    if ex_style & (WS_EX_TOPMOST as isize) != 0 { HWND_TOPMOST } else { HWND_NOTOPMOST }
}

pub fn exit_pip() -> bool {
    let path = state::state_path();
    let Some(s) = state::load(&path) else { return false };
    if !owns_state(&s) {
        unveil_fs_controller(s.pid); // hwnd recycled with VLC alive: give the strip back
        state::try_delete(&path); // stale: VLC gone or hwnd recycled
        return false;
    }
    let h = s.hwnd;
    let after = restore_frame(h, s.style, s.ex_style);
    let ok = unsafe { SetWindowPos(hw(h), after, s.x, s.y, s.w, s.h, SWP_FRAMECHANGED | SWP_SHOWWINDOW) != 0 };
    if ok || unsafe { IsWindow(hw(h)) } == 0 {
        unveil_fs_controller(s.pid); // session over: a restored fullscreen gets its strip back
        state::try_delete(&path); // live-window restore failure keeps state so the next toggle retries
    }
    ok
}

pub fn toggle(o: &PipOptions) -> bool {
    if in_pip() { exit_pip() } else { enter(find_player(), o) }
}

// ---- status (write-only JSON; smoke-test.ps1 parses it - shape is frozen, SPEC 6.4) ---

pub fn status() -> String {
    let h = find_player();
    if h == 0 {
        return r#"{"found":false}"#.to_string();
    }
    let r = window_rect(h).unwrap_or_default();
    let (style, ex) = styles(h);
    format!(
        r#"{{"found":true,"hwnd":{},"x":{},"y":{},"w":{},"h":{},"caption":{},"topmost":{},"inPip":{},"minimal":{}}}"#,
        h,
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        style & (WS_CAPTION as isize) == (WS_CAPTION as isize), // BOTH caption bits
        ex & (WS_EX_TOPMOST as isize) != 0,
        in_pip(),
        has_region(h),
    )
}

// ---- minimal look (Ctrl+H-like) via SetWindowRgn on the video child area -------------

// Cross-tick converger memory. `prev` holds last tick's (window, child) rects for the
// stability debounce; `fs_prev` is the fullscreen-origin dissolve watch's baseline (the
// window rect last seen WITH a live video child); `heal_tries` bounds the reopen heal
// so an unhealable window (e.g. elevated VLC: UIPI silently swallows the SetWindowPos)
// is never fought forever - ~6s of ticks, then the record is dropped; `heal_wait`
// throttles the process snapshots while the heal waits for a relaunch.
#[derive(Default)]
pub struct RegionTracker {
    prev: Option<(geometry::Rect, geometry::Rect)>,
    fs_prev: Option<geometry::Rect>,
    heal_tries: u32,
    heal_wait: u32,
}

impl RegionTracker {
    /// Drop only the stability debounce (our own reshapes invalidate it). The dissolve
    /// baseline deliberately survives: wiping it across a drag would disarm the watch
    /// exactly when media can end mid-gesture, leaving a stuck fullscreen-origin session.
    pub fn reset_debounce(&mut self) {
        self.prev = None;
    }
}

/// Qt left fullscreen UNDERNEATH a fullscreen-origin PiP (media end and stop do this
/// with no input; the window balloons to Qt's windowed geometry within a tick). Dissolve
/// the session: frame back at Qt's chosen rect, state dropped - the saved fullscreen
/// rect must never be restored onto an internally windowed VLC.
fn dissolve_fs_pip(s: &PipState, path: &Path) {
    let h = s.hwnd;
    let after = restore_frame(h, s.style | (WS_CAPTION | WS_THICKFRAME) as isize, s.ex_style);
    unsafe {
        SetWindowPos(hw(h), after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED);
    }
    unveil_fs_controller(s.pid);
    state::try_delete(path);
}

/// Converging per-tick maintenance (daemon timer + one-shot enter): no video -> clear
/// region; child not at target size -> resize with chrome compensation; child at target
/// -> clip to the video area. Acts only on STABLE frames (window+child rects unchanged
/// since the previous tick): VLC re-fits the child asynchronously after our resize, so
/// a fresh measurement can be stale and yield garbage chrome.
pub fn maintain_region(t: &mut RegionTracker, s: Option<PipState>) {
    let path = state::state_path();
    let Some(s) = s else {
        t.prev = None;
        return;
    };
    if !owns_state(&s) {
        t.prev = None;
        heal_reopened(t, &s, &path);
        return;
    }
    t.heal_tries = 0; // a live owned PiP ends any interrupted heal cleanly
    t.heal_wait = 0;
    let h = s.hwnd;
    let child = find_video_child(h);
    let wr = window_rect(h).unwrap_or_default(); // one snapshot serves watch and debounce

    // fullscreen-origin dissolve watch - BEFORE the min gate, it guards every fs session
    if fs_origin(s.style) {
        if child != 0 {
            // baseline: the rect while video is alive (our reshapes and drags included)
            t.fs_prev = Some(wr);
        } else if t.fs_prev.is_some_and(|p| p != wr) {
            dissolve_fs_pip(&s, &path);
            *t = RegionTracker::default();
            return;
        }
    } else {
        t.fs_prev = None;
    }

    if !s.min {
        return;
    }

    if child == 0 {
        t.prev = None;
        if has_region(h) {
            unsafe { SetWindowRgn(hw(h), std::ptr::null_mut(), 1) }; // playback stopped: show full mini UI
        }
        return;
    }

    let cr = window_rect(child).unwrap_or_default();
    let stable = t.prev == Some((wr, cr));
    t.prev = Some((wr, cr));
    if !stable {
        return; // wait until VLC's re-layout settles
    }

    match geometry::plan_region(&wr, &cr, s.target_w, s.target_h, s.corner, s.margin, || work_area(h)) {
        RegionPlan::Skip => {}
        RegionPlan::Resize { x, y, w, h: th } => {
            unsafe { SetWindowPos(hw(h), HWND_TOPMOST, x, y, w, th, SWP_FRAMECHANGED) };
            t.prev = None; // our own resize invalidates the measurement
        }
        RegionPlan::Clip(c) => {
            // verify the box, not just presence: a live-clipped resize drag leaves an
            // approximate region that convergence must confirm or correct
            if region_box(h) != Some(c) {
                set_region(h, &c);
            }
        }
    }
}

/// VLC that closes while in PiP persists the PiP geometry as its own (Qt saves on exit),
/// so its next launch opens full-size at the PiP origin, overflowing the screen. The
/// stale state file is kept as a pending-restore record; when a new player window
/// appears, apply the saved pre-PiP rect and delete the record only once the rect is
/// observed to stick - VLC's own startup positioning must not win the race.
fn heal_reopened(t: &mut RegionTracker, s: &PipState, path: &Path) {
    if s.w <= 0 || s.h <= 0 || s.pid == 0 {
        state::try_delete(path); // garbage record (pid is 0 if VLC died mid-enter): not healable
        return;
    }
    if fs_origin(s.style) {
        // a fullscreen-origin PiP's record holds the FULLSCREEN rect - never heal a
        // reopened window to it. Qt believed fullscreen the whole session, so it
        // persisted its true windowed geometry itself: nothing to heal.
        unveil_fs_controller(s.pid); // no-op if the process died with the session
        state::try_delete(path);
        return;
    }
    // VLC may stay closed for days: snapshot ~once a second while waiting, not per tick
    t.heal_wait += 1;
    if t.heal_wait % 7 != 1 {
        return;
    }
    let pids = vlc_pids();
    if pids.contains(&s.pid) {
        state::try_delete(path); // the recorded VLC still runs (hwnd recycled): not a close-in-PiP
        return;
    }
    if pids.is_empty() {
        return; // VLC not back yet: keep waiting
    }
    t.heal_wait = 0; // VLC is back: converge at full cadence (the 40-try cap still bounds it)
    let h2 = find_player();
    if h2 == 0 {
        return;
    }
    if unsafe { IsIconic(hw(h2)) } != 0 {
        return; // heal the normal placement once restored - the iconic rect is garbage
    }
    let target = geometry::Rect { left: s.x, top: s.y, right: s.x + s.w, bottom: s.y + s.h };
    if unsafe { MonitorFromRect(&to_win(&target), MONITOR_DEFAULTTONULL) }.is_null() {
        state::try_delete(path); // monitor layout changed: VLC's own placement is saner
        return;
    }
    if window_rect(h2) == Some(target) {
        t.heal_tries = 0;
        state::try_delete(path); // heal landed and stuck: done
        return;
    }
    t.heal_tries += 1;
    if t.heal_tries > 40 {
        t.heal_tries = 0;
        state::try_delete(path); // not converging: stop fighting the window
        return;
    }
    unsafe { SetWindowPos(hw(h2), std::ptr::null_mut(), s.x, s.y, s.w, s.h, SWP_NOZORDER | SWP_NOACTIVATE) };
}

