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
        let (k, v) = (&a[..i], &a[i + 1..]);
        match k {
            // w/h pinned positive like normalize_corner pins corners: 0/negative would park
            // an invisible topmost window whose region plan is then unverifiable forever
            "w" => {
                if let Ok(n) = v.trim().parse() && n > 0 {
                    o.w = n;
                }
            }
            "h" => {
                if let Ok(n) = v.trim().parse() && n > 0 {
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
    std::env::var_os("APPDATA").map(|mut a| {
        a.push(r"\vlc\pip\config.txt");
        a.into()
    })
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
    let Some(mut p) = std::env::var_os("APPDATA") else { return };
    // %APPDATA%\vlc exists wherever VLC does; one create_dir (not _all) suffices and keeps
    // std's path-component machinery out of the binary. Must build the same string as
    // config_path so reader and writer can never desync.
    p.push(r"\vlc\pip");
    let _ = std::fs::create_dir(&p);
    p.push(r"\config.txt");
    let _ = std::fs::write(p, format!("w={w} h={h} c={corner}"));
}

#[cfg(test)]
mod tests;
