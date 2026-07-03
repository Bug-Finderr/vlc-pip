use super::*;

#[test]
fn defaults() {
    let o = parse_options([]);
    assert_eq!((o.w, o.h, o.corner, o.margin, o.min), (480, 270, "br", 16, true));
}

#[test]
fn parses_all_keys() {
    let o = parse_options(["w=640", "h=360", "c=tr", "m=24", "min=0"]);
    assert_eq!((o.w, o.h, o.corner, o.margin, o.min), (640, 360, "tr", 24, false));
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
    assert_eq!(parse_options(["c=zz"]).corner, "br"); // writer never escapes: normalize here (SPEC R2)
    assert_eq!(parse_options(["c=bl"]).corner, "bl");
}

#[test]
fn later_duplicates_win() {
    assert_eq!(parse_options(["w=100", "w=200"]).w, 200);
}

#[test]
fn merge_config_beats_defaults_argv_beats_config() {
    let argv = vec!["w=800".to_string()];
    let o = merge("w=640 h=360 c=tr", &argv);
    assert_eq!((o.w, o.h, o.corner), (800, 360, "tr"));
}

#[test]
fn merge_empty_config_is_v2_behavior() {
    let o = merge("", &[]);
    assert_eq!((o.w, o.h, o.corner, o.margin, o.min), (480, 270, "br", 16, true));
}

#[test]
fn merge_garbage_config_tokens_ignored() {
    let o = merge("lol w=x h=999", &[]);
    assert_eq!((o.w, o.h), (480, 999));
}
