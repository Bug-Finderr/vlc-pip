/// Plain rect so this module stays windows-sys-free (native.rs converts at the boundary).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Where within the visible PiP a drag started; stored in an AtomicU8 by the hook.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    pub fn from_u8(v: u8) -> DragZone {
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

#[cfg(test)]
mod tests {
    use super::*;

    // work area 0,0..1920x1040 (taskbar excluded), 480x270, margin 16 - pinned from v1
    #[test]
    fn compute_corner_places_window_inside_work_area() {
        for (corner, ex, ey) in [("br", 1424, 754), ("bl", 16, 754), ("tr", 1424, 16), ("tl", 16, 16)] {
            assert_eq!(compute_corner(0, 0, 1920, 1040, 480, 270, corner, 16), (ex, ey), "corner {corner}");
        }
    }

    #[test]
    fn compute_corner_unknown_corner_falls_back_to_br() {
        assert_eq!(compute_corner(0, 0, 1920, 1040, 480, 270, "zz", 16), (1424, 754));
    }

    #[test]
    fn nearest_corner_quadrants() {
        let work = Rect { left: 0, top: 0, right: 1920, bottom: 1040 };
        let win = |l: i32, t: i32| Rect { left: l, top: t, right: l + 480, bottom: t + 270 };
        assert_eq!(nearest_corner(&win(10, 10), &work), "tl");
        assert_eq!(nearest_corner(&win(1400, 10), &work), "tr");
        assert_eq!(nearest_corner(&win(10, 700), &work), "bl");
        assert_eq!(nearest_corner(&win(1400, 700), &work), "br");
    }

    #[test]
    fn nearest_corner_center_tie_is_br() {
        let work = Rect { left: 0, top: 0, right: 1000, bottom: 1000 };
        let win = Rect { left: 400, top: 400, right: 600, bottom: 600 };
        assert_eq!(nearest_corner(&win, &work), "br");
    }

    #[test]
    fn classify_zone_all_nine() {
        let vis = Rect { left: 100, top: 100, right: 580, bottom: 370 };
        let cases = [
            (300, 200, DragZone::Interior), (105, 200, DragZone::Left), (575, 200, DragZone::Right),
            (300, 105, DragZone::Top), (300, 365, DragZone::Bottom), (105, 105, DragZone::TopLeft),
            (575, 105, DragZone::TopRight), (105, 365, DragZone::BottomLeft), (575, 365, DragZone::BottomRight),
        ];
        for (x, y, z) in cases {
            assert_eq!(classify_zone(x, y, &vis, 16), z, "at ({x},{y})");
        }
    }

    #[test]
    fn classify_zone_band_boundaries() {
        let vis = Rect { left: 0, top: 0, right: 480, bottom: 270 };
        assert_eq!(classify_zone(15, 135, &vis, 16), DragZone::Left); // x < left+band
        assert_eq!(classify_zone(16, 135, &vis, 16), DragZone::Interior);
        assert_eq!(classify_zone(464, 135, &vis, 16), DragZone::Right); // x >= right-band
        assert_eq!(classify_zone(463, 135, &vis, 16), DragZone::Interior);
    }

    #[test]
    fn drag_zone_u8_round_trip() {
        use DragZone::*;
        for z in [Interior, Left, Right, Top, Bottom, TopLeft, TopRight, BottomLeft, BottomRight] {
            assert_eq!(DragZone::from_u8(z as u8), z);
        }
    }
}
