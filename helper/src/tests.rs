//! Pure module tests live here. Tests requiring private state stay by their implementation.

mod geometry {
    use crate::geometry::*;

    // work area 0,0..1920x1040 (taskbar excluded), 480x270, margin 16
    #[test]
    fn compute_corner_places_window_inside_work_area() {
        use Corner::*;
        for (corner, ex, ey) in [(Br, 1424, 754), (Bl, 16, 754), (Tr, 1424, 16), (Tl, 16, 16)] {
            assert_eq!(
                compute_corner(&WORK, 480, 270, corner, 16),
                Some((ex, ey)),
                "corner {corner:?}"
            );
        }
    }

    #[test]
    fn compute_corner_rejects_nonpositive_size() {
        assert_eq!(compute_corner(&WORK, 0, 270, Corner::Br, 16), None);
        assert_eq!(compute_corner(&WORK, 480, -1, Corner::Br, 16), None);
    }

    #[test]
    fn compute_corner_rejects_unrepresentable_coordinate() {
        let work = Rect {
            left: 1,
            top: 0,
            right: 100,
            bottom: 100,
        };
        assert_eq!(compute_corner(&work, 1, 1, Corner::Tl, i32::MAX), None);
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
        let work = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let win = |l: i32, t: i32| Rect {
            left: l,
            top: t,
            right: l + 480,
            bottom: t + 270,
        };
        assert_eq!(nearest_corner(&win(10, 10), &work), Corner::Tl);
        assert_eq!(nearest_corner(&win(1400, 10), &work), Corner::Tr);
        assert_eq!(nearest_corner(&win(10, 700), &work), Corner::Bl);
        assert_eq!(nearest_corner(&win(1400, 700), &work), Corner::Br);
    }

