use std::path::PathBuf;

use crate::geometry::Corner;

pub struct PipOptions {
    pub w: i32,
    pub h: i32,
    pub corner: Corner,
    pub margin: i32,
    pub min: bool,
}

impl Default for PipOptions {
    fn default() -> Self {
        Self {
            w: 480,
            h: 270,
            corner: Corner::Br,
            margin: 16,
            min: true,
        }
    }
}

pub fn parse_options<'a>(args: impl IntoIterator<Item = &'a str>) -> PipOptions {
    let mut o = PipOptions::default();
    // w/h pinned positive: 0/negative would park an invisible topmost window
    let pos = |v: &str| v.trim().parse::<i32>().ok().filter(|&n| n > 0);
    for a in args {
        let Some(i) = a.find('=') else { continue };
        let (k, v) = (&a[..i], &a[i + 1..]);
        match k {
            "w" => {
                if let Some(n) = pos(v) {
                    o.w = n;
                }
            }
            "h" => {
                if let Some(n) = pos(v) {
                    o.h = n;
                }
            }
            "c" => o.corner = Corner::parse(v),
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
    std::env::var_os("APPDATA").map(|mut a| {
        a.push(r"\vlc\pip\config.txt");
        a.into()
    })
}

/// defaults < config tokens < argv: parse_options' later-duplicates-win does the layering.
pub fn merge(cfg: &str, argv: &[String]) -> PipOptions {
    parse_options(
        cfg.split_whitespace()
            .chain(argv.iter().map(String::as_str)),
    )
}

/// Options in effect for an enter. Config is re-read per call so the daemon picks up its
/// own gesture writes (and hand edits) without a restart.
pub fn effective(argv: &[String]) -> PipOptions {
    let cfg = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    merge(&cfg, argv)
}

/// Written on drag release. Failure swallowed: the gesture still holds via the state file.
pub fn save_config(w: i32, h: i32, corner: Corner) {
    let Some(p) = config_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, format!("w={w} h={h} c={}", corner.as_str()));
}
