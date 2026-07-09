use std::path::{Path, PathBuf};

use crate::geometry::Corner;

// The PiP state: x..ex_style restore the window; target_w..min are the options in
// effect at Enter (daemon and one-shot CLI converge on the same geometry); pid guards
// against HWND recycling. A VALID file on disk == "in PiP".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PipState {
    pub hwnd: i64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub style: i64,
    pub ex_style: i64,
    pub target_w: i32,
    pub target_h: i32,
    pub corner: Corner,
    pub margin: i32,
    pub min: bool,
    pub pid: u32,
}

/// %TEMP%\{name} for every IPC file.
pub fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

pub fn state_path() -> PathBuf {
    temp_path("vlc-pip.state")
}

pub fn load(path: &Path) -> Option<PipState> {
    parse_state(&std::fs::read_to_string(path).ok()?) // missing or unreadable = "not in PiP"
}

pub fn save(s: &PipState, path: &Path) -> std::io::Result<()> {
    std::fs::write(path, write_state(s))
}

pub fn try_delete(path: &Path) {
    let _ = std::fs::remove_file(path); // transient lock: next caller retries
}

// One whitespace-separated line, exactly 13 tokens; any deviation reads as None =
// "not in PiP", so torn or corrupt writes fail closed. pid is LAST: a write truncated
// mid-token yields a pid that fails the owner check, never a poisoned restore rect.
pub(crate) fn parse_state(s: &str) -> Option<PipState> {
    let t: Vec<&str> = s.split_whitespace().collect();
    let [hwnd, x, y, w, h, style, ex_style, target_w, target_h, corner, margin, min, pid] = t[..]
    else {
        return None;
    };
    Some(PipState {
        hwnd: hwnd.parse().ok()?,
        x: x.parse().ok()?,
        y: y.parse().ok()?,
        w: w.parse().ok()?,
        h: h.parse().ok()?,
        style: style.parse().ok()?,
        ex_style: ex_style.parse().ok()?,
        target_w: target_w.parse().ok()?,
        target_h: target_h.parse().ok()?,
        corner: Corner::parse(corner),
        margin: margin.parse().ok()?,
        min: match min {
            "1" => true,
            "0" => false,
            _ => return None,
        },
        pid: pid.parse().ok()?,
    })
}

pub(crate) fn write_state(s: &PipState) -> String {
    format!(
        "{} {} {} {} {} {} {} {} {} {} {} {} {}",
        s.hwnd, s.x, s.y, s.w, s.h, s.style, s.ex_style,
        s.target_w, s.target_h, s.corner.as_str(), s.margin, s.min as u8, s.pid
    )
}

// ---- status JSON (write-only; smoke-test.ps1 parses it - shape is frozen) -----------

pub struct StatusInfo {
    pub hwnd: i64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub caption: bool,
    pub topmost: bool,
    pub in_pip: bool,
    pub minimal: bool,
}

pub fn status_json(s: Option<&StatusInfo>) -> String {
    match s {
        None => r#"{"found":false}"#.to_string(),
        Some(s) => format!(
            r#"{{"found":true,"hwnd":{},"x":{},"y":{},"w":{},"h":{},"caption":{},"topmost":{},"inPip":{},"minimal":{}}}"#,
            s.hwnd, s.x, s.y, s.w, s.h, s.caption, s.topmost, s.in_pip, s.minimal
        ),
    }
}
