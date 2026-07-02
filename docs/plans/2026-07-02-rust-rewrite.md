# VLC PiP v2 (Rust Rewrite) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite `pip-helper.exe` in Rust (windows-sys, zero other deps, ~130KB) with observable behavior byte-identical to the retired C# v1, gated by the unchanged 21-check smoke test.

**Architecture:** Single binary crate at `helper/`. Pure logic (state JSON, geometry, options, request file) in unit-tested modules; Win32 work in `native.rs` (one-shot actions + region maintenance) and `daemon.rs` (message pump, hotkey, LL hooks, heartbeat). `pip.lua`, `smoke-test.ps1`, and `uninstall.ps1` are frozen v1 artifacts - only the exe implementation and `install.ps1`'s build block change.

**Tech Stack:** Rust 1.96 stable (x86_64-pc-windows-msvc, edition 2024), windows-sys 0.61, hand-rolled JSON (measured: 44-49KB smaller than serde_json/nanoserde for this one flat schema).

## Global Constraints

- Branch: `rust-rewrite`. Commit style: `feat:`/`test:`/`docs:` like existing history. Commits are SSH-signed automatically; NEVER add Co-Authored-By lines.
- Precondition: the branch already contains the C# removal, the rewritten SPEC.md, and this plan as committed state - `git status` must be CLEAN before Task 1. If it is not, stop and ask.
- Working directory: ALL commands run from the repo root (`D:\Files\Dev\vlc-pip`). Cargo commands pass `--manifest-path helper\Cargo.toml`; git pathspecs are root-relative.
- SPEC.md at repo root is the behavioral contract. Where this plan and SPEC.md disagree, SPEC.md wins - stop and reconcile.
- windows-sys pinned `0.61` with EXACTLY these features (each is load-bearing; `Win32_Security` gates `CreateMutexW`): `Win32_Foundation`, `Win32_UI_WindowsAndMessaging`, `Win32_UI_HiDpi`, `Win32_UI_Input_KeyboardAndMouse`, `Win32_Graphics_Gdi`, `Win32_System_LibraryLoader`, `Win32_System_Threading`, `Win32_Security`, `Win32_System_Diagnostics_ToolHelp`.
- Release profile exactly: `opt-level = "z"`, `lto = true` (explicit - with `codegen-units = 1` the default performs NO LTO), `codegen-units = 1`, `panic = "abort"`, `strip = true`.
- All handles cross statics/files as `isize`/`i64`, never as raw pointers (windows-sys 0.61 handles are `*mut c_void`, not `Send`/`Sync`).
- Runtime file formats are frozen (SPEC §6): state JSON byte-compatible with C# System.Text.Json output; heartbeat `"{epoch} pid=N hotkey=X timer=X kb=X mouse=X"`; status JSON exact key order with lowercase booleans.
- Known accepted deviations from v1 (do not "fix back"): (a) `c=` values are normalized to `br|bl|tr|tl` at parse time (v1 stored raw strings but treated unknown as `br`; the hand-rolled writer does not escape, so normalization is mandatory); (b) well-formed JSON missing a required field (`Hwnd`..`ExStyle`) parses as None where C# defaulted it to 0 - stricter-on-corrupt is the safe direction (reads as "not in PiP"); (c) a state-save I/O failure makes `enter` return false (exit 1) where v1 crashed to exit 3 - nothing had been mutated yet, so failing cleanly is strictly better; (d) the state parser also rejects JSON string escapes and nested unknown values where C# skipped them - reachable only via hand-crafted v1 files (e.g. a quote inside a `c=` value), which then read as "not in PiP"; (e) failure-path exit codes: `stop` exits 1 and `status` still exits 0 when their `%TEMP%` write fails, where v1 crashed to exit 3 with a crash file; (f) on a daemon panic the crash hook deletes the alive file (restoring v1's crash-path respawn behavior) but does not unhook/unregister - the OS frees those at process death.
- `cargo test` and `cargo build` run from `helper/`. The deployed exe is ALWAYS `target\release\pip-helper.exe` - never copy from `target\debug`.

---

### Task 1: Crate scaffold

**Files:**
- Create: `helper/Cargo.toml`
- Create: `helper/src/main.rs` (stub)
- Create: `helper/src/{daemon,geometry,native,options,request,state}.rs` (empty)

**Interfaces:**
- Produces: a compiling workspace-free binary crate named `pip-helper`; module skeleton every later task fills in.

- [ ] **Step 1: Write `helper/Cargo.toml`**

```toml
[package]
name = "pip-helper"
version = "2.0.0"
edition = "2024"

[dependencies.windows-sys]
version = "0.61"
features = [
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_HiDpi",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_Graphics_Gdi",
    "Win32_System_LibraryLoader",
    "Win32_System_Threading",
    "Win32_Security",
    "Win32_System_Diagnostics_ToolHelp",
]

[profile.release]
opt-level = "z"
lto = true          # explicit: with codegen-units=1 the default (false) performs NO LTO
codegen-units = 1
panic = "abort"
strip = true
```

- [ ] **Step 2: Write `helper/src/main.rs` stub**

```rust
#![windows_subsystem = "windows"]
#![allow(dead_code)] // removed in Task 9 when main() wires every module

mod daemon;
mod geometry;
mod native;
mod options;
mod request;
mod state;

fn main() {}
```

- [ ] **Step 3: Create the six empty module files**

`helper/src/daemon.rs`, `geometry.rs`, `native.rs`, `options.rs`, `request.rs`, `state.rs` - each empty (0 bytes is fine).

- [ ] **Step 4: Build**

Run: `cargo build --manifest-path helper\Cargo.toml`
Expected: `Finished` with no errors; first run downloads windows-sys. Also creates `Cargo.lock` - commit it (binary crate: lock files are committed for reproducible builds).

- [ ] **Step 5: Commit**

```bash
git add helper/Cargo.toml helper/Cargo.lock helper/src
git commit -m "feat: scaffold pip-helper Rust crate (windows-sys 0.61, size profile)"
```

