/// Plain rect so this module stays windows-sys-free (native.rs converts at the boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Where within the visible PiP a drag started; stored in an AtomicU8 by the hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DragZone {
    Interior = 0,
    Left = 1,
    Right = 2,
    Top = 3,
    Bottom = 4,
    TopLeft = 5,
    TopRight = 6,
    BottomLeft = 7,
    BottomRight = 8,
}

impl DragZone {
    pub fn from_u8(v: u8) -> Self {
        use DragZone::*;
        match v {
            1 => Left,
            2 => Right,
            3 => Top,
            4 => Bottom,
            5 => TopLeft,
            6 => TopRight,
            7 => BottomLeft,
            8 => BottomRight,
            _ => Interior,
        }
    }
}

/// Work-area quadrant of the window center. Ties resolve toward "br" (codebase fallback).
pub fn nearest_corner(win: &Rect, work: &Rect) -> &'static str {
    let cx = win.left + (win.right - win.left) / 2;
    let cy = win.top + (win.bottom - win.top) / 2;
    let left_half = cx < work.left + (work.right - work.left) / 2;
    let top_half = cy < work.top + (work.bottom - work.top) / 2;
    match (left_half, top_half) {
        (true, true) => "tl",
        (false, true) => "tr",
        (true, false) => "bl",
        (false, false) => "br",
    }
}

/// Zone of a point in the visible rect: outer `band` px = resize, else move. Corners win.
pub fn classify_zone(x: i32, y: i32, vis: &Rect, band: i32) -> DragZone {
    let l = x < vis.left + band;
    let r = x >= vis.right - band;
    let t = y < vis.top + band;
    let b = y >= vis.bottom - band;
    match (l, r, t, b) {
        (true, _, true, _) => DragZone::TopLeft,
        (_, true, true, _) => DragZone::TopRight,
        (true, _, _, true) => DragZone::BottomLeft,
        (_, true, _, true) => DragZone::BottomRight,
        (true, ..) => DragZone::Left,
        (_, true, ..) => DragZone::Right,
        (_, _, true, _) => DragZone::Top,
        (_, _, _, true) => DragZone::Bottom,
        _ => DragZone::Interior,
    }
}

/// New window rect for a live resize drag. The dominant relative delta drives the scale
/// (edges have one axis by construction); the other dimension follows start's aspect,
/// including at the clamps. i64 products: screen coords can make i32 overflow.
pub fn plan_resize(start: &Rect, zone: DragZone, dx: i32, dy: i32, work: &Rect) -> Rect {
    use DragZone::*;
    let (w0, h0) = (start.right - start.left, start.bottom - start.top);
    if w0 < 1 || h0 < 1 {
        return *start; // garbage measurement: no-op
    }
    let dw = match zone {
        Right | TopRight | BottomRight => dx,
        Left | TopLeft | BottomLeft => -dx,
        _ => 0,
    };
    let dh = match zone {
        Bottom | BottomLeft | BottomRight => dy,
        Top | TopLeft | TopRight => -dy,
        _ => 0,
    };
    let width_driven = i64::from(dw.abs()) * i64::from(h0) >= i64::from(dh.abs()) * i64::from(w0);
    let min_w = 256;
    let max_w = ((work.right - work.left) * 4 / 5)
        .min((i64::from(work.bottom - work.top) * 4 / 5 * i64::from(w0) / i64::from(h0)) as i32)
        .max(min_w); // tiny work area: clamp() must never see min > max
    let raw_w = if width_driven { w0 + dw } else { (i64::from(h0 + dh) * i64::from(w0) / i64::from(h0)) as i32 };
    let w = raw_w.clamp(min_w, max_w);
    let h = (i64::from(w) * i64::from(h0) / i64::from(w0)) as i32;
    let (left, right) = match zone {
        Left | TopLeft | BottomLeft => (start.right - w, start.right),
        Right | TopRight | BottomRight => (start.left, start.left + w),
        _ => {
            let l = start.left + (w0 - w) / 2;
            (l, l + w)
        }
    };
    let (top, bottom) = match zone {
        Top | TopLeft | TopRight => (start.bottom - h, start.bottom),
        Bottom | BottomLeft | BottomRight => (start.top, start.top + h),
        _ => {
            let t = start.top + (h0 - h) / 2;
            (t, t + h)
        }
    };
    Rect { left, top, right, bottom }
}

#[allow(clippy::too_many_arguments)] // 4 rect edges + size + corner + margin; keeps this module windows-sys-free
pub fn compute_corner(
    work_left: i32, work_top: i32, work_right: i32, work_bottom: i32,
    w: i32, h: i32, corner: &str, margin: i32,
) -> (i32, i32) {
    let left = work_left + margin;
    let top = work_top + margin;
    let right = work_right - w - margin;
    let bottom = work_bottom - h - margin;
    match corner {
        "tl" => (left, top),
        "tr" => (right, top),
        "bl" => (left, bottom),
        _ => (right, bottom), // "br" and fallback
    }
}
