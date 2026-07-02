use std::path::PathBuf;

use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CreateRectRgn, DeleteObject, GetMonitorInfoW, GetWindowRgn, MonitorFromWindow, SetWindowRgn,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE, HWND_NOTOPMOST,
    HWND_TOPMOST, SWP_FRAMECHANGED, SWP_SHOWWINDOW, SW_RESTORE, WS_CAPTION, WS_EX_TOPMOST,
    WS_MAXIMIZE, WS_THICKFRAME,
};
use windows_sys::core::BOOL;

use crate::geometry;
use crate::options::PipOptions;
use crate::state::{self, PipState, StatusInfo};

// Handles live in statics and the state file, so they travel as isize (windows-sys 0.61
// handles are *mut c_void: not Send/Sync). Cast at the call boundary only.
fn hw(h: isize) -> HWND {
    h as HWND
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

struct FindCtx<'a> {
    pids: &'a [u32],
    best: isize,
    biggest: isize,
    biggest_area: i64,
}

unsafe extern "system" fn find_player_cb(h: HWND, l: LPARAM) -> BOOL {
    unsafe {
        let ctx = &mut *(l as *mut FindCtx);
        if IsWindowVisible(h) == 0 {
            return 1;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        if !ctx.pids.contains(&pid) {
            return 1;
        }
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(h, buf.as_mut_ptr(), 256);
        if n == 0 {
            return 1; // empty title: VLC's hidden/extension windows
        }
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        if title.to_ascii_lowercase().contains("vlc media player") {
            ctx.best = h as isize;
            return 0; // stop enumeration
        }
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut r);
        let area = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
        if area > ctx.biggest_area {
            ctx.biggest_area = area;
            ctx.biggest = h as isize;
        }
        1
    }
}

pub fn find_player() -> isize {
    let pids = vlc_pids();
    if pids.is_empty() {
        return 0;
    }
    let mut ctx = FindCtx { pids: &pids, best: 0, biggest: 0, biggest_area: 0 };
    unsafe {
        EnumWindows(Some(find_player_cb), &mut ctx as *mut FindCtx as LPARAM);
    }
    if ctx.best != 0 { ctx.best } else { ctx.biggest }
}

// ---- state ownership ----------------------------------------------------------------

// Windows recycles HWND values: after VLC dies, the saved handle can belong to another
// app. IsWindow alone would pass and we'd reshape a foreign window; require the owner
// PID recorded at Enter. Old state files (Pid=0) read as stale by design.
fn owns_state(s: &PipState) -> bool {
    unsafe {
        if IsWindow(hw(s.hwnd as isize)) == 0 {
            return false;
        }
        let mut p = 0u32;
        GetWindowThreadProcessId(hw(s.hwnd as isize), &mut p);
        p != 0 && p == s.pid
    }
}

pub fn in_pip() -> bool {
    let path = state::state_path();
    match state::load(&path) {
        None => false,
        Some(s) if !owns_state(&s) => {
            state::try_delete(&path); // stale: VLC gone or hwnd recycled
            false
        }
        Some(_) => true,
    }
}

// ---- enter / exit / toggle ----------------------------------------------------------

fn work_area(h: isize) -> RECT {
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(hw(h), MONITOR_DEFAULTTONEAREST), &mut mi);
        mi.rcWork
    }
}

