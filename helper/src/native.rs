use std::path::Path;

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, POINT, RECT, WAIT_ABANDONED,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, CreateRectRgn, DeleteObject, GetMonitorInfoW, GetRgnBox, GetWindowRgn,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromRect,
    MonitorFromWindow, NULLREGION, SetWindowRgn,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetClientRect,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, HWND_NOTOPMOST,
    HWND_TOPMOST, IsIconic, IsWindow, IsWindowVisible, SW_HIDE, SW_RESTORE, SWP_ASYNCWINDOWPOS,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSENDCHANGING, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_CAPTION, WS_EX_TOPMOST,
    WS_MAXIMIZE, WS_THICKFRAME,
};
use windows_sys::core::{BOOL, w};

use crate::geometry;
use crate::options::PipOptions;
use crate::state::{self, PipState, StatusInfo};

// Handles live in statics and the state file, so they travel as isize (windows-sys 0.61
// handles are *mut c_void: not Send/Sync). Cast at the call boundary only.
fn hw(h: isize) -> HWND {
    h as HWND
}

pub(crate) struct TransitionGuard(HANDLE);

impl TransitionGuard {
    pub(crate) fn acquire() -> Option<Self> {
        Self::wait(INFINITE).ok().flatten()
    }

    pub(crate) fn wait(timeout_ms: u32) -> Result<Option<Self>, ()> {
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, w!("VlcPipTransition")) };
        if handle.is_null() {
            return Err(());
        }
        match unsafe { WaitForSingleObject(handle, timeout_ms) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(Self(handle))),
            WAIT_TIMEOUT => {
                unsafe { CloseHandle(handle) };
                Ok(None)
            }
            _ => {
                unsafe { CloseHandle(handle) };
                Err(())
            }
        }
    }
}

impl Drop for TransitionGuard {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
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
                let len = e
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(e.szExeFile.len());
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

fn find_player_for_pids(pids: &[u32]) -> isize {
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

pub fn find_player() -> isize {
    find_player_for_pids(&vlc_pids())
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

// ---- enter / exit / toggle ----------------------------------------------------------

pub fn work_area(h: isize) -> geometry::Rect {
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(hw(h), MONITOR_DEFAULTTONEAREST), &mut mi);
        let w = mi.rcWork;
        geometry::Rect {
            left: w.left,
            top: w.top,
            right: w.right,
            bottom: w.bottom,
        }
    }
}

// ---- fullscreen-origin PiP -----------------------------------------------------------
// PiP from a fullscreen VLC is the same instant reshape; VLC's internal fullscreen state
// stays ON for the whole session (Qt only restores its windowed geometry from an
// UNTOUCHED fullscreen window - leaving fullscreen first or after desyncs it, SPEC 7).
// Exit restores the saved fullscreen style+rect verbatim. Meanwhile the controller strip
// keeps its empty-region veil and the keyboard hook swallows Esc/F.

/// Was this PiP taken from a fullscreen VLC? The saved pre-PiP style tells (the complete
/// two-bit caption mask is not present). Drives the Esc swallow, controller veil, and heal skip.
pub fn fs_origin(style: isize) -> bool {
    style & WS_CAPTION as isize != WS_CAPTION as isize
}

pub fn window_owner(h: isize) -> u32 {
    let mut p = 0u32;
    unsafe {
        GetWindowThreadProcessId(hw(h), &mut p);
    }
    p
}

fn for_each_fs_controller(pid: u32, mut f: impl FnMut(HWND)) {
    if pid == 0 {
        return;
    }
    enum_windows(|w| {
        if window_owner(w as isize) == pid && class_starts_with(w, "Qt5QWindowToolSaveBits") {
            f(w);
        }
        true
    });
}

fn is_veiled(w: HWND) -> bool {
    unsafe {
        let probe = CreateRectRgn(0, 0, 0, 0);
        if probe.is_null() {
            return false;
        }
        let veiled = GetWindowRgn(w, probe) == NULLREGION;
        DeleteObject(probe);
        veiled
    }
}

/// Apply a persistent empty region to every controller, including hidden strips.
pub fn veil_fs_controller(pid: u32) {
    for_each_fs_controller(pid, |w| unsafe {
        if is_veiled(w) {
            return;
        }
        let empty = CreateRectRgn(0, 0, 0, 0);
        if empty.is_null() {
            return;
        }
        if SetWindowRgn(w, empty, 1) == 0 {
            DeleteObject(empty);
        }
    });
}

/// Remove only our persistent empty-region veil. VLC decides when the controller is
/// shown again, normally on its next fullscreen hover cycle.
pub fn unveil_fs_controller(pid: u32) {
    for_each_fs_controller(pid, |w| unsafe {
        if is_veiled(w) {
            SetWindowRgn(w, std::ptr::null_mut(), 1);
        }
    });
}

fn unveil_if_fs(s: &PipState) {
    if fs_origin(s.style) {
        unveil_fs_controller(s.pid);
    }
}

fn drop_state(s: &PipState, path: &Path) -> bool {
    unveil_if_fs(s);
    state::try_delete(path)
}

// ---- window / region primitives -------------------------------------------------------
// VLC 3.x hosts the video in a native child whose class starts with "VLC video main";
// the minimal look clips the top-level window to it via SetWindowRgn.

pub fn window_rect(h: isize) -> Option<geometry::Rect> {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hw(h), &mut r) == 0 {
            return None;
        }
        Some(geometry::Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        })
    }
}

