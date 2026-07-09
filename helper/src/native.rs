use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

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
pub(crate) fn owns_state(s: &PipState) -> bool {
    unsafe {
        if IsWindow(hw(s.hwnd as isize)) == 0 {
            return false;
        }
        let mut p = 0u32;
        GetWindowThreadProcessId(hw(s.hwnd as isize), &mut p);
        p != 0 && p == s.pid
    }
}

// Read-only: a stale state here may be a pending reopen-heal record whose lifecycle
// belongs to maintain_region - a mere status query must not destroy it.
pub fn in_pip() -> bool {
    state::load(&state::state_path()).is_some_and(|s| owns_state(&s))
}

// ---- enter / exit / toggle ----------------------------------------------------------

pub fn work_area(h: isize) -> geometry::Rect {
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(hw(h), MONITOR_DEFAULTTONEAREST), &mut mi);
        let w = mi.rcWork;
        geometry::Rect { left: w.left, top: w.top, right: w.right, bottom: w.bottom }
    }
}

// ---- fullscreen-origin PiP -------------------------------------------------------------
// Entering PiP from a fullscreen VLC reshapes IMMEDIATELY - the PiP appears at the
// keypress. VLC's internal fullscreen state stays ON for the whole PiP session:
// clearing it first (Esc, then wait for Qt's windowed restore) cost the user ~0.5-1s of
// blank screen, and the reverse order desyncs Qt - it only restores its windowed
// geometry from an UNTOUCHED fullscreen window (reshape-then-Esc left a captionless
// window at the PiP rect, verified live). Exit restores the saved fullscreen style +
// rect verbatim: the user came from fullscreen, they get fullscreen back, and VLC's
// internal state matches its window again. Two side effects are managed meanwhile:
// the fullscreen controller strip would pop up over the desktop on hover (hidden every
// tick), and Esc/F reaching a focused PiP would make Qt leave fullscreen underneath
// the reshape (swallowed by the keyboard hook).

/// Was this PiP taken from a fullscreen VLC? The saved pre-PiP style tells (caption
/// fully absent). Drives the Esc swallow, the strip hiding, and the heal skip.
pub fn fs_origin(style: i64) -> bool {
    style as isize & WS_CAPTION as isize != WS_CAPTION as isize
}

/// Owner PID (0 when the window is gone).
pub fn window_owner(h: isize) -> u32 {
    let mut p = 0u32;
    unsafe {
        GetWindowThreadProcessId(hw(h), &mut p);
    }
    p
}

/// Restore before enter() snapshots anything: the off-screen iconic rect must never
/// become the restore state.
pub fn restore_if_iconic(h: isize) -> bool {
    unsafe {
        if IsIconic(hw(h)) != 0 {
            ShowWindow(hw(h), SW_RESTORE);
            return true;
        }
        false
    }
}

struct FscTargets {
    pid: u32,
    found: Vec<isize>,
}

unsafe extern "system" fn collect_fsc_cb(w: HWND, l: LPARAM) -> BOOL {
    unsafe {
        let ctx = &mut *(l as *mut FscTargets);
        let mut buf = [0u16; 128];
        let n = GetClassNameW(w, buf.as_mut_ptr(), 128);
        if String::from_utf16_lossy(&buf[..n as usize]).starts_with("Qt5QWindowToolSaveBits") {
            let mut p = 0u32;
            GetWindowThreadProcessId(w, &mut p);
            if p == ctx.pid && IsWindowVisible(w) != 0 {
                ctx.found.push(w as isize);
            }
        }
        1
    }
}

/// Hide VLC's fullscreen controller strip - a separate topmost Qt window (class prefix
/// "Qt5QWindowToolSaveBits") parked at the screen bottom, shown on hover while VLC
/// believes it is fullscreen. One hide sticks across hovers (Qt's visibility cache
/// desyncs from the OS state, verified live), but VLC's own hide timer can resync it,
/// so this runs every tick while a fullscreen-origin PiP is active - worst case the
/// strip blinks for one tick. After exit back to fullscreen, VLC's next hover/hide
/// cycle brings the strip back naturally.
pub fn hide_fs_controller(pid: u32) {
    if pid == 0 {
        return;
    }
    let mut ctx = FscTargets { pid, found: Vec::new() };
    unsafe {
        EnumWindows(Some(collect_fsc_cb), &mut ctx as *mut FscTargets as LPARAM);
        for f in ctx.found {
            ShowWindow(hw(f), SW_HIDE);
        }
    }
}

// ---- drag gesture primitives (hook arms, pump applies) --------------------------------

pub fn window_rect(h: isize) -> Option<geometry::Rect> {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hw(h), &mut r) == 0 {
            return None;
        }
        Some(geometry::Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom })
    }
}

