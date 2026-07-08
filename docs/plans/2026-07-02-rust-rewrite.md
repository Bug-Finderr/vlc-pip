# VLC PiP v2 (Rust Rewrite) Implementation Plan

> Executed 2026-07-02 and shipped as v2.0.0. The original plan carried full code listings for every task; those are preserved in git history (commit `242472c`) and superseded by the shipped source at `helper/src/`. What remains here is the research trail and the decisions.

**Goal:** Rewrite `pip-helper.exe` in Rust (windows-sys, zero other deps, ~130KB) with observable behavior byte-identical to the retired C# v1, gated by the unchanged 21-check smoke test.

**Architecture:** Single binary crate at `helper/`. Pure logic (state JSON, geometry, options, request file) in unit-tested modules; Win32 work in `native.rs` (one-shot actions + region maintenance) and `daemon.rs` (message pump, hotkey, LL hooks, heartbeat). `pip.lua`, `smoke-test.ps1`, and `uninstall.ps1` are frozen v1 artifacts - only the exe implementation and `install.ps1`'s build block change.

**Tech stack:** Rust 1.96 stable (x86_64-pc-windows-msvc, edition 2024), windows-sys 0.61, hand-rolled JSON.

## Research trail (do not re-derive)

- **Hand-rolled JSON size spike** (measured 2026-07-02): 125,440 B exe vs 169,472 B (serde_json) vs 174,080 B (nanoserde) - the crates add ~26-28% for one flat frozen schema. The writer does not escape strings, so `c=` values are normalized to `br|bl|tr|tl` at parse time.
- **windows-sys pinned `0.61`** with exactly these features, each load-bearing (`Win32_Security` gates `CreateMutexW`'s `SECURITY_ATTRIBUTES` param): `Win32_Foundation`, `Win32_UI_WindowsAndMessaging`, `Win32_UI_HiDpi`, `Win32_UI_Input_KeyboardAndMouse`, `Win32_Graphics_Gdi`, `Win32_System_LibraryLoader`, `Win32_System_Threading`, `Win32_Security`, `Win32_System_Diagnostics_ToolHelp`.
- **Release profile exactly**: `opt-level = "z"`, `lto = true` (explicit - with `codegen-units = 1` the default `false` performs NO LTO), `codegen-units = 1`, `panic = "abort"`, `strip = true`.
- Handles cross statics/files as `isize`/`i64`, never raw pointers (windows-sys 0.61 handles are `*mut c_void`, not `Send`/`Sync`).
- Runtime file formats frozen (SPEC §6): state JSON byte-compatible with C# System.Text.Json output (parser and writer verified against live v1 output during research); heartbeat `"{epoch} pid=N hotkey=X timer=X kb=X mouse=X"`; status JSON exact key order, lowercase booleans.
- Every constant, flag combination, and call ordering was extracted from the v1 C# source (tag `v1.0.0`) and cross-checked against the windows-sys 0.61.2 crate source. On a windows-sys import failure, check SPEC §8 R5 (module surprises), then docs.rs - never guess module paths.

## Accepted deviations from v1 (do not "fix back")

(a) `c=` values are normalized to `br|bl|tr|tl` at parse time (v1 stored raw strings but treated unknown as `br`; the hand-rolled writer does not escape, so normalization is mandatory); (b) well-formed JSON missing a required field (`Hwnd`..`ExStyle`) parses as None where C# defaulted it to 0 - stricter-on-corrupt is the safe direction (reads as "not in PiP"); (c) a state-save I/O failure makes `enter` return false (exit 1) where v1 crashed to exit 3 - nothing had been mutated yet, so failing cleanly is strictly better; (d) the state parser also rejects JSON string escapes and nested unknown values where C# skipped them - reachable only via hand-crafted v1 files, which then read as "not in PiP"; (e) failure-path exit codes: `stop` exits 1 and `status` still exits 0 when their `%TEMP%` write fails, where v1 crashed to exit 3 with a crash file; (f) on a daemon panic the crash hook deletes the alive file (restoring v1's crash-path respawn behavior) but does not unhook/unregister - the OS frees those at process death.

## Execution record

Eleven tasks, TDD, one signed commit each: crate scaffold (commit `Cargo.lock` - binary crate, reproducible builds), geometry, options, state JSON, request file, native one-shot core, region maintenance, daemon pump + hooks, main wiring + panic hook, `install.ps1` cargo build block + README, end-to-end gate. 18 unit tests (geometry 2, options 6, state 7, request 3); the gate was install + smoke 21/21 + the manual checklist.

Notes that survived execution:

- Measured release exe: 165,376 B (threshold to investigate: ~250KB).
- GUI-subsystem exes detach from the shell: `& $exe` returns immediately, sets no `$LASTEXITCODE`, and races file reads - use `Start-Process -Wait -PassThru`.
- Version went to v2.0.0, not v1.1: identical behavior, but the build toolchain requirement changes completely - breaking for from-source users.
