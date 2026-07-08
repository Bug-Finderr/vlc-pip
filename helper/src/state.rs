use std::path::{Path, PathBuf};

// The PiP state: X..ex_style restore the window; target_w..min are the options in
// effect at Enter (daemon and one-shot CLI converge on the same geometry); pid guards
// against HWND recycling. A VALID file on disk == "in PiP".
#[derive(Debug, PartialEq)]
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
    pub corner: String,
    pub margin: i32,
    pub min: bool,
    pub pid: u32,
}

/// %TEMP%\{name} for every IPC file. Appends to the OsString instead of PathBuf::join:
/// join's component parsing is ~5KB of the binary, and temp_dir() already ends in a
/// separator on Windows (the last-byte check covers exotic TMP values without one).
pub fn temp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir().into_os_string();
    if !matches!(p.as_encoded_bytes().last(), Some(b'\\' | b'/')) {
        p.push(r"\");
    }
    p.push(name);
    p.into()
}

pub fn state_path() -> PathBuf {
    temp_path("vlc-pip.json")
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

// ---- hand-rolled JSON for this one flat frozen schema ------------------------------
// Strict single-pass scanner; anything malformed yields None (corrupt/torn state file
// reads as "not in PiP"). Deliberately stricter than C#: escapes, nested unknown values,
// and leading-zero numbers are rejected. v1 could in principle emit an escaped Corner
// from a hand-crafted c= value; such a file reads as stale (declared deviation (d)).

fn ws(b: &[u8], i: &mut usize) {
    while matches!(b.get(*i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        *i += 1;
    }
}

fn eat(b: &[u8], i: &mut usize, c: u8) -> Option<()> {
    ws(b, i);
    if b.get(*i) == Some(&c) {
        *i += 1;
        Some(())
    } else {
        None
    }
}

// C# never writes escapes for this schema (keys + corner are plain ASCII): reject them.
fn string<'a>(b: &'a [u8], i: &mut usize) -> Option<&'a str> {
    eat(b, i, b'"')?;
    let start = *i;
    loop {
        match b.get(*i)? {
            b'"' => {
                let s = std::str::from_utf8(&b[start..*i]).ok()?;
                *i += 1;
                return Some(s);
            }
            b'\\' | 0x00..=0x1f => return None,
            _ => *i += 1,
        }
    }
}

fn int(b: &[u8], i: &mut usize) -> Option<i64> {
    ws(b, i);
    let start = *i;
    if b.get(*i) == Some(&b'-') {
        *i += 1;
    }
    let d0 = *i;
    while matches!(b.get(*i), Some(b'0'..=b'9')) {
        *i += 1;
    }
    if *i - d0 > 1 && b[d0] == b'0' {
        return None; // leading zeros are not valid JSON
    }
    std::str::from_utf8(&b[start..*i]).ok()?.parse().ok()
}

fn lit(b: &[u8], i: &mut usize, l: &[u8]) -> bool {
    if b[*i..].starts_with(l) {
        *i += l.len();
        true
    } else {
        false
    }
}

fn boolean(b: &[u8], i: &mut usize) -> Option<bool> {
    ws(b, i);
    if lit(b, i, b"true") {
        Some(true)
    } else if lit(b, i, b"false") {
        Some(false)
    } else {
        None
    }
}

// unknown keys: skip any scalar value (C# ignored unmapped members; keeps us upgrade-tolerant)
fn skip_value(b: &[u8], i: &mut usize) -> Option<()> {
    ws(b, i);
    match b.get(*i)? {
        b'"' => string(b, i).map(|_| ()),
        b't' | b'f' => boolean(b, i).map(|_| ()),
        b'n' => lit(b, i, b"null").then_some(()),
        b'-' | b'0'..=b'9' => {
            while matches!(b.get(*i), Some(b'+' | b'-' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
                *i += 1;
            }
            Some(())
        }
        _ => None, // nested arrays/objects are not part of this flat schema
    }
}

fn i32_field(b: &[u8], i: &mut usize) -> Option<i32> {
    i32::try_from(int(b, i)?).ok()
}

pub(crate) fn parse_state(s: &str) -> Option<PipState> {
    let b = s.as_bytes();
    let mut i = 0usize;
    eat(b, &mut i, b'{')?;

    let (mut hwnd, mut x, mut y, mut w, mut h, mut style, mut ex_style) =
        (None, None, None, None, None, None, None);
    // old 7-field files (v1.0 pre-audit) get these defaults, matching the C# record
    let (mut target_w, mut target_h, mut corner, mut margin, mut min, mut pid) =
        (480, 270, String::from("br"), 16, true, 0u32);

    loop {
        let key = string(b, &mut i)?;
        eat(b, &mut i, b':')?;
        match key {
            "Hwnd" => hwnd = Some(int(b, &mut i)?),
            "X" => x = Some(i32_field(b, &mut i)?),
            "Y" => y = Some(i32_field(b, &mut i)?),
            "W" => w = Some(i32_field(b, &mut i)?),
            "H" => h = Some(i32_field(b, &mut i)?),
            "Style" => style = Some(int(b, &mut i)?),
            "ExStyle" => ex_style = Some(int(b, &mut i)?),
            "TargetW" => target_w = i32_field(b, &mut i)?,
            "TargetH" => target_h = i32_field(b, &mut i)?,
            "Corner" => corner = string(b, &mut i)?.to_string(),
            "Margin" => margin = i32_field(b, &mut i)?,
            "Min" => min = boolean(b, &mut i)?,
            "Pid" => pid = u32::try_from(int(b, &mut i)?).ok()?,
            _ => skip_value(b, &mut i)?,
        }
        ws(b, &mut i);
        match b.get(i)? {
            b',' => i += 1,
            b'}' => {
                i += 1;
                break;
            }
            _ => return None,
        }
    }
    ws(b, &mut i);
    if i != b.len() {
        return None; // trailing garbage
    }
    Some(PipState {
        hwnd: hwnd?,
        x: x?,
        y: y?,
        w: w?,
        h: h?,
        style: style?,
        ex_style: ex_style?,
        target_w,
        target_h,
        corner,
        margin,
        min,
        pid,
    })
}

// Byte-identical to the C# System.Text.Json source-gen output. Corner is NOT escaped:
// options::normalize_corner pins it to {br,bl,tr,tl} before it ever reaches a PipState.
pub(crate) fn write_state(s: &PipState) -> String {
    format!(
        r#"{{"Hwnd":{},"X":{},"Y":{},"W":{},"H":{},"Style":{},"ExStyle":{},"TargetW":{},"TargetH":{},"Corner":"{}","Margin":{},"Min":{},"Pid":{}}}"#,
        s.hwnd, s.x, s.y, s.w, s.h, s.style, s.ex_style,
        s.target_w, s.target_h, s.corner, s.margin, s.min, s.pid
    )
}

// ---- status JSON (write-only; exact v1 shape, key order, lowercase booleans) --------

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