// The minimal-look region clips painting AND hit-testing, so the gesture surface is the
// region box (offset to screen coords by the window origin), not the window rect.
pub fn visible_rect(h: isize) -> Option<geometry::Rect> {
    let wr = window_rect(h)?;
    Some(region_box(h).map_or(wr, |(l, t, r, b)| geometry::Rect {
        left: wr.left + l,
        top: wr.top + t,
        right: wr.left + r,
        bottom: wr.top + b,
    }))
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
                set_region(h, c.left, c.top, c.right, c.bottom);
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

/// Adopt the drag result: Corner = nearest as of `fin`; a resize also adopts the new video
/// size (`fin` minus chrome measured at drag start). State first, then config. Finalizes
/// from the CALLER's computed rect - the final async SetWindowPos may not have landed in
/// VLC yet, so a fresh GetWindowRect would read stale.
pub fn finish_drag(fin: &geometry::Rect, resized: bool, chrome_w: i32, chrome_h: i32) {
    let path = state::state_path();
    let Some(mut s) = state::load(&path) else { return };
    if !owns_state(&s) {
        return; // VLC died mid-drag: next tick's maintain_region cleans up
    }
    let work = work_area(s.hwnd as isize);
    s.corner = geometry::nearest_corner(fin, &work);
    if resized {
        let (tw, th) = (fin.right - fin.left - chrome_w, fin.bottom - fin.top - chrome_h);
        if tw > 0 && th > 0 {
            s.target_w = tw;
            s.target_h = th;
        }
    }
    // failures swallowed (SPEC 12): the gesture already holds on screen
    let _ = state::save(&s, &path);
    crate::options::save_config(s.target_w, s.target_h, s.corner);
}

// Client-relative chrome around the video child (menu above, controller below). These are
// Qt widgets in the CLIENT area, so the offsets survive the border strip and predict where
// the child lands after the PiP resize. None when not playing or mid-relayout garbage.
fn client_chrome(h: isize) -> Option<(i32, i32, i32, i32)> {
    let child = find_video_child(h);
    if child == 0 {
        return None;
    }
    unsafe {
        let mut client: RECT = std::mem::zeroed();
        let mut origin = POINT { x: 0, y: 0 };
        let mut cr: RECT = std::mem::zeroed();
        if GetClientRect(hw(h), &mut client) == 0
            || ClientToScreen(hw(h), &mut origin) == 0
            || GetWindowRect(hw(child), &mut cr) == 0
        {
            return None;
        }
        let l = cr.left - origin.x;
        let t = cr.top - origin.y;
        let r = (origin.x + client.right) - cr.right;
        let b = (origin.y + client.bottom) - cr.bottom;
        // same sanity envelope as plan_region (per-AXIS sums): anything outside is a
        // stale measurement, and a rect the converger would forever Skip must never land
        if l >= 0 && t >= 0 && r >= 0 && b >= 0 && (0..=300).contains(&(l + r)) && (0..=300).contains(&(t + b)) {
            Some((l, t, r, b))
        } else {
            None
        }
    }
}

pub fn enter(h: isize, o: &PipOptions) -> bool {
    if h == 0 || in_pip() {
        return false;
    }
    // losslessly reversible; else the off-screen iconic rect gets saved as the restore state
    restore_if_iconic(h);
    let (r, style, ex) = unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hw(h), &mut r);
        let style = GetWindowLongPtrW(hw(h), GWL_STYLE);
        let ex = GetWindowLongPtrW(hw(h), GWL_EXSTYLE);
        (r, style, ex)
    };
    let pid = window_owner(h);
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
        corner: o.corner,
        margin: o.margin,
        min: o.min,
        pid,
    };
    if state::save(&s, &state::state_path()).is_err() {
        return false; // nothing mutated yet: fail cleanly, retry next toggle
    }
    // measured pre-strip: with it, enter lands in ONE SetWindowPos at the final
    // chrome-compensated rect with the region applied immediately - no visible
    // grow-then-clip pass from the converger (it only verifies afterwards)
    let chrome = if o.min { client_chrome(h) } else { None };
    if fs_origin(style as i64) {
        // the strip is likely on screen RIGHT NOW (the user was just hovering the
        // fullscreen video): gone before the PiP lands, not a tick later. One hide
        // sticks; the daemon tick only re-hides after VLC's own show/hide resync.
        hide_fs_controller(pid);
    }
    unsafe {
        // also strip WS_MAXIMIZE: a zoomed window keeps IsZoomed, so Win+Down/Aero would
        // snap the PiP back to Qt's normal placement rect
        SetWindowLongPtrW(hw(h), GWL_STYLE, style & !((WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZE) as isize));
        let wa = work_area(h);
        let (vx, vy) = geometry::compute_corner(&wa, o.w, o.h, o.corner, o.margin);
        let (x, y, tw, th) = match chrome {
            Some((cl, ct, cr, cb)) => (vx - cl, vy - ct, o.w + cl + cr, o.h + ct + cb),
            None => (vx, vy, o.w, o.h), // not playing: converger takes over once a child exists
        };
        let ok = SetWindowPos(hw(h), HWND_TOPMOST, x, y, tw, th, SWP_FRAMECHANGED | SWP_SHOWWINDOW) != 0;
        if ok {
            if let Some((cl, ct, _, _)) = chrome {
                set_region(h, cl, ct, cl + o.w, ct + o.h);
            }
        } else {
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
    state::temp_path("vlc-pip-status.json")
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

fn region_box(h: isize) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let probe = CreateRectRgn(0, 0, 0, 0);
        let mut b: RECT = std::mem::zeroed();
        let r = if GetWindowRgn(hw(h), probe) != 0 && GetRgnBox(probe, &mut b) > NULLREGION {
            Some((b.left, b.top, b.right, b.bottom))
        } else {
            None
        };
        DeleteObject(probe);
        r
    }
}

