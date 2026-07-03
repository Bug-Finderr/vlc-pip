use super::*;

fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT { left: l, top: t, right: r, bottom: b }
}

fn work() -> geometry::Rect {
    // 480x270 br margin 16 => video corner at (1424, 754)
    geometry::Rect { left: 0, top: 0, right: 1920, bottom: 1040 }
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
