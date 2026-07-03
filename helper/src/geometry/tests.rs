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

const WORK: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1040 };

fn rc(l: i32, t: i32, r: i32, b: i32) -> Rect {
    Rect { left: l, top: t, right: r, bottom: b }
}

#[test]
fn resize_br_grows_anchored_tl() {
    // 480x270 at (100,100); +48/+27 is width-driven (48*270 >= 27*480): 528x297
    assert_eq!(plan_resize(&rc(100, 100, 580, 370), DragZone::BottomRight, 48, 27, &WORK), rc(100, 100, 628, 397));
}

#[test]
fn resize_tl_anchors_br() {
    // dw = -dx = 48: 528x297 anchored at (right,bottom)
    assert_eq!(plan_resize(&rc(100, 100, 580, 370), DragZone::TopLeft, -48, 0, &WORK), rc(52, 73, 580, 370));
}

#[test]
fn resize_right_edge_keeps_vertical_center() {
    // edge zone: dy ignored; 576x324, v-center 235 fixed
    assert_eq!(plan_resize(&rc(100, 100, 580, 370), DragZone::Right, 96, 500, &WORK), rc(100, 73, 676, 397));
}

#[test]
fn resize_top_edge_keeps_horizontal_center() {
    // dh = -dy = 54 -> h-driven: 576x324, anchored bottom, h-center 340 fixed
    assert_eq!(plan_resize(&rc(100, 100, 580, 370), DragZone::Top, 500, -54, &WORK), rc(52, 46, 628, 370));
}

#[test]
fn resize_corner_height_driven_when_dy_dominates() {
    // 100*480 > 30*270: h = 370 -> w = 370*480/270 = 657 -> h = 657*270/480 = 369
    assert_eq!(plan_resize(&rc(0, 0, 480, 270), DragZone::BottomRight, 30, 100, &WORK), rc(0, 0, 657, 369));
}

#[test]
fn resize_clamps_min_256() {
    assert_eq!(plan_resize(&rc(0, 0, 480, 270), DragZone::BottomRight, -400, -400, &WORK), rc(0, 0, 256, 144));
}

#[test]
fn resize_clamps_max_80pct_work() {
    // max_w = min(1536, 832*480/270 = 1479) = 1479; h = 1479*270/480 = 831
    assert_eq!(plan_resize(&rc(0, 0, 480, 270), DragZone::BottomRight, 5000, 0, &WORK), rc(0, 0, 1479, 831));
}

#[test]
fn resize_degenerate_start_is_noop() {
    assert_eq!(plan_resize(&rc(0, 0, 0, 270), DragZone::BottomRight, 50, 50, &WORK), rc(0, 0, 0, 270));
}

#[test]
fn resize_tiny_work_area_clamp_does_not_panic() {
    // 80% of 200 < 256: max floors to min - clamp() must not see min > max
    let tiny = rc(0, 0, 200, 200);
    assert_eq!(plan_resize(&rc(0, 0, 480, 270), DragZone::BottomRight, -400, 0, &tiny), rc(0, 0, 256, 144));
}