// Apply a rectangular region (window-relative); the system owns rgn only on success.
fn set_region(h: isize, left: i32, top: i32, right: i32, bottom: i32) {
    unsafe {
        let rgn = CreateRectRgn(left, top, right, bottom);
        if SetWindowRgn(hw(h), rgn, 1) == 0 {
            DeleteObject(rgn);
        }
    }
}

// ---- minimal look (Ctrl+H-like) via SetWindowRgn on the video child area -------------
// VLC 3.x hosts the video in a native child whose class starts with "VLC video main".

unsafe extern "system" fn find_child_cb(c: HWND, l: LPARAM) -> BOOL {
    unsafe {
        let found = &mut *(l as *mut isize);
        if IsWindowVisible(c) == 0 {
            return 1;
        }
        let mut buf = [0u16; 128];
        let n = GetClassNameW(c, buf.as_mut_ptr(), 128);
        if String::from_utf16_lossy(&buf[..n as usize]).starts_with("VLC video main") {
            *found = c as isize;
            return 0;
        }
        1
    }
}

fn find_video_child(top: isize) -> isize {
    let mut found = 0isize;
    unsafe {
        EnumChildWindows(hw(top), Some(find_child_cb), &mut found as *mut isize as LPARAM);
    }
    found
}

fn same_rect(a: &RECT, b: &RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

// Cross-tick measurement memory for the stability debounce; v1 kept these in statics.
// fs_prev/fs_have: the fullscreen-origin dissolve watch's baseline - the window rect
// last seen WITH a live video child.
pub struct RegionTracker {
    prev_win: RECT,
    prev_child: RECT,
    have_prev: bool,
    fs_prev: RECT,
    fs_have: bool,
}

impl Default for RegionTracker {
    // manual impl: windows-sys RECT has no Default
    fn default() -> Self {
        unsafe {
            Self {
                prev_win: std::mem::zeroed(),
                prev_child: std::mem::zeroed(),
                have_prev: false,
                fs_prev: std::mem::zeroed(),
                fs_have: false,
            }
        }
    }
}

/// Qt left fullscreen UNDERNEATH a fullscreen-origin PiP. Media end and stop trigger
/// Qt's own leave-fullscreen re-layout - the window balloons to Qt's idea of windowed
/// geometry, no input involved (verified live: within ~one tick of the vout dying).
/// The PiP session dissolves: give the window its frame back at Qt's chosen rect and
/// drop the state. Stock VLC lands windowed after fullscreen playback ends too, and the
/// saved fullscreen rect must never be restored onto an internally windowed VLC.
fn dissolve_fs_pip(s: &PipState, path: &Path) {
    let h = s.hwnd as isize;
    unsafe {
        SetWindowRgn(hw(h), std::ptr::null_mut(), 1);
        SetWindowLongPtrW(
            hw(h),
            GWL_STYLE,
            s.style as isize | (WS_CAPTION | WS_THICKFRAME) as isize,
        );
        SetWindowLongPtrW(hw(h), GWL_EXSTYLE, s.ex_style as isize);
        let after = if s.ex_style & (WS_EX_TOPMOST as i64) != 0 { HWND_TOPMOST } else { HWND_NOTOPMOST };
        SetWindowPos(hw(h), after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED);
    }
    state::try_delete(path);
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
        heal_reopened(&s, &path);
        return;
    }
    HEAL_TRIES.store(0, Relaxed); // a live owned PiP ends any interrupted heal cleanly
    let h = s.hwnd as isize;
    let child = find_video_child(h);

    // fullscreen-origin dissolve watch - BEFORE the min gate, it guards every fs session
    if fs_origin(s.style) {
        let mut wr: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(hw(h), &mut wr) };
        if child != 0 {
            // baseline: the rect while video is alive (our reshapes and drags included)
            t.fs_prev = wr;
            t.fs_have = true;
        } else if t.fs_have && !same_rect(&wr, &t.fs_prev) {
            dissolve_fs_pip(&s, &path);
            t.have_prev = false;
            t.fs_have = false;
            return;
        }
    } else {
        t.fs_have = false;
    }

    if !s.min {
        return;
    }

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

        match plan_region(&wr, &cr, s.target_w, s.target_h, s.corner, s.margin, || work_area(h)) {
            RegionPlan::Skip => {}
            RegionPlan::Resize { x, y, w, h: th } => {
                SetWindowPos(hw(h), HWND_TOPMOST, x, y, w, th, SWP_FRAMECHANGED);
                t.have_prev = false; // our own resize invalidates the measurement
            }
            RegionPlan::Clip { left, top, right, bottom } => {
                // verify the box, not just presence: a live-clipped resize drag leaves an
                // approximate region that convergence must confirm or correct
                if region_box(h) != Some((left, top, right, bottom)) {
                    set_region(h, left, top, right, bottom);
                }
            }
        }
    }
}

