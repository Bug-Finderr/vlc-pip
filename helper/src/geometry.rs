/// Plain rect so this module stays windows-sys-free (native.rs converts at the boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The four PiP corners. Unknown input falls back to Br.
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

/// Where within the visible PiP a drag started, as per-axis signs: -1 is the low
/// edge, 1 the high edge, and 0 neither. `(0, 0)` is the move interior.
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

/// Zone of a point in the visible rect: outer `band` px = resize, else move. Axes are
/// independent, and the low edge wins where opposite bands overlap.
pub fn classify_zone(x: i32, y: i32, vis: &Rect, band: i32) -> DragZone {
    let sx = if x < vis.left + band {
        -1
    } else if x >= vis.right - band {
        1
    } else {
        0
    };
    let sy = if y < vis.top + band {
        -1
    } else if y >= vis.bottom - band {
        1
    } else {
        0
    };
    (sx, sy)
}

/// New window rect for a live resize drag. The dominant relative delta drives the scale
/// (edges have one axis by construction); the other dimension follows the VISIBLE box's
/// aspect at drag start - chrome is a constant band, so following the outer rect would
/// skew the video ratio on every resize, and release persists that skew. Chrome outside
/// the 0..=MAX_CHROME sanity range plans chrome-free from the outer rect. i64
/// intermediates accept the full i32 pointer-delta range; an anchored result outside
/// Win32's i32 coordinate range is a no-op.
pub fn plan_resize(
    start: &Rect,
    vis: &Rect,
    zone: DragZone,
    dx: i64,
    dy: i64,
    work: &Rect,
) -> Rect {
    let (ow0, oh0) = (start.right - start.left, start.bottom - start.top);
    let (cw, ch) = if credible_vis(start, vis) {
        (ow0 - (vis.right - vis.left), oh0 - (vis.bottom - vis.top))
    } else {
        (0, 0) // stale region measurement: plan on the outer rect
    };
    let (w0, h0) = (ow0 - cw, oh0 - ch);
    if w0 < 1 || h0 < 1 {
        return *start; // garbage measurement: no-op
    }
    let limit = i64::from(u32::MAX); // largest difference between two i32 coordinates
    let (dx, dy) = (dx.clamp(-limit, limit), dy.clamp(-limit, limit));
    let dw = match zone.0 {
        -1 => -dx,
        1 => dx,
        _ => 0,
    };
    let dh = match zone.1 {
        -1 => -dy,
        1 => dy,
        _ => 0,
    };
    let width_driven = dw.abs() * i64::from(h0) >= dh.abs() * i64::from(w0);
    let min_w = 256;
    let max_w = ((work.right - work.left) * 4 / 5 - cw)
        .min(
            ((i64::from(work.bottom - work.top) * 4 / 5 - i64::from(ch)) * i64::from(w0)
                / i64::from(h0)) as i32,
        )
        .max(min_w); // tiny work area: clamp() must never see min > max
    let raw_w = if width_driven {
        i64::from(w0) + dw
    } else {
        (i64::from(h0) + dh).saturating_mul(i64::from(w0)) / i64::from(h0)
    };
    let vw = raw_w.clamp(i64::from(min_w), i64::from(max_w)) as i32;
    // aspects beyond 256:1 truncate to 0, which release would refuse to persist
    let vh = ((i64::from(vw) * i64::from(h0) / i64::from(w0)) as i32).max(1);
    let (w, h) = (i64::from(vw) + i64::from(cw), i64::from(vh) + i64::from(ch));
    let (left, right) = match zone.0 {
        -1 => (i64::from(start.right) - w, i64::from(start.right)),
        1 => (i64::from(start.left), i64::from(start.left) + w),
        _ => {
            let l = i64::from(start.left) + (i64::from(ow0) - w) / 2;
            (l, l + w)
        }
    };
    let (top, bottom) = match zone.1 {
        -1 => (i64::from(start.bottom) - h, i64::from(start.bottom)),
        1 => (i64::from(start.top), i64::from(start.top) + h),
        _ => {
            let t = i64::from(start.top) + (i64::from(oh0) - h) / 2;
            (t, t + h)
        }
    };
    let (Ok(left), Ok(top), Ok(right), Ok(bottom)) = (
        i32::try_from(left),
        i32::try_from(top),
        i32::try_from(right),
        i32::try_from(bottom),
    ) else {
        return *start;
    };
    Rect {
        left,
        top,
        right,
        bottom,
    }
}

/// True when the visible box's implied chrome is credible for `outer`. A stale region
/// mid-relayout implies impossible chrome; a gesture must then treat it as no region,
/// or release would persist target = final size minus garbage chrome.
pub(crate) fn credible_vis(outer: &Rect, vis: &Rect) -> bool {
    let cw = (outer.right - outer.left) - (vis.right - vis.left);
    let ch = (outer.bottom - outer.top) - (vis.bottom - vis.top);
    (0..=MAX_CHROME).contains(&cw) && (0..=MAX_CHROME).contains(&ch)
}

/// Translate a move drag without letting a full-width pointer delta wrap screen
/// coordinates. None means the target cannot be represented by Win32's i32 rect.
pub fn plan_move(start: &Rect, dx: i64, dy: i64) -> Option<Rect> {
    let shift = |n: i32, delta: i64| i64::from(n).checked_add(delta)?.try_into().ok();
    Some(Rect {
        left: shift(start.left, dx)?,
        top: shift(start.top, dy)?,
        right: shift(start.right, dx)?,
        bottom: shift(start.bottom, dy)?,
    })
}