pub fn enter(h: isize, o: &PipOptions) -> bool {
    if h == 0 || in_pip() {
        return false;
    }
    // gather only - nothing is mutated in this block
    let (r, style, ex, pid) = unsafe {
        if IsIconic(hw(h)) != 0 {
            ShowWindow(hw(h), SW_RESTORE); // else the off-screen iconic rect gets saved as the restore state
        }
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hw(h), &mut r);
        let style = GetWindowLongPtrW(hw(h), GWL_STYLE);
        let ex = GetWindowLongPtrW(hw(h), GWL_EXSTYLE);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hw(h), &mut pid);
        (r, style, ex, pid)
    };
    // save state FIRST, so a failed save can never leave a mutated window with no restore data
    let s = PipState {
        hwnd: h as i64,
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
        style: style as i64,
        ex_style: ex as i64,
        target_w: o.w,
        target_h: o.h,
        corner: o.corner.to_string(),
        margin: o.margin,
        min: o.min,
        pid,
    };
    if state::save(&s, &state::state_path()).is_err() {
        return false; // nothing mutated yet: fail cleanly, retry next toggle
    }
    unsafe {
        // also strip WS_MAXIMIZE: a zoomed window keeps IsZoomed, so Win+Down/Aero would
        // snap the PiP back to Qt's normal placement rect
        SetWindowLongPtrW(hw(h), GWL_STYLE, style & !((WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZE) as isize));
        let wa = work_area(h);
        let (x, y) = geometry::compute_corner(wa.left, wa.top, wa.right, wa.bottom, o.w, o.h, o.corner, o.margin);
        let ok = SetWindowPos(hw(h), HWND_TOPMOST, x, y, o.w, o.h, SWP_FRAMECHANGED | SWP_SHOWWINDOW) != 0;
        if !ok {
            // e.g. UIPI vs elevated VLC: don't claim in-PiP
            SetWindowLongPtrW(hw(h), GWL_STYLE, style);
            state::try_delete(&state::state_path());
        }
        ok
    }
}

pub fn exit_pip() -> bool {
    let path = state::state_path();
    let Some(s) = state::load(&path) else { return false };
    if !owns_state(&s) {
        state::try_delete(&path); // stale: VLC gone or hwnd recycled
        return false;
    }
    let h = s.hwnd as isize;
    unsafe {
        SetWindowRgn(hw(h), std::ptr::null_mut(), 1); // drop the minimal-look clip before restoring
        SetWindowLongPtrW(hw(h), GWL_STYLE, s.style as isize);
        SetWindowLongPtrW(hw(h), GWL_EXSTYLE, s.ex_style as isize);
        // WS_EX_TOPMOST only changes via SetWindowPos: honor the user's own always-on-top
        let after = if s.ex_style & (WS_EX_TOPMOST as i64) != 0 { HWND_TOPMOST } else { HWND_NOTOPMOST };
        let ok = SetWindowPos(hw(h), after, s.x, s.y, s.w, s.h, SWP_FRAMECHANGED | SWP_SHOWWINDOW) != 0;
        if ok || IsWindow(hw(h)) == 0 {
            state::try_delete(&path); // live-window restore failure keeps state so the next toggle retries
        }
        ok
    }
}

pub fn toggle(o: &PipOptions) -> bool {
    if in_pip() { exit_pip() } else { enter(find_player(), o) }
}

// ---- status -------------------------------------------------------------------------

pub fn status_path() -> PathBuf {
    std::env::temp_dir().join("vlc-pip-status.json")
}

pub fn status() -> String {
    let h = find_player();
    if h == 0 {
        return state::status_json(None);
    }
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hw(h), &mut r);
        let style = GetWindowLongPtrW(hw(h), GWL_STYLE);
        let ex = GetWindowLongPtrW(hw(h), GWL_EXSTYLE);
        state::status_json(Some(&StatusInfo {
            hwnd: h as i64,
            x: r.left,
            y: r.top,
            w: r.right - r.left,
            h: r.bottom - r.top,
            caption: style & (WS_CAPTION as isize) == (WS_CAPTION as isize), // BOTH bits
            topmost: ex & (WS_EX_TOPMOST as isize) != 0,
            in_pip: in_pip(),
            minimal: has_region(h),
        }))
    }
}

fn has_region(h: isize) -> bool {
    unsafe {
        let probe = CreateRectRgn(0, 0, 0, 0);
        let r = GetWindowRgn(hw(h), probe) != 0; // 0 = ERROR (no region)
        DeleteObject(probe);
        r
    }
}

// ---- minimal look (Ctrl+H-like) via SetWindowRgn on the video child area -------------
// VLC 3.x hosts the video in a native child whose class starts with "VLC video main".

struct ChildCtx {
    found: isize,
}

unsafe extern "system" fn find_child_cb(c: HWND, l: LPARAM) -> BOOL {
    unsafe {
        let ctx = &mut *(l as *mut ChildCtx);
        if IsWindowVisible(c) == 0 {
            return 1;
        }
        let mut buf = [0u16; 128];
        let n = GetClassNameW(c, buf.as_mut_ptr(), 128);
        if String::from_utf16_lossy(&buf[..n as usize]).starts_with("VLC video main") {
            ctx.found = c as isize;
            return 0;
        }
        1
    }
}