    #[test]
    fn nearest_corner_center_tie_is_br() {
        let work = Rect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 1000,
        };
        let win = Rect {
            left: 400,
            top: 400,
            right: 600,
            bottom: 600,
        };
        assert_eq!(nearest_corner(&win, &work), Corner::Br);
    }

    #[test]
    fn classify_zone_all_nine() {
        let vis = Rect {
            left: 100,
            top: 100,
            right: 580,
            bottom: 370,
        };
        let cases = [
            (300, 200, (0, 0)),
            (105, 200, (-1, 0)),
            (575, 200, (1, 0)),
            (300, 105, (0, -1)),
            (300, 365, (0, 1)),
            (105, 105, (-1, -1)),
            (575, 105, (1, -1)),
            (105, 365, (-1, 1)),
            (575, 365, (1, 1)),
        ];
        for (x, y, z) in cases {
            assert_eq!(classify_zone(x, y, &vis, 16), z, "at ({x},{y})");
        }
    }

    #[test]
    fn classify_zone_band_boundaries() {
        let vis = Rect {
            left: 0,
            top: 0,
            right: 480,
            bottom: 270,
        };
        assert_eq!(classify_zone(15, 135, &vis, 16), (-1, 0)); // x < left+band
        assert_eq!(classify_zone(16, 135, &vis, 16), (0, 0));
        assert_eq!(classify_zone(464, 135, &vis, 16), (1, 0)); // x >= right-band
        assert_eq!(classify_zone(463, 135, &vis, 16), (0, 0));
    }

    #[test]
    fn classify_zone_low_edge_wins_when_opposite_bands_overlap() {
        let vis = Rect {
            left: 100,
            top: 100,
            right: 120,
            bottom: 120,
        };
        assert_eq!(classify_zone(110, 110, &vis, 16), (-1, -1));
    }

    const WORK: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    fn rc(l: i32, t: i32, r: i32, b: i32) -> Rect {
        Rect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    #[test]
    fn resize_br_grows_anchored_tl() {
        // 480x270 at (100,100); +48/+27 is width-driven (48*270 >= 27*480): 528x297
        assert_eq!(
            plan_resize(&rc(100, 100, 580, 370), (1, 1), 48, 27, &WORK),
            rc(100, 100, 628, 397)
        );
    }

    #[test]
    fn resize_tl_anchors_br() {
        // dw = -dx = 48: 528x297 anchored at (right,bottom)
        assert_eq!(
            plan_resize(&rc(100, 100, 580, 370), (-1, -1), -48, 0, &WORK),
            rc(52, 73, 580, 370)
        );
    }

    #[test]
    fn resize_right_edge_keeps_vertical_center() {
        // edge zone: dy ignored; 576x324, v-center 235 fixed
        assert_eq!(
            plan_resize(&rc(100, 100, 580, 370), (1, 0), 96, 500, &WORK),
            rc(100, 73, 676, 397)
        );
    }

    #[test]
    fn resize_top_edge_keeps_horizontal_center() {
        // dh = -dy = 54 -> h-driven: 576x324, anchored bottom, h-center 340 fixed
        assert_eq!(
            plan_resize(&rc(100, 100, 580, 370), (0, -1), 500, -54, &WORK),
            rc(52, 46, 628, 370)
        );
    }

    #[test]
    fn resize_corner_height_driven_when_dy_dominates() {
        // 100*480 > 30*270: h = 370 -> w = 370*480/270 = 657 -> h = 657*270/480 = 369
        assert_eq!(
            plan_resize(&rc(0, 0, 480, 270), (1, 1), 30, 100, &WORK),
            rc(0, 0, 657, 369)
        );
    }

    #[test]
    fn resize_clamps_min_256() {
        assert_eq!(
            plan_resize(&rc(0, 0, 480, 270), (1, 1), -400, -400, &WORK),
            rc(0, 0, 256, 144)
        );
    }

    #[test]
    fn resize_clamps_max_80pct_work() {
        // max_w = min(1536, 832*480/270 = 1479) = 1479; h = 1479*270/480 = 831
        assert_eq!(
            plan_resize(&rc(0, 0, 480, 270), (1, 1), 5000, 0, &WORK),
            rc(0, 0, 1479, 831)
        );
    }

    #[test]
    fn resize_degenerate_start_is_noop() {
        assert_eq!(
            plan_resize(&rc(0, 0, 0, 270), (1, 1), 50, 50, &WORK),
            rc(0, 0, 0, 270)
        );
    }

    #[test]
    fn resize_tiny_work_area_clamp_does_not_panic() {
        // 80% of 200 < 256: max floors to min - clamp() must not see min > max
        let tiny = rc(0, 0, 200, 200);
        assert_eq!(
            plan_resize(&rc(0, 0, 480, 270), (1, 1), -400, 0, &tiny),
            rc(0, 0, 256, 144)
        );
    }

    #[test]
    fn resize_low_edge_handles_min_pointer_delta() {
        assert_eq!(
            plan_resize(&rc(0, 0, 480, 270), (-1, 0), i64::from(i32::MIN), 0, &WORK),
            rc(-999, -280, 480, 551)
        );
    }

    #[test]
    fn resize_accepts_full_pointer_delta_width() {
        let full_delta = i64::from(i32::MAX) - i64::from(i32::MIN);
        assert_eq!(
            plan_resize(&rc(0, 0, 480, 270), (1, 0), full_delta, 0, &WORK),
            rc(0, -280, 1479, 551)
        );
    }

    #[test]
    fn resize_high_edge_overflow_is_noop() {
        let start = rc(i32::MAX - 480, 0, i32::MAX, 270);
        assert_eq!(plan_resize(&start, (1, 0), 1000, 0, &WORK), start);
    }

    #[test]
    fn resize_low_edge_overflow_is_noop() {
        let start = rc(i32::MIN, 0, i32::MIN + 480, 270);
        assert_eq!(plan_resize(&start, (-1, 0), -1000, 0, &WORK), start);
    }

    #[test]
    fn move_translation_preserves_normal_behavior() {
        assert_eq!(
            plan_move(&rc(100, 100, 580, 370), 20, -30),
            Some(rc(120, 70, 600, 340))
        );
    }

    #[test]
    fn move_translation_rejects_unrepresentable_coordinates() {
        let start = rc(i32::MAX - 480, 0, i32::MAX, 270);
        assert_eq!(plan_move(&start, 1, 0), None);
        let full_delta = i64::from(i32::MAX) - i64::from(i32::MIN);
        assert_eq!(plan_move(&rc(0, 0, 480, 270), full_delta, 0), None);
    }

    #[test]
    fn resize_clip_preserves_per_side_chrome() {
        // vis inset 10/30/10/5 in a 480x270 window; target grown to 580x370
        let clip = resize_clip(
            &rc(100, 100, 580, 370),
            &rc(110, 130, 570, 365),
            &rc(100, 100, 680, 470),
        );
        assert_eq!(clip, Some(rc(10, 30, 570, 365)));
    }

    #[test]
    fn resize_clip_target_below_chrome_is_none() {
        // 200px left chrome + 200px right chrome > 210px target width: inverted box
        let clip = resize_clip(
            &rc(0, 0, 480, 270),
            &rc(200, 100, 280, 170),
            &rc(0, 0, 210, 110),
        );
        assert_eq!(clip, None);
    }

    #[test]
    fn chrome_clamp_boundary_300_ok_301_stale() {
        // child at target, chrome_h exactly 300 -> clip; 301 -> stale
        let cr = rc(0, 0, 480, 270);
        let ok = plan_region(&rc(0, 0, 480, 570), &cr, 480, 270, Corner::Br, 16, || WORK);
        assert_eq!(
            ok,
            RegionPlan::Clip {
                left: 0,
                top: 0,
                right: 480,
                bottom: 270
            }
        );
        let stale = plan_region(&rc(0, 0, 480, 571), &cr, 480, 270, Corner::Br, 16, || WORK);
        assert_eq!(stale, RegionPlan::Skip);
    }

    #[test]
    fn two_px_tolerance_clips_three_resizes() {
        // 482 wide child (diff 2) counts as converged; 483 (diff 3) does not
        let at_2 = plan_region(
            &rc(0, 0, 482, 270),
            &rc(0, 0, 482, 270),
            480,
            270,
            Corner::Br,
            16,
            || WORK,
        );
        assert!(matches!(at_2, RegionPlan::Clip { .. }));
        let at_3 = plan_region(
            &rc(0, 0, 483, 270),
            &rc(0, 0, 483, 270),
            480,
            270,
            Corner::Br,
            16,
            || WORK,
        );
        assert!(matches!(at_3, RegionPlan::Resize { .. }));
    }

    #[test]
    fn resize_grows_by_chrome_and_lands_child_at_corner() {
        // window 420x360 at (100,100); child 400x225 at rel (10,30) => chrome 20x135
        let plan = plan_region(
            &rc(100, 100, 520, 460),
            &rc(110, 130, 510, 355),
            480,
            270,
            Corner::Br,
            16,
            || WORK,
        );
        // target 480x270 + chrome => 500x405, positioned so the CHILD hits (1424,754)
        assert_eq!(
            plan,
            RegionPlan::Resize {
                x: 1414,
                y: 724,
                w: 500,
                h: 405
            }
        );
    }

    #[test]
    fn clip_is_child_rect_relative_to_window() {
        let plan = plan_region(
            &rc(1424, 700, 1904, 1024),
            &rc(1424, 754, 1904, 1024),
            480,
            270,
            Corner::Br,
            16,
            || WORK,
        );
        assert_eq!(
            plan,
            RegionPlan::Clip {
                left: 0,
                top: 54,
                right: 480,
                bottom: 324
            }
        );
    }

    #[test]
    fn invalid_region_inputs_skip() {
        let cases = [
            (
                "negative chrome",
                rc(0, 0, 480, 270),
                rc(0, 0, 481, 270),
                480,
                270,
                Corner::Br,
                16,
                WORK,
            ),
            (
                "child outside window",
                rc(0, 0, 2, 1),
                rc(-1, 0, 0, 1),
                1,
                1,
                Corner::Br,
                0,
                WORK,
            ),
            (
                "negative target",
                rc(0, 0, 480, 300),
                rc(0, 20, 480, 290),
                -500,
                270,
                Corner::Br,
                16,
                WORK,
            ),
            (
                "nonpositive target with positive window",
                rc(0, 0, 780, 270),
                rc(0, 0, 480, 270),
                -1,
                270,
                Corner::Br,
                16,
                WORK,
            ),
            (
                "rect difference overflow",
                rc(i32::MIN, 0, i32::MAX, 270),
                rc(0, 0, 480, 270),
                480,
                270,
                Corner::Br,
                16,
                WORK,
            ),
            (
                "target plus chrome overflow",
                rc(0, 0, 481, 270),
                rc(0, 0, 480, 270),
                i32::MAX,
                270,
                Corner::Br,
                16,
                WORK,
            ),
            (
                "coordinate offset overflow",
                rc(0, 0, 780, 270),
                rc(300, 0, 780, 270),
                476,
                270,
                Corner::Tl,
                0,
                rc(i32::MIN, 0, i32::MAX, 1040),
            ),
            (
                "nonpositive rect size",
                rc(100, 0, 99, 270),
                rc(100, 0, 99, 270),
                480,
                270,
                Corner::Br,
                16,
                WORK,
            ),
        ];
        for (name, wr, cr, target_w, target_h, corner, margin, work) in cases {
            assert_eq!(
                plan_region(&wr, &cr, target_w, target_h, corner, margin, || work),
                RegionPlan::Skip,
                "{name}"
            );
        }
    }
}

