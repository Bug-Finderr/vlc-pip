use super::*;

// byte-for-byte what C# System.Text.Json source-gen emitted (verified against a live run)
const FULL: &str = r#"{"Hwnd":66112,"X":100,"Y":200,"W":1000,"H":640,"Style":349110272,"ExStyle":256,"TargetW":480,"TargetH":270,"Corner":"br","Margin":16,"Min":true,"Pid":12345}"#;
// v1.0 pre-audit 7-field format, exactly as pinned in the retired C# StateTests
const OLD: &str = r#"{"Hwnd":4660,"X":100,"Y":200,"W":1000,"H":640,"Style":349110272,"ExStyle":256}"#;

fn tmp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("pip-state-test-{name}-{}.json", std::process::id()))
}

#[test]
fn temp_path_matches_std_join_byte_for_byte() {
    // as_os_str: PathBuf PartialEq normalizes separators and would mask a doubled one
    assert_eq!(temp_path("x.json").as_os_str(), std::env::temp_dir().join("x.json").as_os_str());
}

#[test]
fn full_sample_round_trips_byte_identical() {
    let s = parse_state(FULL).unwrap();
    assert_eq!(s.hwnd, 66112);
    assert_eq!((s.x, s.y, s.w, s.h), (100, 200, 1000, 640));
    assert_eq!((s.style, s.ex_style), (349110272, 256));
    assert_eq!((s.target_w, s.target_h, s.margin), (480, 270, 16));
    assert_eq!(s.corner, "br");
    assert!(s.min);
    assert_eq!(s.pid, 12345);
    assert_eq!(write_state(&s), FULL);
}

#[test]
fn old_format_loads_with_defaults() {
    // missing fields = v1 constructor defaults; Pid=0 then reads as stale (one re-toggle after upgrade)
    let s = parse_state(OLD).unwrap();
    assert_eq!(s.hwnd, 4660);
    assert_eq!(s.w, 1000);
    assert_eq!((s.target_w, s.target_h, s.margin), (480, 270, 16));
    assert_eq!(s.corner, "br");
    assert!(s.min);
    assert_eq!(s.pid, 0);
}

#[test]
fn corrupt_input_reads_as_none() {
    for bad in ["{", "", "not json", &format!("{FULL}x"), r#"{"Hwnd":1}"#] {
        assert!(parse_state(bad).is_none(), "should reject: {bad}");
    }
}

#[test]
fn unknown_scalar_fields_are_skipped() {
    let with_extra = FULL.replace(r#""Pid":12345}"#, r#""Pid":12345,"Future":"x"}"#);
    assert_eq!(parse_state(&with_extra).unwrap().pid, 12345);
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
