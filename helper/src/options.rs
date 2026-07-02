use std::path::PathBuf;

pub struct PipOptions {
    pub w: i32,
    pub h: i32,
    pub corner: &'static str,
    pub margin: i32,
    pub min: bool,
}

impl Default for PipOptions {
    fn default() -> Self {
        Self { w: 480, h: 270, corner: "br", margin: 16, min: true }
    }
}

/// The state-file writer does not JSON-escape strings, so corners are pinned to the
/// four legal values at the boundary (unknown = "br", matching `compute_corner`'s fallback).
fn normalize_corner(v: &str) -> &'static str {
    match v {
        "tl" => "tl",
        "tr" => "tr",
        "bl" => "bl",
        _ => "br",
    }
}

pub fn parse_options<'a>(args: impl IntoIterator<Item = &'a str>) -> PipOptions {
    let mut o = PipOptions::default();
    for a in args {
        let Some(i) = a.find('=') else { continue };
        if i < 1 {
            continue;
        }
        let (k, v) = (&a[..i], &a[i + 1..]);
        match k {
            "w" => {
                if let Ok(n) = v.trim().parse() {
                    o.w = n;
                }
            }
            "h" => {
                if let Ok(n) = v.trim().parse() {
                    o.h = n;
                }
            }
            "c" => o.corner = normalize_corner(v),
            "m" => {
                if let Ok(n) = v.trim().parse() {
                    o.margin = n;
                }
            }
            "min" => o.min = v != "0" && !v.eq_ignore_ascii_case("false"),
            _ => {}
        }
    }
    o
}

pub fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"vlc\pip\config.txt"))
}

/// defaults < config tokens < argv: parse_options' later-duplicates-win does the layering.
pub fn merge(cfg: &str, argv: &[String]) -> PipOptions {
    parse_options(cfg.split_whitespace().chain(argv.iter().map(String::as_str)))
}

/// Options in effect for an enter. Config is re-read per call so the daemon picks up its
/// own gesture writes (and hand edits) without a restart.
pub fn effective(argv: &[String]) -> PipOptions {
    let cfg = config_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    merge(&cfg, argv)
}

/// Written on drag release. Failure swallowed: the gesture still holds via the state file.
pub fn save_config(w: i32, h: i32, corner: &str) {
    if let Some(p) = config_path() {
        let _ = std::fs::write(p, format!("w={w} h={h} c={corner}"));
    }
}

#[cfg(test)]
mod tests {
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
}