/// Media-adapted PiP box for enter: keep the configured width as the size knob and
/// follow the video's aspect, shrinking at that aspect when the height would exceed
/// 80% of the work area; the 256px floor wins over the cap, matching plan_resize.
/// No media or degenerate dimensions fall back to the configured box.
pub fn adapt_box(o_w: i32, o_h: i32, media: Option<(i32, i32)>, work: &Rect) -> (i32, i32) {
    let Some((mw, mh)) = media else {
        return (o_w, o_h);
    };
    if o_w < 1 || mw < 1 || mh < 1 {
        return (o_w, o_h);
    }
    let mut w = i64::from(o_w);
    let mut h = (w * i64::from(mh) / i64::from(mw)).max(1);
    let max_h = i64::from(work.bottom - work.top) * 4 / 5;
    if h > max_h {
        h = max_h.max(1);
        w = (h * i64::from(mw) / i64::from(mh)).max(1);
        if w < 256 {
            w = 256;
            h = (w * i64::from(mh) / i64::from(mw)).max(1);
        }
    }
    match (i32::try_from(w), i32::try_from(h)) {
        (Ok(w), Ok(h)) => (w, h),
        _ => (o_w, o_h),
    }
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

pub fn compute_corner(
    work: &Rect,
    w: i32,
    h: i32,
    corner: Corner,
    margin: i32,
) -> Option<(i32, i32)> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let left = i64::from(work.left).checked_add(i64::from(margin))?;
    let top = i64::from(work.top).checked_add(i64::from(margin))?;
    let right = i64::from(work.right)
        .checked_sub(i64::from(w))?
        .checked_sub(i64::from(margin))?;
    let bottom = i64::from(work.bottom)
        .checked_sub(i64::from(h))?
        .checked_sub(i64::from(margin))?;
    let (x, y) = match corner {
        Corner::Tl => (left, top),
        Corner::Tr => (right, top),
        Corner::Bl => (left, bottom),
        Corner::Br => (right, bottom),
    };
    Some((x.try_into().ok()?, y.try_into().ok()?))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RegionPlan {
    Skip,
    Resize {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    Clip {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    },
}

// Real chrome (menu + controller + borders) is well under this. Enter's measurement
// and the converger must share the bound so they accept the same geometry.
pub(crate) const MAX_CHROME: i32 = 300;

// Pure planning math for the minimal-look convergence: resize grows by chrome so the
// video is exactly target WxH with the child landing at the corner; clip trims to the
// child area. `work` is lazy because only the resize branch needs its user32 calls.
pub(crate) fn plan_region(
    wr: &Rect,
    cr: &Rect,
    target_w: i32,
    target_h: i32,
    corner: Corner,
    margin: i32,
    work: impl FnOnce() -> Rect,
) -> RegionPlan {
    if target_w <= 0 || target_h <= 0 {
        return RegionPlan::Skip;
    }
    let Some(rel_l) = cr.left.checked_sub(wr.left) else {
        return RegionPlan::Skip;
    };
    let Some(rel_t) = cr.top.checked_sub(wr.top) else {
        return RegionPlan::Skip;
    };
    let Some(rel_r) = wr.right.checked_sub(cr.right) else {
        return RegionPlan::Skip;
    };
    let Some(rel_b) = wr.bottom.checked_sub(cr.bottom) else {
        return RegionPlan::Skip;
    };
    if rel_l < 0 || rel_t < 0 || rel_r < 0 || rel_b < 0 {
        return RegionPlan::Skip;
    }
    let Some(cw) = cr.right.checked_sub(cr.left) else {
        return RegionPlan::Skip;
    };
    let Some(ch) = cr.bottom.checked_sub(cr.top) else {
        return RegionPlan::Skip;
    };
    let Some(ww) = wr.right.checked_sub(wr.left) else {
        return RegionPlan::Skip;
    };
    let Some(wh) = wr.bottom.checked_sub(wr.top) else {
        return RegionPlan::Skip;
    };
    if cw <= 0 || ch <= 0 || ww <= 0 || wh <= 0 {
        return RegionPlan::Skip;
    }
    let Some(chrome_w) = ww.checked_sub(cw) else {
        return RegionPlan::Skip;
    };
    let Some(chrome_h) = wh.checked_sub(ch) else {
        return RegionPlan::Skip;
    };
    // Negative or huge delta means stale rects from VLC's asynchronous re-layout.
    if !(0..=MAX_CHROME).contains(&chrome_w) || !(0..=MAX_CHROME).contains(&chrome_h) {
        return RegionPlan::Skip;
    }
    let Some(width_delta) = cw.checked_sub(target_w) else {
        return RegionPlan::Skip;
    };
    let Some(height_delta) = ch.checked_sub(target_h) else {
        return RegionPlan::Skip;
    };
    if width_delta.abs() > 2 || height_delta.abs() > 2 {
        let Some((vx, vy)) = compute_corner(&work(), target_w, target_h, corner, margin) else {
            return RegionPlan::Skip;
        };
        let Some(tw) = target_w.checked_add(chrome_w) else {
            return RegionPlan::Skip;
        };
        let Some(th) = target_h.checked_add(chrome_h) else {
            return RegionPlan::Skip;
        };
        let Some(tx) = vx.checked_sub(rel_l) else {
            return RegionPlan::Skip;
        };
        let Some(ty) = vy.checked_sub(rel_t) else {
            return RegionPlan::Skip;
        };
        return RegionPlan::Resize {
            x: tx,
            y: ty,
            w: tw,
            h: th,
        };
    }
    let Some(right) = rel_l.checked_add(cw) else {
        return RegionPlan::Skip;
    };
    let Some(bottom) = rel_t.checked_add(ch) else {
        return RegionPlan::Skip;
    };
    RegionPlan::Clip {
        left: rel_l,
        top: rel_t,
        right,
        bottom,
    }
}