fn find_video_child(top: isize) -> isize {
    let mut ctx = ChildCtx { found: 0 };
    unsafe {
        EnumChildWindows(hw(top), Some(find_child_cb), &mut ctx as *mut ChildCtx as LPARAM);
    }
    ctx.found
}

fn same_rect(a: &RECT, b: &RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

// Cross-tick measurement memory for the stability debounce; v1 kept these in statics.
pub struct RegionTracker {
    prev_win: RECT,
    prev_child: RECT,
    have_prev: bool,
}

impl RegionTracker {
    pub fn new() -> Self {
        unsafe { Self { prev_win: std::mem::zeroed(), prev_child: std::mem::zeroed(), have_prev: false } }
    }
}

impl Default for RegionTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Converging per-tick maintenance, called by the daemon timer (and one-shot enter):
/// no video -> clear region; video child not yet at target size -> resize window with
/// chrome compensation; child at target -> clip window to the video area. Geometry
/// targets come from the state file (recorded at Enter), so daemon and one-shot agree.
/// Acts only on STABLE frames (window+child rects unchanged since the previous tick):
/// VLC re-fits the child asynchronously after our resize, so a fresh measurement can be
/// stale and yield garbage chrome (observed in v1: perpetual resize thrash).
pub fn maintain_region(t: &mut RegionTracker) {
    let path = state::state_path();
    let Some(s) = state::load(&path) else {
        t.have_prev = false;
        return;
    };
    if !owns_state(&s) {
        t.have_prev = false;
        state::try_delete(&path); // stale: VLC gone or hwnd recycled
        return;
    }
    if !s.min {
        return;
    }
    let h = s.hwnd as isize;

    let child = find_video_child(h);
    unsafe {
        if child == 0 {
            t.have_prev = false;
            if has_region(h) {
                SetWindowRgn(hw(h), std::ptr::null_mut(), 1); // playback stopped: show full mini UI
            }
            return;
        }

        let mut wr: RECT = std::mem::zeroed();
        let mut cr: RECT = std::mem::zeroed();
        GetWindowRect(hw(h), &mut wr);
        GetWindowRect(hw(child), &mut cr);
        let stable = t.have_prev && same_rect(&wr, &t.prev_win) && same_rect(&cr, &t.prev_child);
        t.prev_win = wr;
        t.prev_child = cr;
        t.have_prev = true;
        if !stable {
            return; // wait until VLC's re-layout settles
        }

        match plan_region(&wr, &cr, s.target_w, s.target_h, &s.corner, s.margin, || work_area(h)) {
            RegionPlan::Skip => {}
            RegionPlan::Resize { x, y, w, h: th } => {
                SetWindowPos(hw(h), HWND_TOPMOST, x, y, w, th, SWP_FRAMECHANGED);
                t.have_prev = false; // our own resize invalidates the measurement
            }
            RegionPlan::Clip { left, top, right, bottom } => {
                if !has_region(h) {
                    let rgn = CreateRectRgn(left, top, right, bottom);
                    if SetWindowRgn(hw(h), rgn, 1) == 0 {
                        DeleteObject(rgn); // system owns rgn only on success
                    }
                }
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum RegionPlan {
    Skip,
    Resize { x: i32, y: i32, w: i32, h: i32 },
    Clip { left: i32, top: i32, right: i32, bottom: i32 },
}

// Pure planning math for the minimal-look convergence (v1 gotcha #8 lives here): given a
// STABLE window+child measurement, either resize the window (child not at target size:
// grow by chrome so the video itself is exactly target WxH, positioned so the CHILD lands
// at the corner) or clip to the child area. `work` is lazy - it costs two user32 calls
// and only the resize branch needs it.
fn plan_region(
    wr: &RECT, cr: &RECT, target_w: i32, target_h: i32, corner: &str, margin: i32,
    work: impl FnOnce() -> RECT,
) -> RegionPlan {
    let rel_l = cr.left - wr.left;
    let rel_t = cr.top - wr.top;
    let cw = cr.right - cr.left;
    let ch = cr.bottom - cr.top;
    let chrome_w = (wr.right - wr.left) - cw;
    let chrome_h = (wr.bottom - wr.top) - ch;
    // real chrome (menu + controller + borders) is well under 300px; negative or huge
    // delta = stale rects from VLC's async re-layout
    if !(0..=300).contains(&chrome_w) || !(0..=300).contains(&chrome_h) {
        return RegionPlan::Skip;
    }
    if (cw - target_w).abs() > 2 || (ch - target_h).abs() > 2 {
        let wa = work();
        let (vx, vy) =
            geometry::compute_corner(wa.left, wa.top, wa.right, wa.bottom, target_w, target_h, corner, margin);
        let (tw, th, tx, ty) = (target_w + chrome_w, target_h + chrome_h, vx - rel_l, vy - rel_t);
        if tw <= 0 || th <= 0 {
            return RegionPlan::Skip; // hostile/garbage state values: do nothing
        }
        if wr.left == tx && wr.top == ty && wr.right - wr.left == tw && wr.bottom - wr.top == th {
            return RegionPlan::Skip; // defensive (v1 parity): never issue a no-op SetWindowPos
        }
        return RegionPlan::Resize { x: tx, y: ty, w: tw, h: th };
    }
    RegionPlan::Clip { left: rel_l, top: rel_t, right: rel_l + cw, bottom: rel_t + ch }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: r, bottom: b }
    }

    fn work() -> RECT {
        rect(0, 0, 1920, 1040) // 480x270 br margin 16 => video corner at (1424, 754)
    }

    #[test]
    fn negative_chrome_is_stale_measurement() {
        // child wider than its own window = mid-relayout garbage
        let plan = plan_region(&rect(0, 0, 480, 270), &rect(0, 0, 481, 270), 480, 270, "br", 16, work);
        assert_eq!(plan, RegionPlan::Skip);
    }

    #[test]
    fn chrome_clamp_boundary_300_ok_301_stale() {
        // child at target, chrome_h exactly 300 -> clip; 301 -> stale
        let cr = rect(0, 0, 480, 270);
        let ok = plan_region(&rect(0, 0, 480, 570), &cr, 480, 270, "br", 16, work);
        assert_eq!(ok, RegionPlan::Clip { left: 0, top: 0, right: 480, bottom: 270 });
        let stale = plan_region(&rect(0, 0, 480, 571), &cr, 480, 270, "br", 16, work);
        assert_eq!(stale, RegionPlan::Skip);
    }

    #[test]
    fn two_px_tolerance_clips_three_resizes() {
        // 482 wide child (diff 2) counts as converged; 483 (diff 3) does not
        let at_2 = plan_region(&rect(0, 0, 482, 270), &rect(0, 0, 482, 270), 480, 270, "br", 16, work);
        assert!(matches!(at_2, RegionPlan::Clip { .. }));
        let at_3 = plan_region(&rect(0, 0, 483, 270), &rect(0, 0, 483, 270), 480, 270, "br", 16, work);
        assert!(matches!(at_3, RegionPlan::Resize { .. }));
    }

    #[test]
    fn resize_grows_by_chrome_and_lands_child_at_corner() {
        // window 420x360 at (100,100); child 400x225 at rel (10,30) => chrome 20x135
        let plan = plan_region(&rect(100, 100, 520, 460), &rect(110, 130, 510, 355), 480, 270, "br", 16, work);
        // target 480x270 + chrome => 500x405, positioned so the CHILD hits (1424,754)
        assert_eq!(plan, RegionPlan::Resize { x: 1414, y: 724, w: 500, h: 405 });
    }

    #[test]
    fn clip_is_child_rect_relative_to_window() {
        let plan = plan_region(&rect(1424, 700, 1904, 1024), &rect(1424, 754, 1904, 1024), 480, 270, "br", 16, work);
        assert_eq!(plan, RegionPlan::Clip { left: 0, top: 54, right: 480, bottom: 324 });
    }

    #[test]
    fn hostile_negative_target_skips() {
        // a hand-crafted state file with TargetW=-500 must not produce a resize
        let plan = plan_region(&rect(0, 0, 480, 300), &rect(0, 20, 480, 290), -500, 270, "br", 16, work);
        assert_eq!(plan, RegionPlan::Skip);
    }
}
