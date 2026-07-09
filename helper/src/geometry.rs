/// Plain rect so this module stays windows-sys-free (native.rs converts at the boundary).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The four PiP corners; anything unknown pins to Br.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Corner {
    Tl,
    Tr,
    Bl,
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

/// Per-axis drag-start signs (-1 low edge, 1 high, 0 neither): (0,0) = interior move, else resize from that edge.
pub type DragZone = (i32, i32);

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

/// Outer `band` px = resize, else move; the low edge wins where opposite bands overlap.
pub fn classify_zone(x: i32, y: i32, vis: &Rect, band: i32) -> DragZone {
    let sx = if x < vis.left + band { -1 } else if x >= vis.right - band { 1 } else { 0 };
    let sy = if y < vis.top + band { -1 } else if y >= vis.bottom - band { 1 } else { 0 };
    (sx, sy)
}

/// Dominant relative delta drives the scale; the other axis follows start's aspect even at the clamps (i64: screen coords overflow i32).
pub fn plan_resize(start: &Rect, zone: DragZone, dx: i32, dy: i32, work: &Rect) -> Rect {
    let (w0, h0) = (start.right - start.left, start.bottom - start.top);
    if w0 < 1 || h0 < 1 {
        return *start; // garbage measurement: no-op
    }
    // a -1 (low) edge grows when the pointer moves toward negative; 0 = axis not dragged
    let (dw, dh) = (zone.0 * dx, zone.1 * dy);
    let width_driven = i64::from(dw.abs()) * i64::from(h0) >= i64::from(dh.abs()) * i64::from(w0);
    let min_w = 256;
    let max_w = ((work.right - work.left) * 4 / 5)
        .min((i64::from(work.bottom - work.top) * 4 / 5 * i64::from(w0) / i64::from(h0)) as i32)
        .max(min_w); // tiny work area: clamp() must never see min > max
    let raw_w = if width_driven { w0 + dw } else { (i64::from(h0 + dh) * i64::from(w0) / i64::from(h0)) as i32 };
    let w = raw_w.clamp(min_w, max_w);
    let h = (i64::from(w) * i64::from(h0) / i64::from(w0)) as i32;
    let (left, right) = match zone.0 {
        -1 => (start.right - w, start.right),
        1 => (start.left, start.left + w),
        _ => {
            let l = start.left + (w0 - w) / 2;
            (l, l + w)
        }
    };
    let (top, bottom) = match zone.1 {
        -1 => (start.bottom - h, start.bottom),
        1 => (start.top, start.top + h),
        _ => {
            let t = start.top + (h0 - h) / 2;
            (t, t + h)
        }
    };
    Rect { left, top, right, bottom }
}

/// Keeps the minimal look live through a resize drag; None when the target shrank below the drag-start chrome.
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

#[derive(Debug, PartialEq)]
pub(crate) enum RegionPlan {
    Skip,
    Resize { x: i32, y: i32, w: i32, h: i32 },
    Clip(Rect),
}

// enter's chrome bound and the converger's MUST match, else enter can land a rect the converger fights forever
pub(crate) const MAX_CHROME: i32 = 300;

/// Pinned at every parse boundary: keeps target + chrome away from i32 overflow, which release builds silently wrap (SPEC 6.1).
pub(crate) fn target_ok(n: i32) -> bool {
    (1..=16_384).contains(&n)
}

/// Per-axis chrome sums: negative or huge = stale rects from VLC's async re-layout.
pub(crate) fn chrome_ok(w: i32, h: i32) -> bool {
    (0..=MAX_CHROME).contains(&w) && (0..=MAX_CHROME).contains(&h)
}

// Resize grows by chrome so the VIDEO is exactly target WxH at the corner; `work` is lazy - only the resize branch needs it.
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
        // target_ok (both parse boundaries) + chrome_ok bound both terms: the sum cannot overflow
        let (tw, th, tx, ty) = (target_w + chrome_w, target_h + chrome_h, vx - rel_l, vy - rel_t);
        if wr.left == tx && wr.top == ty && wr.right - wr.left == tw && wr.bottom - wr.top == th {
            // already at the computed rect: re-issuing the no-op resize would reset the debounce every tick and loop forever
            return RegionPlan::Skip;
        }
        return RegionPlan::Resize { x: tx, y: ty, w: tw, h: th };
    }
    RegionPlan::Clip(Rect { left: rel_l, top: rel_t, right: rel_l + cw, bottom: rel_t + ch })
}