mod native {
    use crate::geometry::{Corner, Rect};
    use crate::native::{fs_origin, heal_snapshot_due, heal_target};
    use crate::state::PipState;

    fn state() -> PipState {
        PipState {
            hwnd: 1,
            x: 100,
            y: 200,
            w: 1000,
            h: 640,
            style: 1,
            ex_style: 2,
            target_w: 480,
            target_h: 270,
            corner: Corner::Br,
            margin: 16,
            min: true,
            pid: 3,
        }
    }

    #[test]
    fn fs_origin_requires_both_caption_bits_absent() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{WS_BORDER, WS_CAPTION, WS_THICKFRAME};
        assert!(!fs_origin((WS_CAPTION | WS_THICKFRAME) as isize)); // ordinary windowed VLC
        assert!(fs_origin(0)); // fullscreen: caption fully absent
        // WS_CAPTION is two bits (WS_BORDER|WS_DLGFRAME): one bit alone is NOT a caption
        assert!(fs_origin(WS_BORDER as isize));
    }

    #[test]
    fn absent_vlc_snapshot_wait_is_bounded_to_six_ticks() {
        let mut wait = 6;
        for expected in [false, false, false, false, false, false, true] {
            assert_eq!(heal_snapshot_due(&mut wait), expected);
            assert!(wait <= 6);
        }
    }

    #[test]
    fn heal_target_rejects_unrepresentable_edges() {
        let mut s = state();
        assert_eq!(
            heal_target(&s),
            Some(Rect {
                left: 100,
                top: 200,
                right: 1100,
                bottom: 840,
            })
        );

        s.x = i32::MAX;
        assert_eq!(heal_target(&s), None);
        s.x = 100;
        s.y = i32::MAX;
        assert_eq!(heal_target(&s), None);
    }
}