fn styles(h: isize) -> (isize, isize) {
    unsafe {
        (
            GetWindowLongPtrW(hw(h), GWL_STYLE),
            GetWindowLongPtrW(hw(h), GWL_EXSTYLE),
        )
    }
}

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

// Every region set on the main VLC window has a nonempty box, so presence == nonempty box.
fn has_region(h: isize) -> bool {
    region_box(h).is_some()
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
        if rgn.is_null() {
            return;
        }
        if SetWindowRgn(hw(h), rgn, 1) == 0 {
            DeleteObject(rgn);
        }
    }
}

// ---- drag gesture primitives (hook arms, pump applies) --------------------------------

// The minimal-look region clips painting AND hit-testing, so the gesture surface is the
// region box (offset to screen coords by the window origin), not the window rect. One
// call returns (visible, window) so region presence comes from one coherent snapshot.
pub fn gesture_rects(h: isize) -> Option<(geometry::Rect, geometry::Rect)> {
    let wr = window_rect(h)?;
    let vis = region_box(h).map_or(wr, |(l, t, r, b)| geometry::Rect {
        left: wr.left + l,
        top: wr.top + t,
        right: wr.left + r,
        bottom: wr.top + b,
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
            hw(h),
            std::ptr::null_mut(),
            r.left,
            r.top,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | SWP_NOSENDCHANGING,
        );
    }
}

pub fn drag_resize(h: isize, r: &geometry::Rect, clip: Option<&geometry::Rect>) {
    unsafe {
        match clip {
            Some(c) => {
                set_region(h, c.left, c.top, c.right, c.bottom);
            }
            None => {
                if has_region(h) {
                    SetWindowRgn(hw(h), std::ptr::null_mut(), 1); // no clip context: show it all
                }
            }
        }
        SetWindowPos(
            hw(h),
            std::ptr::null_mut(),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | SWP_NOSENDCHANGING,
        );
    }
}

