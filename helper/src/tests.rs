//! Every unit test, one flat cfg(test) file. Tests are siblings of the modules (not
//! children), so tested-but-internal items cost exactly four pub(crate)s: plan_region,
//! RegionPlan, parse_state, write_state.

mod geometry {
    use crate::geometry::*;

    // work area 0,0..1920x1040 (taskbar excluded), 480x270, margin 16
    #[test]
    fn compute_corner_places_window_inside_work_area() {
        use Corner::*;
        for (corner, ex, ey) in [(Br, 1424, 754), (Bl, 16, 754), (Tr, 1424, 16), (Tl, 16, 16)] {
            assert_eq!(compute_corner(&WORK, 480, 270, corner, 16), (ex, ey), "corner {corner:?}");
        }
    }

    #[test]
    fn corner_parse_unknown_falls_back_to_br() {
        assert_eq!(Corner::parse("zz"), Corner::Br);
        for c in [Corner::Tl, Corner::Tr, Corner::Bl, Corner::Br] {
            assert_eq!(Corner::parse(c.as_str()), c);
        }
    }

    #[test]
    fn nearest_corner_quadrants() {
        let work = Rect { left: 0, top: 0, right: 1920, bottom: 1040 };
        let win = |l: i32, t: i32| Rect { left: l, top: t, right: l + 480, bottom: t + 270 };
        assert_eq!(nearest_corner(&win(10, 10), &work), Corner::Tl);
        assert_eq!(nearest_corner(&win(1400, 10), &work), Corner::Tr);
        assert_eq!(nearest_corner(&win(10, 700), &work), Corner::Bl);
        assert_eq!(nearest_corner(&win(1400, 700), &work), Corner::Br);
    }

    #[test]
    fn nearest_corner_center_tie_is_br() {
        let work = Rect { left: 0, top: 0, right: 1000, bottom: 1000 };
        let win = Rect { left: 400, top: 400, right: 600, bottom: 600 };
        assert_eq!(nearest_corner(&win, &work), Corner::Br);
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
}

mod native {
    use crate::geometry::{self, Corner};
    use crate::native::{plan_region, RegionPlan};
    use windows_sys::Win32::Foundation::RECT;

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
        let plan = plan_region(&rect(0, 0, 480, 270), &rect(0, 0, 481, 270), 480, 270, Corner::Br, 16, work);
        assert_eq!(plan, RegionPlan::Skip);
    }

    #[test]
    fn chrome_clamp_boundary_300_ok_301_stale() {
        // child at target, chrome_h exactly 300 -> clip; 301 -> stale
        let cr = rect(0, 0, 480, 270);
        let ok = plan_region(&rect(0, 0, 480, 570), &cr, 480, 270, Corner::Br, 16, work);
        assert_eq!(ok, RegionPlan::Clip { left: 0, top: 0, right: 480, bottom: 270 });
        let stale = plan_region(&rect(0, 0, 480, 571), &cr, 480, 270, Corner::Br, 16, work);
        assert_eq!(stale, RegionPlan::Skip);
    }

    #[test]
    fn two_px_tolerance_clips_three_resizes() {
        // 482 wide child (diff 2) counts as converged; 483 (diff 3) does not
        let at_2 = plan_region(&rect(0, 0, 482, 270), &rect(0, 0, 482, 270), 480, 270, Corner::Br, 16, work);
        assert!(matches!(at_2, RegionPlan::Clip { .. }));
        let at_3 = plan_region(&rect(0, 0, 483, 270), &rect(0, 0, 483, 270), 480, 270, Corner::Br, 16, work);
        assert!(matches!(at_3, RegionPlan::Resize { .. }));
    }

    #[test]
    fn resize_grows_by_chrome_and_lands_child_at_corner() {
        // window 420x360 at (100,100); child 400x225 at rel (10,30) => chrome 20x135
        let plan = plan_region(&rect(100, 100, 520, 460), &rect(110, 130, 510, 355), 480, 270, Corner::Br, 16, work);
        // target 480x270 + chrome => 500x405, positioned so the CHILD hits (1424,754)
        assert_eq!(plan, RegionPlan::Resize { x: 1414, y: 724, w: 500, h: 405 });
    }

    #[test]
    fn clip_is_child_rect_relative_to_window() {
        let plan = plan_region(&rect(1424, 700, 1904, 1024), &rect(1424, 754, 1904, 1024), 480, 270, Corner::Br, 16, work);
        assert_eq!(plan, RegionPlan::Clip { left: 0, top: 54, right: 480, bottom: 324 });
    }

    #[test]
    fn hostile_negative_target_skips() {
        // a hand-crafted state file with a -500 target must not produce a resize
        let plan = plan_region(&rect(0, 0, 480, 300), &rect(0, 20, 480, 290), -500, 270, Corner::Br, 16, work);
        assert_eq!(plan, RegionPlan::Skip);
    }
}

mod options {
    use crate::geometry::Corner;
    use crate::options::*;

    #[test]
    fn defaults() {
        let o = parse_options([]);
        assert_eq!((o.w, o.h, o.corner, o.margin, o.min), (480, 270, Corner::Br, 16, true));
    }