mod options {
    use crate::geometry::Corner;
    use crate::options::*;

    #[test]
    fn defaults() {
        let o = parse_options([]);
        assert_eq!(
            (o.w, o.h, o.corner, o.margin, o.min),
            (480, 270, Corner::Br, 16, true)
        );
    }

    #[test]
    fn parses_all_keys() {
        let o = parse_options(["w=640", "h=360", "c=tr", "m=24", "min=0"]);
        assert_eq!(
            (o.w, o.h, o.corner, o.margin, o.min),
            (640, 360, Corner::Tr, 24, false)
        );
    }

    #[test]
    fn min_only_zero_and_false_disable() {
        assert!(!parse_options(["min=0"]).min);
        assert!(!parse_options(["min=false"]).min);
        assert!(!parse_options(["min=FALSE"]).min);
        assert!(parse_options(["min=no"]).min);
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
    fn later_duplicates_win() {
        assert_eq!(parse_options(["w=100", "w=200"]).w, 200);
    }

    #[test]
    fn merge_config_beats_defaults_argv_beats_config() {
        let argv = vec!["w=800".to_string()];
        let o = merge("w=640 h=360 c=tr", &argv);
        assert_eq!((o.w, o.h, o.corner), (800, 360, Corner::Tr));
    }
}

mod request {
    use crate::state::consume_request as consume;

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

mod daemon {
    use crate::daemon::heartbeat_line;