// Bounded so an unhealable window (e.g. elevated VLC: UIPI silently swallows the
// SetWindowPos) is never fought forever: ~6s of ticks, then the record is dropped.
static HEAL_TRIES: AtomicU32 = AtomicU32::new(0);

/// VLC that closes while in PiP persists the PiP geometry as its own (Qt saves on exit),
/// so its next launch opens full-size at the PiP origin, overflowing the screen. The
/// stale state file is kept as a pending-restore record; when a new player window
/// appears, apply the saved pre-PiP rect and delete the record only once the rect is
/// observed to stick - VLC's own startup positioning must not win the race.
fn heal_reopened(s: &PipState, path: &Path) {
    if s.w <= 0 || s.h <= 0 || s.pid == 0 {
        state::try_delete(path); // garbage record (pid is 0 if VLC died mid-enter): not healable
        return;
    }
    if fs_origin(s.style) {
        // a fullscreen-origin PiP's record holds the FULLSCREEN rect - never heal a
        // reopened window to it. Qt believed fullscreen the whole session, so it
        // persisted its true windowed geometry itself: nothing to heal.
        state::try_delete(path);
        return;
    }
    let pids = vlc_pids();
    if pids.contains(&s.pid) {
        state::try_delete(path); // the recorded VLC still runs (hwnd recycled): not a close-in-PiP
        return;
    }
    if pids.is_empty() {
        return; // VLC not back yet: keep waiting (one process snapshot per tick)
    }
    let h2 = find_player();
    if h2 == 0 {
        return;
    }
    unsafe {
        if IsIconic(hw(h2)) != 0 {
            return; // heal the normal placement once restored - the iconic rect is garbage
        }
        let target = RECT { left: s.x, top: s.y, right: s.x + s.w, bottom: s.y + s.h };
        if MonitorFromRect(&target, MONITOR_DEFAULTTONULL).is_null() {
            state::try_delete(path); // monitor layout changed: VLC's own placement is saner
            return;
        }
        let mut wr: RECT = std::mem::zeroed();
        GetWindowRect(hw(h2), &mut wr);
        if same_rect(&wr, &target) {
            HEAL_TRIES.store(0, Relaxed);
            state::try_delete(path); // heal landed and stuck: done
            return;
        }
        if HEAL_TRIES.fetch_add(1, Relaxed) >= 40 {
            HEAL_TRIES.store(0, Relaxed);
            state::try_delete(path); // not converging: stop fighting the window
            return;
        }
        SetWindowPos(hw(h2), std::ptr::null_mut(), s.x, s.y, s.w, s.h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RegionPlan {
    Skip,
    Resize { x: i32, y: i32, w: i32, h: i32 },
    Clip { left: i32, top: i32, right: i32, bottom: i32 },
}

// Pure planning math for the minimal-look convergence (v1 gotcha #8 lives here): given a
// STABLE window+child measurement, either resize the window (child not at target size:
// grow by chrome so the video itself is exactly target WxH, positioned so the CHILD lands
// at the corner) or clip to the child area. `work` is lazy - it costs two user32 calls
// and only the resize branch needs it.
pub(crate) fn plan_region(
    wr: &RECT, cr: &RECT, target_w: i32, target_h: i32, corner: geometry::Corner, margin: i32,
    work: impl FnOnce() -> geometry::Rect,
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
        let (vx, vy) = geometry::compute_corner(&wa, target_w, target_h, corner, margin);
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
