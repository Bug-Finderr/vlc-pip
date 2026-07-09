/// Plain rect so this module stays windows-sys-free (native.rs converts at the boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The four PiP corners; anything unknown pins to Br (v1 semantics, same fallback everywhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Corner {
    Tl,
    Tr,
    Bl,
    #[default]
    Br,
}

impl Corner {
    pub fn parse(s: &str) -> Self {
        match s {
            "tl" => Self::Tl,
            "tr" => Self::Tr,
            "bl" => Self::Bl,
            _ => Self::Br,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tl => "tl",
            Self::Tr => "tr",
            Self::Bl => "bl",
            Self::Br => "br",
        }
    }
}

/// Where within the visible PiP a drag started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DragZone {
    #[default]
    Interior,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Work-area quadrant of the window center. Ties resolve toward Br.
pub fn nearest_corner(win: &Rect, work: &Rect) -> Corner {
    let cx = win.left + (win.right - win.left) / 2;
    let cy = win.top + (win.bottom - win.top) / 2;
    let left_half = cx < work.left + (work.right - work.left) / 2;
    let top_half = cy < work.top + (work.bottom - work.top) / 2;
    match (left_half, top_half) {
        (true, true) => Corner::Tl,
        (false, true) => Corner::Tr,
        (true, false) => Corner::Bl,
        (false, false) => Corner::Br,
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

/// Window-relative region that keeps the minimal look live through a resize drag: the
/// per-side chrome measured at drag start, applied to the target size. None when the
/// target has shrunk below the chrome (an inverted box must not clip).
pub fn resize_clip(start: &Rect, vis: &Rect, target: &Rect) -> Option<Rect> {
    let c = Rect {
        left: vis.left - start.left,
        top: vis.top - start.top,
        right: (target.right - target.left) - (start.right - vis.right),
        bottom: (target.bottom - target.top) - (start.bottom - vis.bottom),
    };
    (c.right > c.left && c.bottom > c.top).then_some(c)
}

pub fn compute_corner(work: &Rect, w: i32, h: i32, corner: Corner, margin: i32) -> (i32, i32) {
    let left = work.left + margin;
    let top = work.top + margin;
    let right = work.right - w - margin;
    let bottom = work.bottom - h - margin;
    match corner {
        Corner::Tl => (left, top),
        Corner::Tr => (right, top),
        Corner::Bl => (left, bottom),
        Corner::Br => (right, bottom),
    }
}

// ---- minimal-look convergence planning (applied by native::maintain_region) -----------

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RegionPlan {
    Skip,
    Resize { x: i32, y: i32, w: i32, h: i32 },
    Clip(Rect),
}

// Real chrome (menu + controller + borders) is well under this. enter's measurement and
// the converger MUST share the bound: a chrome enter accepts but plan_region skips
// would land a rect the converger fights forever.
pub(crate) const MAX_CHROME: i32 = 300;

/// Per-axis chrome sums: negative or huge = stale rects from VLC's async re-layout.
pub(crate) fn chrome_ok(w: i32, h: i32) -> bool {
    (0..=MAX_CHROME).contains(&w) && (0..=MAX_CHROME).contains(&h)
}

// Pure planning math for the minimal-look convergence: resize grows by chrome so the
// VIDEO is exactly target WxH with the child landing at the corner; clip trims to the
// child area. `work` is lazy - only the resize branch needs its two user32 calls.
pub(crate) fn plan_region(
    wr: &Rect, cr: &Rect, target_w: i32, target_h: i32,
    corner: Corner, margin: i32, work: impl FnOnce() -> Rect,
) -> RegionPlan {
    let rel_l = cr.left - wr.left;
    let rel_t = cr.top - wr.top;
    let cw = cr.right - cr.left;
    let ch = cr.bottom - cr.top;
    let chrome_w = (wr.right - wr.left) - cw;
    let chrome_h = (wr.bottom - wr.top) - ch;
    if !chrome_ok(chrome_w, chrome_h) {
        return RegionPlan::Skip;
    }
    if (cw - target_w).abs() > 2 || (ch - target_h).abs() > 2 {
        let wa = work();
        let (vx, vy) = compute_corner(&wa, target_w, target_h, corner, margin);
        // targets are pinned positive at both parse boundaries (options + state file),
        // and chrome is non-negative here, so target + chrome cannot underflow
        let (tw, th, tx, ty) = (target_w + chrome_w, target_h + chrome_h, vx - rel_l, vy - rel_t);
        if wr.left == tx && wr.top == ty && wr.right - wr.left == tw && wr.bottom - wr.top == th {
            // already at the computed rect but the child never re-fit: re-issuing the
            // no-op resize would reset the debounce every tick and loop forever
            return RegionPlan::Skip;
        }
        return RegionPlan::Resize { x: tx, y: ty, w: tw, h: th };
    }
    RegionPlan::Clip(Rect { left: rel_l, top: rel_t, right: rel_l + cw, bottom: rel_t + ch })
}