    #[test]
    fn heartbeat_keeps_diagnostic_field_order_and_numeric_flags() {
        assert_eq!(
            heartbeat_line(1_234, 42, true, false, false, true),
            "1234 pid=42 hotkey=1 timer=0 kb=0 mouse=1"
        );
    }
}

mod state {
    use crate::geometry::Corner;
    use crate::state::*;

    const FULL: &str = "66112 100 200 1000 640 349110272 256 480 270 br 16 1 12345\n";

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pip-state-test-{name}-{}.txt", std::process::id()))
    }

    #[test]
    fn full_sample_round_trips_byte_identical() {
        let s = parse_state(FULL).unwrap();
        let _: (isize, isize, isize) = (s.hwnd, s.style, s.ex_style);
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
    fn invalid_geometry_fields_preserve_restore_metadata() {
        let raw =
            "66112 100 200 1000 640 349110272 256 -2147483648 2147483647 br 2147483647 1 12345\n";
        let s = parse_state(raw).unwrap();
        assert_eq!(
            (s.x, s.y, s.w, s.h, s.style, s.ex_style),
            (100, 200, 1000, 640, 349110272, 256)
        );
        assert_eq!(
            (s.target_w, s.target_h, s.margin),
            (i32::MIN, i32::MAX, i32::MAX)
        );
        assert_eq!(write_state(&s), raw);
    }

    #[test]
    fn native_fields_round_trip_signed_i64_wire_values() {
        let raw = "-1 100 200 1000 640 -2 -3 480 270 br 16 1 12345\n";
        let s = parse_state(raw).unwrap();
        let _: (isize, isize, isize) = (s.hwnd, s.style, s.ex_style);
        assert_eq!((s.hwnd, s.style, s.ex_style), (-1, -2, -3));
        assert_eq!(write_state(&s), raw);
    }

    #[test]
    fn corrupt_input_reads_as_none() {
        let torn_pid = &FULL[..FULL.len() - 3]; // "...123", no newline: torn write
        let short = FULL.replace(" 12345\n", "\n"); // 12 tokens
        let extra = FULL.replace("12345\n", "12345 7\n"); // 14 tokens
        let bad_num = FULL.replace("349110272", "wide");
        let bad_min = FULL.replace(" 1 12345", " yes 12345");
        for bad in [
            "",
            "not a state\n",
            torn_pid,
            &short,
            &extra,
            &bad_num,
            &bad_min,
        ] {
            assert!(parse_state(bad).is_none(), "should reject: {bad:?}");
        }
    }

    #[test]
    fn unknown_corner_token_in_full_line_falls_back_to_br() {
        // corrupt_input pins what is REJECTED; the corner token is the one field that
        // instead accepts-with-fallback (SPEC 6.1: unknown reads as br)
        let s = parse_state(&FULL.replace(" br ", " zz ")).unwrap();
        assert_eq!(s.corner, Corner::Br);
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
    fn try_delete_reports_failure_success_and_absence() {
        let path = tmp("delete-result");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&path);
        std::fs::create_dir(&path).unwrap();
        assert!(!try_delete(&path));
        assert!(path.is_dir());

        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "state").unwrap();
        assert!(try_delete(&path));
        assert!(try_delete(&path)); // NotFound is already the desired state
    }

    #[test]
    fn daemon_runtime_paths_keep_frozen_names() {
        assert_eq!(alive_path().file_name().unwrap(), "vlc-pip-daemon.alive");
        assert_eq!(crash_path().file_name().unwrap(), "vlc-pip-crash.txt");
    }

    #[test]
    fn status_json_shapes() {
        assert_eq!(status_json(None), r#"{"found":false}"#);
        let s = StatusInfo {
            hwnd: 66112,
            x: 1424,
            y: 754,
            w: 480,
            h: 270,
            caption: false,
            topmost: true,
            in_pip: true,
            minimal: true,
        };
        assert_eq!(
            status_json(Some(&s)),
            r#"{"found":true,"hwnd":66112,"x":1424,"y":754,"w":480,"h":270,"caption":false,"topmost":true,"inPip":true,"minimal":true}"#
        );
    }
}