/// Adopt the drag result: corner = nearest as of `fin`; a resize also adopts the new
/// video size (`fin` minus chrome measured at drag start). Works from the CALLER's
/// computed rect - the final async SetWindowPos may not have landed in VLC yet.
pub fn finish_drag(fin: &geometry::Rect, resized: bool, chrome_w: i32, chrome_h: i32) {
    let path = state::state_path();
    let Some(mut s) = state::load(&path) else {
        return;
    };
    if !owns_state(&s) {
        return; // VLC died mid-drag: next tick's maintain_region cleans up
    }
    let work = work_area(s.hwnd);
    s.corner = geometry::nearest_corner(fin, &work);
    if resized {
        let (tw, th) = (
            fin.right - fin.left - chrome_w,
            fin.bottom - fin.top - chrome_h,
        );
        if tw > 0 && th > 0 {
            s.target_w = tw;
            s.target_h = th;
        }
    }
    let _ = state::save(&s, &path); // failure swallowed: the gesture already holds on screen
    crate::options::save_config(s.target_w, s.target_h, s.corner);
}

// Client-relative chrome around the video child (menu above, controller below): Qt
// widgets in the CLIENT area, so the offsets survive the border strip and predict where
// the child lands after the reshape. None when not playing or mid-relayout garbage.
fn client_chrome(h: isize) -> Option<(i32, i32, i32, i32)> {
    let child = find_video_child(h);
    if child == 0 {
        return None;
    }
    let cr = window_rect(child)?;
    unsafe {
        let mut client: RECT = std::mem::zeroed();
        let mut origin = POINT { x: 0, y: 0 };
        if GetClientRect(hw(h), &mut client) == 0 || ClientToScreen(hw(h), &mut origin) == 0 {
            return None;
        }
        let l = cr.left.checked_sub(origin.x)?;
        let t = cr.top.checked_sub(origin.y)?;
        let r = origin.x.checked_add(client.right)?.checked_sub(cr.right)?;
        let b = origin
            .y
            .checked_add(client.bottom)?
            .checked_sub(cr.bottom)?;
        let chrome_w = l.checked_add(r)?;
        let chrome_h = t.checked_add(b)?;
        // same sanity envelope as plan_region (per-AXIS sums): anything outside is a
        // stale measurement, and a rect the converger would forever Skip must never land
        if l >= 0
            && t >= 0
            && r >= 0
            && b >= 0
            && (0..=geometry::MAX_CHROME).contains(&chrome_w)
            && (0..=geometry::MAX_CHROME).contains(&chrome_h)
        {
            Some((l, t, r, b))
        } else {
            None
        }
    }
}

