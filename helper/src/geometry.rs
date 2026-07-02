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
}