    #[test]
    fn parses_all_keys() {
        let o = parse_options(["w=640", "h=360", "c=tr", "m=24", "min=0"]);
        assert_eq!((o.w, o.h, o.corner, o.margin, o.min), (640, 360, Corner::Tr, 24, false));
    }

    #[test]
    fn min_only_zero_and_false_disable() {
        assert!(!parse_options(["min=0"]).min);
        assert!(!parse_options(["min=false"]).min);
        assert!(!parse_options(["min=FALSE"]).min);
        assert!(parse_options(["min=no"]).min); // v1: anything else is true
        assert!(parse_options(["min="]).min);
    }

    #[test]
    fn bad_and_unknown_tokens_ignored() {
        let o = parse_options(["w=abc", "width=5", "noequals", "=x"]);
        assert_eq!((o.w, o.h), (480, 270));
    }

    #[test]
    fn non_positive_w_h_ignored() {
        let o = parse_options(["w=0", "h=-500"]);
        assert_eq!((o.w, o.h), (480, 270));
    }

    #[test]
    fn corner_normalized_to_known_values() {
        assert_eq!(parse_options(["c=zz"]).corner, Corner::Br);
        assert_eq!(parse_options(["c=bl"]).corner, Corner::Bl);
    }

    #[test]
    fn later_duplicates_win() {
        assert_eq!(parse_options(["w=100", "w=200"]).w, 200);
    }

    #[test]
    fn merge_config_beats_defaults_argv_beats_config() {
        let argv = vec!["w=800".to_string()];
        let o = merge("w=640 h=360 c=tr", &argv);
        assert_eq!((o.w, o.h, o.corner), (800, 360, Corner::Tr));
    }

    #[test]
    fn merge_empty_config_is_v2_behavior() {
        let o = merge("", &[]);
        assert_eq!((o.w, o.h, o.corner, o.margin, o.min), (480, 270, Corner::Br, 16, true));
    }

    #[test]
    fn merge_garbage_config_tokens_ignored() {
        let o = merge("lol w=x h=999", &[]);
        assert_eq!((o.w, o.h), (480, 999));
    }
}

mod request {
    use crate::request::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pip-req-test-{name}-{}.txt", std::process::id()))
    }

    #[test]
    fn consume_reads_command_and_deletes_file() {
        let path = tmp("consume");
        std::fs::write(&path, "toggle\r\n").unwrap();
        assert_eq!(consume(&path).as_deref(), Some("toggle"));
        assert!(!path.exists());
    }

    #[test]
    fn consume_missing_file_returns_none() {
        assert_eq!(consume(&tmp("nope")), None);
    }

    #[test]
    fn consume_empty_file_is_deleted_and_none() {
        let path = tmp("empty");
        std::fs::write(&path, "  \r\n").unwrap();
        assert_eq!(consume(&path), None);
        assert!(!path.exists());
    }
}

mod state {
    use crate::geometry::Corner;
    use crate::state::*;

    const FULL: &str = "66112 100 200 1000 640 349110272 256 480 270 br 16 1 12345";

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pip-state-test-{name}-{}.txt", std::process::id()))
    }

    #[test]
    fn full_sample_round_trips_byte_identical() {
        let s = parse_state(FULL).unwrap();
        assert_eq!(s.hwnd, 66112);
        assert_eq!((s.x, s.y, s.w, s.h), (100, 200, 1000, 640));
        assert_eq!((s.style, s.ex_style), (349110272, 256));
        assert_eq!((s.target_w, s.target_h, s.margin), (480, 270, 16));
        assert_eq!(s.corner, Corner::Br);
        assert!(s.min);
        assert_eq!(s.pid, 12345);
        assert_eq!(write_state(&s), FULL);
    }

    #[test]
    fn corrupt_input_reads_as_none() {
        let truncated = &FULL[..FULL.len() - 6]; // 12 tokens
        let extra = format!("{FULL} 7"); // 14 tokens
        let bad_num = FULL.replace("349110272", "wide");
        let bad_min = FULL.replace(" 1 12345", " yes 12345");
        for bad in ["", "not a state", truncated, &extra, &bad_num, &bad_min] {
            assert!(parse_state(bad).is_none(), "should reject: {bad}");
        }
    }

    #[test]
    fn state_round_trips_via_file() {
        let path = tmp("roundtrip");
        let s = parse_state(FULL).unwrap();
        save(&s, &path).unwrap();
        let loaded = load(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(loaded, Some(s));
    }

    #[test]
    fn load_missing_file_returns_none() {
        assert_eq!(load(&tmp("nope")), None);
    }

    #[test]
    fn status_json_shapes() {
        assert_eq!(status_json(None), r#"{"found":false}"#);
        let s = StatusInfo {
            hwnd: 66112, x: 1424, y: 754, w: 480, h: 270,
            caption: false, topmost: true, in_pip: true, minimal: true,
        };
        assert_eq!(
            status_json(Some(&s)),
            r#"{"found":true,"hwnd":66112,"x":1424,"y":754,"w":480,"h":270,"caption":false,"topmost":true,"inPip":true,"minimal":true}"#
        );
    }
}