pub fn enter(h: isize, o: &PipOptions) -> bool {
    if h == 0 {
        return false;
    }
    let path = state::state_path();
    let previous = state::load(&path);
    if previous.as_ref().is_some_and(owns_state) {
        return false;
    }
    if o.w <= 0 || o.h <= 0 {
        return false;
    }

    // restore FIRST: the off-screen iconic rect must never become the restore state
    if unsafe { IsIconic(hw(h)) } != 0 {
        unsafe { ShowWindow(hw(h), SW_RESTORE) };
    }

    // With the restored geometry available, validate the complete landing before
    // writing restoration state or applying any PiP mutation.
    let Some((vx, vy)) = geometry::compute_corner(&work_area(h), o.w, o.h, o.corner, o.margin)
    else {
        return false;
    };
    let chrome = if o.min { client_chrome(h) } else { None };
    let (x, y, tw, th, clip) = match chrome {
        Some((cl, ct, cr, cb)) => {
            let Some(x) = vx.checked_sub(cl) else {
                return false;
            };
            let Some(y) = vy.checked_sub(ct) else {
                return false;
            };
            let Some(chrome_w) = cl.checked_add(cr) else {
                return false;
            };
            let Some(chrome_h) = ct.checked_add(cb) else {
                return false;
            };
            let Some(tw) = o.w.checked_add(chrome_w) else {
                return false;
            };
            let Some(th) = o.h.checked_add(chrome_h) else {
                return false;
            };
            let Some(right) = cl.checked_add(o.w) else {
                return false;
            };
            let Some(bottom) = ct.checked_add(o.h) else {
                return false;
            };
            (x, y, tw, th, Some((cl, ct, right, bottom)))
        }
        None => (vx, vy, o.w, o.h, None),
    };

    let Some(r) = window_rect(h) else {
        return false;
    };
    let Some(rw) = r.right.checked_sub(r.left) else {
        return false;
    };
    let Some(rh) = r.bottom.checked_sub(r.top) else {
        return false;
    };
    if rw <= 0 || rh <= 0 {
        return false;
    }
    let (style, ex) = styles(h);
    let pid = window_owner(h);
    // Save before PiP mutations, so failure cannot leave PiP changes without restore data.
    let s = PipState {
        hwnd: h,
        x: r.left,
        y: r.top,
        w: rw,
        h: rh,
        style,
        ex_style: ex,
        target_w: o.w,
        target_h: o.h,
        corner: o.corner,
        margin: o.margin,
        min: o.min,
        pid,
    };
    if state::save(&s, &path).is_err() {
        return false; // no PiP mutation yet: fail cleanly, retry next toggle
    }
    if let Some(previous) = previous.as_ref() {
        unveil_if_fs(previous); // a successful save just overwrote this stale record
    }
    if fs_origin(style) {
        // the user was likely just hovering the fullscreen video, so the strip is on
        // screen RIGHT NOW: hide it before the PiP lands, not a tick later
        for_each_fs_controller(pid, |w| {
            if unsafe { IsWindowVisible(w) } != 0 {
                unsafe { ShowWindow(w, SW_HIDE) };
            }
        });
        veil_fs_controller(pid); // the empty region persists across later Qt shows
    }
    unsafe {
        // WS_MAXIMIZE too: a zoomed window keeps IsZoomed, and Aero snap would then
        // bounce the PiP back to Qt's normal placement rect
        SetWindowLongPtrW(
            hw(h),
            GWL_STYLE,
            style & !((WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZE) as isize),
        );
        let ok = SetWindowPos(
            hw(h),
            HWND_TOPMOST,
            x,
            y,
            tw,
            th,
            SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        ) != 0;
        if ok {
            if let Some((left, top, right, bottom)) = clip {
                set_region(h, left, top, right, bottom);
            }
        } else {
            // e.g. UIPI vs elevated VLC: don't claim in-PiP
            SetWindowLongPtrW(hw(h), GWL_STYLE, style);
            let _ = drop_state(&s, &path);
        }
        ok
    }
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
    if ex_style & WS_EX_TOPMOST as isize != 0 {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreResult {
    Restored,
    Failed,
    Pending,
}

impl RestoreResult {
    pub(crate) fn code(self) -> i32 {
        match self {
            Self::Restored => 0,
            Self::Failed => 1,
            Self::Pending => 4,
        }
    }
}

fn restore_state(s: &PipState, path: &Path, drop_gone: bool) -> bool {
    let h = s.hwnd;
    let after = restore_frame(h, s.style, s.ex_style);
    unsafe {
        if SetWindowPos(
            hw(h),
            after,
            s.x,
            s.y,
            s.w,
            s.h,
            SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        ) != 0
        {
            return drop_state(s, path);
        }
        if drop_gone && IsWindow(hw(h)) == 0 {
            let _ = drop_state(s, path);
        }
    }
    false
}

pub fn exit_pip() -> bool {
    let path = state::state_path();
    let Some(s) = state::load(&path) else {
        return false;
    };
    if !owns_state(&s) {
        let _ = drop_state(&s, &path); // stale: VLC gone or hwnd recycled
        return false;
    }
    restore_state(&s, &path, true)
}

/// Installer/uninstaller restore: an unowned record is pending heal, never stale cleanup.
pub fn maintenance_restore() -> RestoreResult {
    let path = state::state_path();
    let Some(s) = state::load(&path) else {
        return RestoreResult::Failed;
    };
    if !owns_state(&s) {
        return RestoreResult::Pending;
    }
    if restore_state(&s, &path, false) {
        RestoreResult::Restored
    } else {
        RestoreResult::Failed
    }
}

pub fn toggle(o: &PipOptions) -> bool {
    if in_pip() {
        exit_pip()
    } else {
        enter(find_player(), o)
    }
}

// ---- status -------------------------------------------------------------------------

pub fn status() -> String {
    let h = find_player();
    if h == 0 {
        return state::status_json(None);
    }
    let r = window_rect(h).unwrap_or_default();
    let (style, ex) = styles(h);
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

// ---- minimal look (Ctrl+H-like) via SetWindowRgn on the video child area -------------

// Cross-tick converger memory. `prev` holds last tick's (window, child) rects for the
// stability debounce; `fs_prev` is the fullscreen-origin dissolve watch's baseline (the
// window rect last seen WITH a live video child); `heal_tries` bounds the reopen heal
// so an unhealable window (e.g. elevated VLC: UIPI silently swallows the SetWindowPos)
// is never fought forever; `heal_wait` throttles snapshots only while VLC is absent.
#[derive(Default)]
pub struct RegionTracker {
    prev: Option<(geometry::Rect, geometry::Rect)>,
    fs_prev: Option<geometry::Rect>,
    heal_tries: u32,
    heal_wait: u8,
}

impl RegionTracker {
    pub fn reset_debounce(&mut self) {
        self.prev = None;
    }

    fn finish_state_drop(&mut self, dropped: bool) -> bool {
        if dropped {
            *self = Self::default();
        }
        dropped
    }
}

/// Qt left fullscreen UNDERNEATH a fullscreen-origin PiP (media end and stop do this
/// with no input; the window balloons to Qt's windowed geometry within a tick). Dissolve
/// the session: frame back at Qt's chosen rect, then drop state. A failed deletion keeps
/// the tracker's live-video baseline so the next tick retries the terminal transition.
fn dissolve_fs_pip(s: &PipState, path: &Path) -> bool {
    let h = s.hwnd;
    let after = restore_frame(
        h,
        s.style | (WS_CAPTION | WS_THICKFRAME) as isize,
        s.ex_style,
    );
    unsafe {
        SetWindowPos(
            hw(h),
            after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED,
        );
    }
    drop_state(s, path)
}

/// Converging per-tick maintenance (daemon timer + one-shot enter): no video -> clear
/// region; child not at target size -> resize with chrome compensation; child at target
/// -> clip to the video area. Acts only on STABLE frames (window+child rects unchanged
/// since the previous tick): VLC re-fits the child asynchronously after our resize, so
/// a fresh measurement can be stale and yield garbage chrome. Returns true only when a
/// terminal dissolve/heal path drops state, so the daemon can resync its session cache.
pub fn maintain_region(t: &mut RegionTracker, s: Option<PipState>) -> bool {
    let path = state::state_path();
    let Some(s) = s else {
        *t = RegionTracker::default();
        return false;
    };
    if !owns_state(&s) {
        t.prev = None;
        return heal_reopened(&mut t.heal_tries, &mut t.heal_wait, &s, &path);
    }
    t.heal_tries = 0; // a live owned PiP ends any interrupted heal cleanly
    t.heal_wait = 0;
    let h = s.hwnd;
    let child = find_video_child(h);
    let wr = window_rect(h).unwrap_or_default(); // one snapshot serves watch and debounce

    // fullscreen-origin dissolve watch - BEFORE the min gate, it guards every fs session
    if fs_origin(s.style) {
        if child != 0 {
            t.fs_prev = Some(wr);
        } else if t.fs_prev.is_some_and(|p| p != wr) {
            let dropped = dissolve_fs_pip(&s, &path);
            return t.finish_state_drop(dropped);
        }
    } else {
        t.fs_prev = None;
    }

    if !s.min {
        return false;
    }

    if child == 0 {
        t.prev = None;
        if has_region(h) {
            unsafe { SetWindowRgn(hw(h), std::ptr::null_mut(), 1) }; // playback stopped: show full mini UI
        }
        return false;
    }

    let cr = window_rect(child).unwrap_or_default();
    let stable = t.prev == Some((wr, cr));
    t.prev = Some((wr, cr));
    if !stable {
        return false; // wait until VLC's re-layout settles
    }

    match geometry::plan_region(&wr, &cr, s.target_w, s.target_h, s.corner, s.margin, || {
        work_area(h)
    }) {
        geometry::RegionPlan::Skip => {}
        geometry::RegionPlan::Resize { x, y, w, h: th } => {
            unsafe { SetWindowPos(hw(h), HWND_TOPMOST, x, y, w, th, SWP_FRAMECHANGED) };
            t.prev = None; // our own resize invalidates the measurement
        }
        geometry::RegionPlan::Clip {
            left,
            top,
            right,
            bottom,
        } => {
            // verify the box, not just presence: a live-clipped resize drag leaves an
            // approximate region that convergence must confirm or correct
            if region_box(h) != Some((left, top, right, bottom)) {
                set_region(h, left, top, right, bottom);
            }
        }
    }
    false
}

pub(crate) fn heal_snapshot_due(wait: &mut u8) -> bool {
    if *wait == 0 {
        true
    } else {
        *wait -= 1;
        false
    }
}

pub(crate) fn heal_target(s: &PipState) -> Option<geometry::Rect> {
    Some(geometry::Rect {
        left: s.x,
        top: s.y,
        right: s.x.checked_add(s.w)?,
        bottom: s.y.checked_add(s.h)?,
    })
}

fn finish_heal(tries: &mut u32, wait: &mut u8, s: &PipState, path: &Path) -> bool {
    let dropped = drop_state(s, path);
    if dropped {
        *tries = 0;
        *wait = 0;
    }
    dropped
}

fn reopen_replacement_ready(recorded_pid: u32, pids: &[u32]) -> bool {
    !pids.is_empty() && !pids.contains(&recorded_pid)
}

/// VLC that closes while in PiP persists the PiP geometry as its own (Qt saves on exit),
/// so its next launch opens full-size at the PiP origin, overflowing the screen. The
/// stale state file is kept as a pending-restore record; when a new player window
/// appears, apply the saved pre-PiP rect and delete the record only once the rect is
/// observed to stick - VLC's own startup positioning must not win the race.
fn heal_reopened(tries: &mut u32, wait: &mut u8, s: &PipState, path: &Path) -> bool {
    if s.w <= 0 || s.h <= 0 || s.pid == 0 {
        return finish_heal(tries, wait, s, path); // garbage record: not healable
    }
    if fs_origin(s.style) {
        // a fullscreen-origin PiP's record holds the FULLSCREEN rect - never heal a
        // reopened window to it. Qt believed fullscreen the whole session, so it
        // persisted its true windowed geometry itself: nothing to heal.
        return finish_heal(tries, wait, s, path);
    }
    let Some(target) = heal_target(s) else {
        return finish_heal(tries, wait, s, path);
    };
    if !heal_snapshot_due(wait) {
        return false;
    }
    let pids = vlc_pids();
    if !reopen_replacement_ready(s.pid, &pids) {
        *wait = 6;
        return false; // wait until the recorded process is gone and another VLC is ready
    }
    *wait = 0; // VLC is back: check every tick while its player window settles
    let h2 = find_player_for_pids(&pids);
    if h2 == 0 {
        return false;
    }
    unsafe {
        if IsIconic(hw(h2)) != 0 {
            return false; // heal the normal placement once restored - the iconic rect is garbage
        }
        let tr = RECT {
            left: target.left,
            top: target.top,
            right: target.right,
            bottom: target.bottom,
        };
        if MonitorFromRect(&tr, MONITOR_DEFAULTTONULL).is_null() {
            return finish_heal(tries, wait, s, path); // monitor layout changed: VLC's placement is saner
        }
        if window_rect(h2) == Some(target) {
            return finish_heal(tries, wait, s, path); // heal landed and stuck: done
        }
        *tries += 1;
        if *tries > 40 {
            return finish_heal(tries, wait, s, path); // not converging: stop fighting the window
        }
        SetWindowPos(
            hw(h2),
            std::ptr::null_mut(),
            s.x,
            s.y,
            s.w,
            s.h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    false
}

#[cfg(test)]
mod internal_tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> geometry::Rect {
        geometry::Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn state(pid: u32) -> PipState {
        PipState {
            hwnd: 1,
            x: 100,
            y: 200,
            w: 1000,
            h: 640,
            style: WS_CAPTION as isize,
            ex_style: 0,
            target_w: 480,
            target_h: 270,
            corner: geometry::Corner::Br,
            margin: 16,
            min: true,
            pid,
        }
    }

    #[test]
    fn heal_reports_only_terminal_state_deletion() {
        let path = std::env::temp_dir().join(format!(
            "pip-maintenance-signal-test-{}.state",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&path);
        std::fs::create_dir(&path).unwrap();
        let (mut tries, mut wait) = (9, 6);
        assert!(!heal_reopened(&mut tries, &mut wait, &state(0), &path));
        assert!(path.is_dir());

        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "pending").unwrap();
        assert!(heal_reopened(&mut tries, &mut wait, &state(0), &path));
        assert!(!path.exists());
    }

    #[test]
    fn reopen_heal_waits_for_the_recorded_process_to_exit() {
        assert!(!reopen_replacement_ready(42, &[]));
        assert!(!reopen_replacement_ready(42, &[42]));
        assert!(!reopen_replacement_ready(42, &[7, 42]));
        assert!(reopen_replacement_ready(42, &[7]));
    }

    #[test]
    fn maintenance_restore_has_distinct_terminal_codes() {
        assert_eq!(RestoreResult::Restored.code(), 0);
        assert_eq!(RestoreResult::Failed.code(), 1);
        assert_eq!(RestoreResult::Pending.code(), 4);
    }

    #[test]
    fn failed_state_drop_preserves_dissolve_baseline() {
        let path = std::env::temp_dir().join(format!(
            "pip-dissolve-drop-test-{}.state",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&path);
        std::fs::create_dir(&path).unwrap();
        let baseline = rect(1, 2, 3, 4);
        let mut tracker = RegionTracker {
            fs_prev: Some(baseline),
            ..RegionTracker::default()
        };

        assert!(!tracker.finish_state_drop(drop_state(&state(42), &path)));
        assert_eq!(tracker.fs_prev, Some(baseline));

        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "pending").unwrap();
        assert!(tracker.finish_state_drop(drop_state(&state(42), &path)));
        assert_eq!(tracker.fs_prev, None);
    }

    #[test]
    fn reset_debounce_preserves_dissolve_and_heal_tracking() {
        let previous = rect(1, 2, 3, 4);
        let baseline = rect(5, 6, 7, 8);
        let mut tracker = RegionTracker {
            prev: Some((previous, previous)),
            fs_prev: Some(baseline),
            heal_tries: 9,
            heal_wait: 6,
        };

        tracker.reset_debounce();

        assert_eq!(tracker.prev, None);
        assert_eq!(tracker.fs_prev, Some(baseline));
        assert_eq!(tracker.heal_tries, 9);
        assert_eq!(tracker.heal_wait, 6);
    }

    #[test]
    fn absent_state_resets_all_session_tracking() {
        let previous = rect(1, 2, 3, 4);
        let baseline = rect(5, 6, 7, 8);
        let mut tracker = RegionTracker {
            prev: Some((previous, previous)),
            fs_prev: Some(baseline),
            heal_tries: 9,
            heal_wait: 6,
        };

        assert!(!maintain_region(&mut tracker, None));

        assert_eq!(tracker.prev, None);
        assert_eq!(tracker.fs_prev, None);
        assert_eq!(tracker.heal_tries, 0);
        assert_eq!(tracker.heal_wait, 0);
    }
}
