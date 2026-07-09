use std::path::{Path, PathBuf};

use crate::geometry::Corner;

// The PiP state: x..ex_style restore the window; target_w..min are the options in
// effect at Enter (daemon and one-shot CLI converge on the same geometry); pid guards
// against HWND recycling. A VALID file on disk == "in PiP".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipState {
    pub hwnd: isize,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub style: isize,
    pub ex_style: isize,
    pub target_w: i32,
    pub target_h: i32,
    pub corner: Corner,
    pub margin: i32,
    pub min: bool,
    pub pid: u32,
}

/// %TEMP%\{name} for every IPC file (all five SPEC section 6 names live in this file;
/// pip.lua and the scripts hardcode them - frozen).
fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

pub fn state_path() -> PathBuf {
    temp_path("vlc-pip.state")
}

pub fn alive_path() -> PathBuf {
    temp_path("vlc-pip-daemon.alive")
}

pub fn crash_path() -> PathBuf {
    temp_path("vlc-pip-crash.txt")
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

// One whitespace line of exactly 13 tokens; any deviation reads as None. The trailing
// newline is the torn-write sentinel: a truncated write can never parse (SPEC 6.1).
pub(crate) fn parse_state(s: &str) -> Option<PipState> {
    if !s.ends_with('\n') {
        return None;
    }
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
        // same pin as options::parse_options: a hand-edited target outside 1..=16384
        // must read as no-state, not reach the converger
        target_w: target_w.parse().ok().filter(|&n| crate::geometry::target_ok(n))?,
        target_h: target_h.parse().ok().filter(|&n| crate::geometry::target_ok(n))?,
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
        "{} {} {} {} {} {} {} {} {} {} {} {} {}\n",
        s.hwnd, s.x, s.y, s.w, s.h, s.style, s.ex_style,
        s.target_w, s.target_h, s.corner.as_str(), s.margin, s.min as u8, s.pid
    )
}

// ---- request file (command channel into the daemon) ---------------------------------

pub fn request_path() -> PathBuf {
    temp_path("vlc-pip-request.txt")
}

pub fn status_path() -> PathBuf {
    temp_path("vlc-pip-status.json")
}

pub fn consume_request(path: &Path) -> Option<String> {
    let cmd = std::fs::read_to_string(path).ok()?; // missing or mid-write: retry next poll
    std::fs::remove_file(path).ok()?; // couldn't delete: leave the command for next poll
    let cmd = cmd.trim();
    if cmd.is_empty() { None } else { Some(cmd.to_string()) }
}