---

### Task 2: geometry.rs

**Files:**
- Modify: `helper/src/geometry.rs`

**Interfaces:**
- Produces: `pub fn compute_corner(work_left: i32, work_top: i32, work_right: i32, work_bottom: i32, w: i32, h: i32, corner: &str, margin: i32) -> (i32, i32)`

- [ ] **Step 1: Write the failing tests** (in `helper/src/geometry.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // work area 0,0..1920x1040 (taskbar excluded), 480x270, margin 16 - pinned from v1
    #[test]
    fn compute_corner_places_window_inside_work_area() {
        for (corner, ex, ey) in [("br", 1424, 754), ("bl", 16, 754), ("tr", 1424, 16), ("tl", 16, 16)] {
            assert_eq!(compute_corner(0, 0, 1920, 1040, 480, 270, corner, 16), (ex, ey), "corner {corner}");
        }
    }

    #[test]
    fn compute_corner_unknown_corner_falls_back_to_br() {
        assert_eq!(compute_corner(0, 0, 1920, 1040, 480, 270, "zz", 16), (1424, 754));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path helper\Cargo.toml geometry`
Expected: compile error - `compute_corner` not found.

- [ ] **Step 3: Implement** (above the tests module)

```rust
pub fn compute_corner(
    work_left: i32, work_top: i32, work_right: i32, work_bottom: i32,
    w: i32, h: i32, corner: &str, margin: i32,
) -> (i32, i32) {
    let left = work_left + margin;
    let top = work_top + margin;
    let right = work_right - w - margin;
    let bottom = work_bottom - h - margin;
    match corner {
        "tl" => (left, top),
        "tr" => (right, top),
        "bl" => (left, bottom),
        _ => (right, bottom), // "br" and fallback
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path helper\Cargo.toml geometry`
Expected: `2 passed`.

- [ ] **Step 5: Commit**

```bash
git add helper/src/geometry.rs
git commit -m "feat: corner geometry with br fallback"
```

---

### Task 3: options.rs

**Files:**
- Modify: `helper/src/options.rs`

**Interfaces:**
- Produces: `pub struct PipOptions { pub w: i32, pub h: i32, pub corner: &'static str, pub margin: i32, pub min: bool }` with `Default` (480, 270, "br", 16, true); `pub fn parse_options<'a>(args: impl IntoIterator<Item = &'a str>) -> PipOptions`; `pub fn normalize_corner(v: &str) -> &'static str`.

- [ ] **Step 1: Write the failing tests**

```rust
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
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path helper\Cargo.toml options`
Expected: compile error - `parse_options` not found.

- [ ] **Step 3: Implement**

```rust
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
/// four legal values at the boundary (unknown = "br", matching compute_corner's fallback).
pub fn normalize_corner(v: &str) -> &'static str {
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path helper\Cargo.toml options`
Expected: `6 passed`.

- [ ] **Step 5: Commit**

```bash
git add helper/src/options.rs
git commit -m "feat: argv option parsing with corner normalization"
```

---

### Task 4: state.rs - PipState JSON + status JSON

**Files:**
- Modify: `helper/src/state.rs`

**Interfaces:**
- Produces: `pub struct PipState { pub hwnd: i64, pub x: i32, pub y: i32, pub w: i32, pub h: i32, pub style: i64, pub ex_style: i64, pub target_w: i32, pub target_h: i32, pub corner: String, pub margin: i32, pub min: bool, pub pid: u32 }` (`Debug, PartialEq`); `pub fn parse_state(s: &str) -> Option<PipState>`; `pub fn write_state(s: &PipState) -> String`; `pub fn state_path() -> PathBuf`; `pub fn load(path: &Path) -> Option<PipState>`; `pub fn save(s: &PipState, path: &Path) -> std::io::Result<()>`; `pub fn try_delete(path: &Path)`; `pub struct StatusInfo { pub hwnd: i64, pub x: i32, pub y: i32, pub w: i32, pub h: i32, pub caption: bool, pub topmost: bool, pub in_pip: bool, pub minimal: bool }`; `pub fn status_json(s: Option<&StatusInfo>) -> String`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // byte-for-byte what C# System.Text.Json source-gen emitted (verified against a live run)
    const FULL: &str = r#"{"Hwnd":66112,"X":100,"Y":200,"W":1000,"H":640,"Style":349110272,"ExStyle":256,"TargetW":480,"TargetH":270,"Corner":"br","Margin":16,"Min":true,"Pid":12345}"#;
    // v1.0 pre-audit 7-field format, exactly as pinned in the retired C# StateTests
    const OLD: &str = r#"{"Hwnd":4660,"X":100,"Y":200,"W":1000,"H":640,"Style":349110272,"ExStyle":256}"#;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pip-state-test-{name}-{}.json", std::process::id()))
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
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path helper\Cargo.toml state`
Expected: compile error - types not found.

- [ ] **Step 3: Implement** (above the tests; this parser was verified byte-compatible against live C# System.Text.Json output during research - keep it exactly)

```rust
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

pub fn state_path() -> PathBuf {
    std::env::temp_dir().join("vlc-pip.json")
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

pub fn parse_state(s: &str) -> Option<PipState> {
    let b = s.as_bytes();
    let mut i = 0usize;
    eat(b, &mut i, b'{')?;

    let (mut hwnd, mut x, mut y, mut w, mut h, mut style, mut ex_style) =
        (None, None, None, None, None, None, None);
    // old 7-field files (v1.0 pre-audit) get these defaults, matching the C# record
    let (mut target_w, mut target_h, mut corner, mut margin, mut min, mut pid) =
        (480, 270, String::from("br"), 16, true, 0u32);

    ws(b, &mut i);
    if b.get(i) == Some(&b'}') {
        i += 1;
    } else {
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
pub fn write_state(s: &PipState) -> String {
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path helper\Cargo.toml state`
Expected: `7 passed`.

- [ ] **Step 5: Commit**

```bash
git add helper/src/state.rs
git commit -m "feat: PipState with hand-rolled C#-byte-compatible JSON, status JSON"
```

---

### Task 5: request.rs

**Files:**
- Modify: `helper/src/request.rs`

**Interfaces:**
- Produces: `pub fn request_path() -> PathBuf`; `pub fn consume(path: &Path) -> Option<String>` (read + delete + trim; None on missing/empty/any I/O error - an unreadable or undeletable file is left for the next 150ms tick).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path helper\Cargo.toml request`
Expected: compile error - `consume` not found.

- [ ] **Step 3: Implement**

```rust
use std::path::{Path, PathBuf};

pub fn request_path() -> PathBuf {
    std::env::temp_dir().join("vlc-pip-request.txt")
}

pub fn consume(path: &Path) -> Option<String> {
    let cmd = std::fs::read_to_string(path).ok()?; // missing or mid-write: retry next poll
    std::fs::remove_file(path).ok()?; // couldn't delete: leave the command for next poll
    let cmd = cmd.trim();
    if cmd.is_empty() { None } else { Some(cmd.to_string()) }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path helper\Cargo.toml request`
Expected: `3 passed`.

- [ ] **Step 5: Commit**

```bash
git add helper/src/request.rs
git commit -m "feat: request-file consume with retry-friendly error handling"
```

---

### Task 6: native.rs part 1 - find/enter/exit/toggle/status

No unit tests possible (live Win32); the gate is `cargo build` now and the 21-check smoke test in Task 11. Copy the code exactly - every flag and ordering is contract (SPEC §7).

**Files:**
- Modify: `helper/src/native.rs`

**Interfaces:**
- Consumes: `geometry::compute_corner`, `options::PipOptions`, `state::*`.
- Produces: `pub fn enable_dpi_awareness()`; `pub fn find_player() -> isize` (0 = none); `pub fn in_pip() -> bool`; `pub fn enter(h: isize, o: &PipOptions) -> bool`; `pub fn exit_pip() -> bool`; `pub fn toggle(o: &PipOptions) -> bool`; `pub fn status() -> String`; `pub fn status_path() -> PathBuf`. (Task 7 appends `RegionTracker`/`maintain_region` to this file.)

- [ ] **Step 1: Implement**

```rust
use std::path::PathBuf;

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CreateRectRgn, DeleteObject, GetMonitorInfoW, GetWindowRgn, MonitorFromWindow, SetWindowRgn,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE, HWND_NOTOPMOST,
    HWND_TOPMOST, SWP_FRAMECHANGED, SWP_SHOWWINDOW, SW_RESTORE, WS_CAPTION, WS_EX_TOPMOST,
    WS_MAXIMIZE, WS_THICKFRAME,
};

use crate::geometry;
use crate::options::PipOptions;
use crate::state::{self, PipState, StatusInfo};

// Handles live in statics and the state file, so they travel as isize (windows-sys 0.61
// handles are *mut c_void: not Send/Sync). Cast at the call boundary only.
fn hw(h: isize) -> HWND {
    h as HWND
}

pub fn enable_dpi_awareness() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

// ---- find the VLC player window ----------------------------------------------------

fn vlc_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return pids;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case("vlc.exe") {
                    pids.push(e.th32ProcessID);
                }
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    pids
}

struct FindCtx<'a> {
    pids: &'a [u32],
    best: isize,
    biggest: isize,
    biggest_area: i64,
}

unsafe extern "system" fn find_player_cb(h: HWND, l: LPARAM) -> BOOL {
    unsafe {
        let ctx = &mut *(l as *mut FindCtx);
        if IsWindowVisible(h) == 0 {
            return 1;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        if !ctx.pids.contains(&pid) {
            return 1;
        }
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(h, buf.as_mut_ptr(), 256);
        if n == 0 {
            return 1; // empty title: VLC's hidden/extension windows
        }
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        if title.to_ascii_lowercase().contains("vlc media player") {
            ctx.best = h as isize;
            return 0; // stop enumeration
        }
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut r);
        let area = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
        if area > ctx.biggest_area {
            ctx.biggest_area = area;
            ctx.biggest = h as isize;
        }
        1
    }
}

pub fn find_player() -> isize {
    let pids = vlc_pids();
    if pids.is_empty() {
        return 0;
    }
    let mut ctx = FindCtx { pids: &pids, best: 0, biggest: 0, biggest_area: 0 };
    unsafe {
        EnumWindows(Some(find_player_cb), &mut ctx as *mut FindCtx as LPARAM);
    }
    if ctx.best != 0 { ctx.best } else { ctx.biggest }
}

// ---- state ownership ----------------------------------------------------------------

// Windows recycles HWND values: after VLC dies, the saved handle can belong to another
// app. IsWindow alone would pass and we'd reshape a foreign window; require the owner
// PID recorded at Enter. Old state files (Pid=0) read as stale by design.
fn owns_state(s: &PipState) -> bool {
    unsafe {
        if IsWindow(hw(s.hwnd as isize)) == 0 {
            return false;
        }
        let mut p = 0u32;
        GetWindowThreadProcessId(hw(s.hwnd as isize), &mut p);
        p != 0 && p == s.pid
    }
}

pub fn in_pip() -> bool {
    let path = state::state_path();
    match state::load(&path) {
        None => false,
        Some(s) if !owns_state(&s) => {
            state::try_delete(&path); // stale: VLC gone or hwnd recycled
            false
        }
        Some(_) => true,
    }
}

// ---- enter / exit / toggle ----------------------------------------------------------

fn work_area(h: isize) -> RECT {
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(hw(h), MONITOR_DEFAULTTONEAREST), &mut mi);
        mi.rcWork
    }
}

pub fn enter(h: isize, o: &PipOptions) -> bool {
    if h == 0 || in_pip() {
        return false;
    }
    unsafe {
        if IsIconic(hw(h)) != 0 {
            ShowWindow(hw(h), SW_RESTORE); // else the off-screen iconic rect gets saved as the restore state
        }
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hw(h), &mut r);
        let style = GetWindowLongPtrW(hw(h), GWL_STYLE);
        let ex = GetWindowLongPtrW(hw(h), GWL_EXSTYLE);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hw(h), &mut pid);
        let s = PipState {
            hwnd: h as i64,
            x: r.left,
            y: r.top,
            w: r.right - r.left,
            h: r.bottom - r.top,
            style: style as i64,
            ex_style: ex as i64,
            target_w: o.w,
            target_h: o.h,
            corner: o.corner.to_string(),
            margin: o.margin,
            min: o.min,
            pid,
        };
        if state::save(&s, &state::state_path()).is_err() {
            return false; // nothing mutated yet: fail cleanly, retry next toggle
        }

        // also strip WS_MAXIMIZE: a zoomed window keeps IsZoomed, so Win+Down/Aero would
        // snap the PiP back to Qt's normal placement rect
        SetWindowLongPtrW(hw(h), GWL_STYLE, style & !((WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZE) as isize));
        let wa = work_area(h);
        let (x, y) = geometry::compute_corner(wa.left, wa.top, wa.right, wa.bottom, o.w, o.h, o.corner, o.margin);
        let ok = SetWindowPos(hw(h), HWND_TOPMOST, x, y, o.w, o.h, SWP_FRAMECHANGED | SWP_SHOWWINDOW) != 0;
        if !ok {
            // e.g. UIPI vs elevated VLC: don't claim in-PiP
            SetWindowLongPtrW(hw(h), GWL_STYLE, style);
            state::try_delete(&state::state_path());
        }
        ok
    }
}

pub fn exit_pip() -> bool {
    let path = state::state_path();
    let Some(s) = state::load(&path) else { return false };
    if !owns_state(&s) {
        state::try_delete(&path); // stale: VLC gone or hwnd recycled
        return false;
    }
    let h = s.hwnd as isize;
    unsafe {
        SetWindowRgn(hw(h), std::ptr::null_mut(), 1); // drop the minimal-look clip before restoring
        SetWindowLongPtrW(hw(h), GWL_STYLE, s.style as isize);
        SetWindowLongPtrW(hw(h), GWL_EXSTYLE, s.ex_style as isize);
        // WS_EX_TOPMOST only changes via SetWindowPos: honor the user's own always-on-top
        let after = if s.ex_style & (WS_EX_TOPMOST as i64) != 0 { HWND_TOPMOST } else { HWND_NOTOPMOST };
        let ok = SetWindowPos(hw(h), after, s.x, s.y, s.w, s.h, SWP_FRAMECHANGED | SWP_SHOWWINDOW) != 0;
        if ok || IsWindow(hw(h)) == 0 {
            state::try_delete(&path); // live-window restore failure keeps state so the next toggle retries
        }
        ok
    }
}

pub fn toggle(o: &PipOptions) -> bool {
    if in_pip() { exit_pip() } else { enter(find_player(), o) }
}

// ---- status -------------------------------------------------------------------------

pub fn status_path() -> PathBuf {
    std::env::temp_dir().join("vlc-pip-status.json")
}

pub fn status() -> String {
    let h = find_player();
    if h == 0 {
        return state::status_json(None);
    }
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hw(h), &mut r);
        let style = GetWindowLongPtrW(hw(h), GWL_STYLE);
        let ex = GetWindowLongPtrW(hw(h), GWL_EXSTYLE);
        state::status_json(Some(&StatusInfo {
            hwnd: h as i64,
            x: r.left,
            y: r.top,
            w: r.right - r.left,
            h: r.bottom - r.top,
            caption: style & (WS_CAPTION as isize) == (WS_CAPTION as isize), // BOTH bits
            topmost: ex & (WS_EX_TOPMOST as isize) != 0,
            in_pip: in_pip(),
            minimal: has_region(h),
        }))
    }
}

fn has_region(h: isize) -> bool {
    unsafe {
        let probe = CreateRectRgn(0, 0, 0, 0);
        let r = GetWindowRgn(hw(h), probe) != 0; // 0 = ERROR (no region)
        DeleteObject(probe);
        r
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build --manifest-path helper\Cargo.toml`
Expected: success; exactly one warning - `unused imports: EnumChildWindows and GetClassNameW` - which is fine, Task 7's appended code consumes them. (`has_region` is used by `status` now and by Task 7's region code.)

- [ ] **Step 3: Run all tests (regression)**

Run: `cargo test --manifest-path helper\Cargo.toml`
Expected: `18 passed` total across modules (geometry 2, options 6, state 7, request 3), 0 failed.

- [ ] **Step 4: Commit**

```bash
git add helper/src/native.rs
git commit -m "feat: Win32 find/enter/exit/toggle/status with PID-guarded state"
```

---

### Task 7: native.rs part 2 - minimal-look region maintenance

**Files:**
- Modify: `helper/src/native.rs` (append)

**Interfaces:**
- Consumes: everything from Task 6.
- Produces: `pub struct RegionTracker` with `pub fn new() -> Self`; `pub fn maintain_region(t: &mut RegionTracker)`. (v1 used C# statics for the cross-tick rects; a caller-owned tracker is the Rust-idiomatic equivalent - the daemon owns one across ticks, the one-shot loop owns one across its 6 iterations. Observable behavior identical.)

- [ ] **Step 1: Append the region code to `helper/src/native.rs`**

Add `FindVideoChild`'s callback context + the region logic at the end of the file:

```rust
// ---- minimal look (Ctrl+H-like) via SetWindowRgn on the video child area -------------
// VLC 3.x hosts the video in a native child whose class starts with "VLC video main".

struct ChildCtx {
    found: isize,
}

unsafe extern "system" fn find_child_cb(c: HWND, l: LPARAM) -> BOOL {
    unsafe {
        let ctx = &mut *(l as *mut ChildCtx);
        if IsWindowVisible(c) == 0 {
            return 1;
        }
        let mut buf = [0u16; 128];
        let n = GetClassNameW(c, buf.as_mut_ptr(), 128);
        if String::from_utf16_lossy(&buf[..n as usize]).starts_with("VLC video main") {
            ctx.found = c as isize;
            return 0;
        }
        1
    }
}

fn find_video_child(top: isize) -> isize {
    let mut ctx = ChildCtx { found: 0 };
    unsafe {
        EnumChildWindows(hw(top), Some(find_child_cb), &mut ctx as *mut ChildCtx as LPARAM);
    }
    ctx.found
}

fn same_rect(a: &RECT, b: &RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

// Cross-tick measurement memory for the stability debounce; v1 kept these in statics.
pub struct RegionTracker {
    prev_win: RECT,
    prev_child: RECT,
    have_prev: bool,
}

impl RegionTracker {
    pub fn new() -> Self {
        unsafe { Self { prev_win: std::mem::zeroed(), prev_child: std::mem::zeroed(), have_prev: false } }
    }
}

/// Converging per-tick maintenance, called by the daemon timer (and one-shot enter):
/// no video -> clear region; video child not yet at target size -> resize window with
/// chrome compensation; child at target -> clip window to the video area. Geometry
/// targets come from the state file (recorded at Enter), so daemon and one-shot agree.
/// Acts only on STABLE frames (window+child rects unchanged since the previous tick):
/// VLC re-fits the child asynchronously after our resize, so a fresh measurement can be
/// stale and yield garbage chrome (observed in v1: perpetual resize thrash).
pub fn maintain_region(t: &mut RegionTracker) {
    let path = state::state_path();
    let Some(s) = state::load(&path) else {
        t.have_prev = false;
        return;
    };
    if !owns_state(&s) {
        t.have_prev = false;
        state::try_delete(&path); // stale: VLC gone or hwnd recycled
        return;
    }
    if !s.min {
        return;
    }
    let h = s.hwnd as isize;

    let child = find_video_child(h);
    unsafe {
        if child == 0 {
            t.have_prev = false;
            if has_region(h) {
                SetWindowRgn(hw(h), std::ptr::null_mut(), 1); // playback stopped: show full mini UI
            }
            return;
        }

        let mut wr: RECT = std::mem::zeroed();
        let mut cr: RECT = std::mem::zeroed();
        GetWindowRect(hw(h), &mut wr);
        GetWindowRect(hw(child), &mut cr);
        let stable = t.have_prev && same_rect(&wr, &t.prev_win) && same_rect(&cr, &t.prev_child);
        t.prev_win = wr;
        t.prev_child = cr;
        t.have_prev = true;
        if !stable {
            return; // wait until VLC's re-layout settles
        }

        let rel_l = cr.left - wr.left;
        let rel_t = cr.top - wr.top;
        let cw = cr.right - cr.left;
        let ch = cr.bottom - cr.top;
        let chrome_w = (wr.right - wr.left) - cw;
        let chrome_h = (wr.bottom - wr.top) - ch;
        // real chrome (menu + controller + borders) is well under 300px; negative or huge
        // delta = stale rects from VLC's async re-layout
        if !(0..=300).contains(&chrome_w) || !(0..=300).contains(&chrome_h) {
            return;
        }

        if (cw - s.target_w).abs() > 2 || (ch - s.target_h).abs() > 2 {
            // chrome = window minus video child; grow the window so the video itself is WxH
            let wa = work_area(h);
            let (vx, vy) = geometry::compute_corner(
                wa.left, wa.top, wa.right, wa.bottom, s.target_w, s.target_h, &s.corner, s.margin,
            );
            let (tw, th, tx, ty) = (s.target_w + chrome_w, s.target_h + chrome_h, vx - rel_l, vy - rel_t);
            if tw <= 0 || th <= 0 {
                return;
            }
            if wr.left != tx || wr.top != ty || wr.right - wr.left != tw || wr.bottom - wr.top != th {
                SetWindowPos(hw(h), HWND_TOPMOST, tx, ty, tw, th, SWP_FRAMECHANGED);
                t.have_prev = false; // our own resize invalidates the measurement
            }
            return;
        }

        if !has_region(h) {
            let rgn = CreateRectRgn(rel_l, rel_t, rel_l + cw, rel_t + ch);
            if SetWindowRgn(hw(h), rgn, 1) == 0 {
                DeleteObject(rgn); // system owns rgn only on success
            }
        }
    }
}
```

- [ ] **Step 2: Build + regression tests**

Run: `cargo build --manifest-path helper\Cargo.toml && cargo test --manifest-path helper\Cargo.toml`
Expected: build success; `18 passed`.

- [ ] **Step 3: Commit**

```bash
git add helper/src/native.rs
git commit -m "feat: minimal-look region maintenance with two-tick debounce and chrome clamp"
```

---

### Task 8: daemon.rs - pump, hotkey, hooks, heartbeat

**Files:**
- Modify: `helper/src/daemon.rs`

**Interfaces:**
- Consumes: `native::{toggle, enter, exit_pip, find_player, maintain_region, RegionTracker}`, `request::{consume, request_path}`, `state::{load, state_path}`, `options::PipOptions`.
- Produces: `pub fn run(o: &PipOptions) -> i32` (always 0); `pub fn owns_alive_file() -> bool` (consumed by main.rs's panic hook in Task 9).

Constraints baked into this code (SPEC §3, §7): hooks NEVER touch the disk - they read `CACHED_HWND`, refreshed only on the pump thread (hook callbacks dispatch on that thread; atomics with `Relaxed` are used purely because Rust statics require `Sync`). The heartbeat write failure must never kill the pump.

- [ ] **Step 1: Implement**

```rust
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering::Relaxed};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    VK_F, VK_P,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetAncestor, GetForegroundWindow, GetMessageW,
    GetSystemMetrics, IsWindow, PostQuitMessage, SetTimer, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    SM_CXDOUBLECLK, SM_CYDOUBLECLK, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_HOTKEY, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_SYSKEYDOWN, WM_TIMER,
};

use crate::options::PipOptions;
use crate::{native, request, state};

// Read replica of the state file for the LL hooks: disk I/O inside a hook callback risks
// the LowLevelHooksTimeout, after which Windows SILENTLY removes the hook and the
// fullscreen-block guarantee dies with no error. Refreshed only on the pump thread; hook
// callbacks dispatch on that same thread. 0 = not in PiP. Read-only by design: stale-file
// DELETION stays in native (toggle paths + maintain_region tick).
static CACHED_HWND: AtomicIsize = AtomicIsize::new(0);
static KB_HOOK: AtomicIsize = AtomicIsize::new(0);
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);
// click rate-limit bookkeeping: the last ALLOWED button-down (time in u32 hook ms, wraps)
static LAST_ALLOWED_TIME: AtomicU32 = AtomicU32::new(0);
static LAST_ALLOWED_X: AtomicI32 = AtomicI32::new(0);
static LAST_ALLOWED_Y: AtomicI32 = AtomicI32::new(0);
static SWALLOW_NEXT_UP: AtomicBool = AtomicBool::new(false);
// true while this process owns the heartbeat file; the panic hook checks it so a daemon
// crash deletes the heartbeat (v1's finally did) and pip.lua respawns immediately instead
// of treating the dead daemon as alive for up to 15s
static OWNS_ALIVE_FILE: AtomicBool = AtomicBool::new(false);

pub fn owns_alive_file() -> bool {
    OWNS_ALIVE_FILE.load(Relaxed)
}

fn refresh_state() {
    let h = state::load(&state::state_path())
        .map(|s| s.hwnd as isize)
        .filter(|&h| unsafe { IsWindow(h as HWND) } != 0)
        .unwrap_or(0);
    CACHED_HWND.store(h, Relaxed);
}

pub fn run(o: &PipOptions) -> i32 {
    unsafe {
        // single instance; second instance exits 0 before touching any file
        let name: Vec<u16> = "VlcPipDaemon\0".encode_utf16().collect();
        let mutex = CreateMutexW(std::ptr::null(), 1, name.as_ptr()); // held for process lifetime
        if mutex.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
            return 0; // already running, or the name is unobtainable: never double-run
        }

        // discard a stale pre-launch "stop" ('pip-helper stop' with no daemon alive leaves
        // one that would kill us on the first tick); only "stop", so a queued toggle survives
        let rp = request::request_path();
        if let Ok(c) = std::fs::read_to_string(&rp) {
            if c.trim() == "stop" {
                let _ = std::fs::remove_file(&rp);
            }
        }

        let hot = RegisterHotKey(std::ptr::null_mut(), 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_P as u32) != 0;
        let timer = SetTimer(std::ptr::null_mut(), 0, 150, None) != 0; // WM_TIMER -> thread queue
        let module = GetModuleHandleW(std::ptr::null());
        KB_HOOK.store(SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0) as isize, Relaxed);
        MOUSE_HOOK.store(SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0) as isize, Relaxed);

        // Heartbeat, not a marker: a force-killed daemon can't delete the file, so consumers
        // (pip.lua) check the leading epoch-seconds for freshness. Also carries arming
        // diagnostics. Write failures are swallowed: NEVER let the heartbeat kill the pump.
        let alive = std::env::temp_dir().join("vlc-pip-daemon.alive");
        let beat = |last: &mut Instant| {
            *last = Instant::now();
            let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let _ = std::fs::write(&alive, format!(
                "{epoch} pid={} hotkey={} timer={} kb={} mouse={}",
                std::process::id(),
                hot as i32,
                timer as i32,
                (KB_HOOK.load(Relaxed) != 0) as i32,
                (MOUSE_HOOK.load(Relaxed) != 0) as i32,
            ));
        };
        OWNS_ALIVE_FILE.store(true, Relaxed);
        let mut last_beat = Instant::now();
        beat(&mut last_beat);
        refresh_state(); // a daemon restarted while already in PiP must be guarded from the first message

        let mut tracker = native::RegionTracker::new();
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                native::toggle(o);
                refresh_state();
            } else if msg.message == WM_TIMER {
                if last_beat.elapsed() > Duration::from_millis(3000) {
                    beat(&mut last_beat);
                }
                poll_request(o);
                refresh_state(); // the hook cache must reflect a request-triggered toggle within this tick
                native::maintain_region(&mut tracker);
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let kb = KB_HOOK.load(Relaxed);
        if kb != 0 {
            UnhookWindowsHookEx(kb as _);
        }
        let mouse = MOUSE_HOOK.load(Relaxed);
        if mouse != 0 {
            UnhookWindowsHookEx(mouse as _);
        }
        UnregisterHotKey(std::ptr::null_mut(), 1);
        let _ = std::fs::remove_file(&alive);
        OWNS_ALIVE_FILE.store(false, Relaxed);
    }
    0
}

fn poll_request(o: &PipOptions) {
    match request::consume(&request::request_path()).as_deref() {
        Some("toggle") => {
            native::toggle(o);
        }
        Some("enter") => {
            native::enter(native::find_player(), o);
        }
        Some("exit") => {
            native::exit_pip();
        }
        Some("stop") => unsafe { PostQuitMessage(0) },
        _ => {}
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && (wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN) {
            let k = &*(lparam as *const KBDLLHOOKSTRUCT);
            let h = CACHED_HWND.load(Relaxed);
            if k.vkCode == VK_F as u32 && h != 0 && GetForegroundWindow() as isize == h {
                return 1; // swallow F -> no fullscreen while in PiP
            }
        }
        CallNextHookEx(KB_HOOK.load(Relaxed) as _, code, wparam, lparam)
    }
}

// Rate-limit clicks over the PiP window: swallow every button-down within double-click
// time+rect of the last ALLOWED button-down, so no two clicks the OS actually receives
// can ever pair into a synthesized WM_LBUTTONDBLCLK. (v1 bug: swallowing only the 2nd
// click let a TRIPLE click through - the OS paired clicks 1+3 and VLC fullscreened.)
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            if wparam as u32 == WM_LBUTTONDOWN {
                let m = &*(lparam as *const MSLLHOOKSTRUCT);
                let h = CACHED_HWND.load(Relaxed);
                if h != 0 && GetAncestor(WindowFromPoint(m.pt), GA_ROOT) as isize == h {
                    let burst = m.time.wrapping_sub(LAST_ALLOWED_TIME.load(Relaxed)) <= GetDoubleClickTime()
                        && (m.pt.x - LAST_ALLOWED_X.load(Relaxed)).abs() <= GetSystemMetrics(SM_CXDOUBLECLK)
                        && (m.pt.y - LAST_ALLOWED_Y.load(Relaxed)).abs() <= GetSystemMetrics(SM_CYDOUBLECLK);
                    if burst {
                        SWALLOW_NEXT_UP.store(true, Relaxed);
                        return 1;
                    }
                    LAST_ALLOWED_TIME.store(m.time, Relaxed);
                    LAST_ALLOWED_X.store(m.pt.x, Relaxed);
                    LAST_ALLOWED_Y.store(m.pt.y, Relaxed);
                }
            } else if wparam as u32 == WM_LBUTTONUP && SWALLOW_NEXT_UP.load(Relaxed) {
                SWALLOW_NEXT_UP.store(false, Relaxed);
                return 1; // keep the input stream paired: drop the up of a dropped down
            }
        }
        CallNextHookEx(MOUSE_HOOK.load(Relaxed) as _, code, wparam, lparam)
    }
}
```

Note: v1 wrapped its tick in `catch (IOException)` - in Rust every file operation here already returns a handled `Result`, so there is no equivalent to add; a genuine panic is meant to reach the crash handler, same as v1's non-IO exceptions.

- [ ] **Step 2: Build + regression tests**

Run: `cargo build --manifest-path helper\Cargo.toml && cargo test --manifest-path helper\Cargo.toml`
Expected: build success; `18 passed`.

- [ ] **Step 3: Commit**

```bash
git add helper/src/daemon.rs
git commit -m "feat: daemon pump with hotkey, LL hooks, click rate-limit, heartbeat"
```

---

### Task 9: main.rs wiring, panic hook, release build

**Files:**
- Modify: `helper/src/main.rs` (replace the stub)

**Interfaces:**
- Consumes: everything.
- Produces: the complete CLI (SPEC §5.2): modes `toggle|enter|exit|status|daemon|stop`, exit codes 0/1/2/3, one-shot region convergence, crash file.

- [ ] **Step 1: Replace `helper/src/main.rs`**

```rust
#![windows_subsystem = "windows"]

mod daemon;
mod geometry;
mod native;
mod options;
mod request;
mod state;

fn main() {
    // GUI-subsystem exe: a panic is otherwise invisible. Location (file:line) survives
    // strip; exit 3 matches v1's crash exit code. The hook itself must never panic.
    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let msg = info.payload_as_str().unwrap_or("panic");
        let _ = std::fs::write(
            std::env::temp_dir().join("vlc-pip-crash.txt"),
            format!("panic at {loc}: {msg}"),
        );
        if daemon::owns_alive_file() {
            // a crashed daemon must not leave a fresh heartbeat: pip.lua would treat it as
            // alive for up to 15s and drop menu toggles (v1 deleted it in its finally block)
            let _ = std::fs::remove_file(std::env::temp_dir().join("vlc-pip-daemon.alive"));
        }
        std::process::exit(3);
    }));
    std::process::exit(run());
}

fn run() -> i32 {
    native::enable_dpi_awareness();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map_or_else(|| "toggle".to_string(), |s| s.to_lowercase());
    let o = options::parse_options(args.iter().skip(1).map(String::as_str));
    match mode.as_str() {
        "toggle" => one_shot(native::toggle(&o), &o),
        "enter" => one_shot(native::enter(native::find_player(), &o), &o),
        "exit" => {
            if native::exit_pip() { 0 } else { 1 }
        }
        "status" => {
            let s = native::status();
            println!("{s}"); // visible only when stdout is a real pipe (GUI subsystem)
            let _ = std::fs::write(native::status_path(), &s); // the reliable channel for scripts
            0
        }
        "daemon" => daemon::run(&o),
        "stop" => {
            if std::fs::write(request::request_path(), "stop").is_ok() { 0 } else { 1 }
        }
        _ => {
            eprintln!("unknown mode: {mode}");
            2
        }
    }
}

// one-shot (no daemon ticks): converge the minimal-look region here, sleeps are harmless
fn one_shot(ok: bool, o: &options::PipOptions) -> i32 {
    if ok && o.min && native::in_pip() {
        // min=0 makes maintain_region a no-op: skip the pure sleep
        let mut tracker = native::RegionTracker::new();
        for _ in 0..6 {
            // debounce needs ~4 ticks: measure, resize, measure, region
            std::thread::sleep(std::time::Duration::from_millis(150));
            native::maintain_region(&mut tracker);
        }
    }
    if ok { 0 } else { 1 }
}
```

- [ ] **Step 2: Full test + release build**

Run: `cargo test --manifest-path helper\Cargo.toml && cargo build --release --manifest-path helper\Cargo.toml`
Expected: `18 passed`; release build succeeds with no `dead_code` warnings (the `#![allow(dead_code)]` is gone with the stub). If any dead-code warning appears, something is unwired - fix, don't re-allow.

- [ ] **Step 3: Check the binary**

Run: `Get-Item helper\target\release\pip-helper.exe | Select-Object Length`
Expected: ~165,000 bytes (this exact code measured 165,376 when assembled and built during plan verification); investigate if it exceeds ~250KB.

- [ ] **Step 4: Sanity-run the status mode**

GUI-subsystem exes detach from the shell: `& $exe` returns immediately, sets no `$LASTEXITCODE`, and races the file read. Use `Start-Process -Wait`:

```powershell
$p = Start-Process helper\target\release\pip-helper.exe -ArgumentList status -Wait -PassThru
$p.ExitCode
Get-Content $env:TEMP\vlc-pip-status.json
```

Expected: exit code `0`, then `{"found":false}` (VLC closed) or a full status object (VLC open). No console window appears.

- [ ] **Step 5: Commit**

```bash
git add helper/src/main.rs
git commit -m "feat: CLI wiring, panic-to-crash-file hook, one-shot region convergence"
```

---

### Task 10: install.ps1 build block + README

**Files:**
- Modify: `scripts/install.ps1` (lines 1-12 area: dotnet/vswhere → cargo)
- Modify: `README.md` (install section + notes)

**Interfaces:**
- Consumes: `helper/target/release/pip-helper.exe` produced by `cargo build --release`.
- Produces: install pipeline for the Rust exe; docs matching reality. Apart from the build block and the Copy-Item source path (Step 2), everything (daemon stop, request cleanup, pip.lua copy, shortcut, start, alive-wait) stays byte-identical.

- [ ] **Step 1: Replace the build block in `scripts/install.ps1`**

Old (delete all of this):

```powershell
$dotnet = "$env:LOCALAPPDATA\Microsoft\dotnet\dotnet.exe"
if (-not (Test-Path $dotnet)) { $dotnet = "dotnet" }
# NativeAOT: the ILCompiler locates MSVC link.exe via a bare vswhere.exe call, so the VS
# Installer dir must be on PATH (build machine only; the produced exe has no dependencies)
$vsInstaller = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer"
if (Test-Path "$vsInstaller\vswhere.exe") { $env:PATH = "$vsInstaller;$env:PATH" }

& $dotnet publish "$root\helper" -c Release -r win-x64 -o "$root\publish"
if ($LASTEXITCODE -ne 0) { throw "publish failed" }
```

New (in its place; rustc locates MSVC link.exe itself - no vswhere/PATH tricks):

```powershell
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw "cargo not found - install Rust (MSVC toolchain) from https://rustup.rs" }
cargo build --release --manifest-path "$root\helper\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw "build failed" }
```

- [ ] **Step 2: Update the copy source**

Old: `Copy-Item "$root\publish\pip-helper.exe" "$pipDir\pip-helper.exe" -Force`
New: `Copy-Item "$root\helper\target\release\pip-helper.exe" "$pipDir\pip-helper.exe" -Force`

- [ ] **Step 3: Update `README.md`**

- Install section paragraph: replace the NativeAOT sentence with: `Builds pip-helper.exe with Rust (~130KB, no runtime dependency; building needs the Rust MSVC toolchain from https://rustup.rs plus Visual Studio Build Tools), then installs:` (table stays).
- Notes section: no changes needed (crash file, PID guard, ANSI codepage, SmartScreen notes all still true).
- Test section: add `cargo test` alongside the smoke test:

```powershell
cargo test --manifest-path helper\Cargo.toml
powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1
```

- [ ] **Step 4: Commit**

```bash
git add scripts/install.ps1 README.md
git commit -m "docs: cargo build pipeline in install.ps1 and README"
```

---

### Task 11: End-to-end gate

**Files:** none (verification only). Preconditions: VLC installed, VLC **closed**, no important playback running.

- [ ] **Step 1: Install**

Run: `powershell -ExecutionPolicy Bypass -File scripts\install.ps1`
Expected: build + `Installed. Restart VLC to see View > PiP Mode. Hotkey: Ctrl+Alt+P`. Verify `%TEMP%\vlc-pip-daemon.alive` matches `<10-digit epoch> pid=NNN hotkey=1 timer=1 kb=1 mouse=1` (in mid-2026 the epoch starts with `1782`; any flag showing 0 means that facility failed to arm - stop and investigate).

- [ ] **Step 2: Smoke test (the acceptance gate)**

Run: `powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1`
Expected: **ALL PASS** (21/21), exit 0. This spawns VLC on `screen://`, drives enter/exit via request file and hotkey, spam-clicks the PiP, and checks exact rect restore. Any FAIL = stop and debug against SPEC §7 before proceeding; do not weaken the test.

- [ ] **Step 3: Unit tests once more (clean tree)**

Run: `cargo test --manifest-path helper\Cargo.toml`
Expected: `18 passed`.

- [ ] **Step 4: Manual checklist (requires a human; record results, do not skip silently)**

- View → "PiP Mode" appears after a VLC restart; menu clicks alternate enter/exit with no console flash.
- F key swallowed only while in PiP with VLC focused; works normally otherwise.
- Stop playback while in PiP → mini UI appears (region cleared); resume → video re-clips.
- Old-state upgrade: while v1 state semantics are long gone, a leftover v1 `%TEMP%\vlc-pip.json` (Pid=0) must simply be treated as stale - one extra toggle, no crash.

- [ ] **Step 5: Commit anything the gate forced you to fix, then hand off**

If Steps 1-3 pass and Step 4 is confirmed: the branch is ready for merge + a `v2.0.0` release (tag signed; release checklist mirrors v1: exe + zip with pip.lua). Version bump rationale: identical behavior but the build toolchain requirement changes completely - breaking for from-source users.

---

## Notes for the executor

- Every constant, flag combination, and call ordering above was extracted from the v1 C# source (see git tag `v1.0.0`) and cross-checked against the windows-sys 0.61.2 crate source; the JSON parser/writer was verified byte-compatible against live C# System.Text.Json output during research (2026-07-02).
- Unit-test total is 18: geometry 2, options 6, state 7, request 3.
- If `cargo build` fails on a windows-sys import, check SPEC §8 R5 first (module surprises), then docs.rs for 0.61 - do not guess module paths.
