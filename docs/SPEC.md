# VLC Picture-in-Picture for Windows - Build Spec (v2, Rust)

A spec for a **Picture-in-Picture** on Windows that turns the *real* VLC player window into a borderless, always-on-top, corner-parked mini window, toggled from VLC's own View menu and a global hotkey, then restores VLC exactly. v1 (C#/.NET NativeAOT, tag `v1.0.0`) shipped this behavior at 2.26MB; v2 is the Rust rewrite (~157KB as of v2.1) with **identical observable behavior**. v2.1 adds drag gestures (move, aspect-locked resize) and size/corner persistence - §12. This document is the behavioral contract (extracted from the working v1 code) plus the Rust implementation constraints.

Target: VLC 3.0.x (3.0.23 verified), Windows 11 x64, single monitor primary use.

**The acceptance gate is language-agnostic:** `scripts/smoke-test.ps1` (38 checks: the 21 v1-era checks unchanged, plus 13 v2.1 gesture/heal checks, plus 4 v2.1.1 fullscreen-handoff checks), `scripts/uninstall.ps1`, and `extension/pip.lua` (descriptor version aside) carry over from v1. Only `pip-helper.exe`'s implementation swaps; every file format and observable behavior below must hold byte-for-byte.

---

## 1. Goal & non-goals

**Goal**
- Toggle the current VLC window into a small, borderless, always-on-top window in a screen corner ("PiP"), and toggle back to the exact original size/position/border and clear always-on-top (z-order is never saved; Exit restores with HWND_NOTOPMOST, or HWND_TOPMOST if the user had VLC's own always-on-top).
- Trigger from **VLC's View menu** and a **global hotkey** (Ctrl+Alt+P), consistently.
- **Zero added video latency / quality loss** (it's the real decoding window, not a mirror).
- **No terminal/console flashes** on toggle.
- While in PiP, **don't let the video go fullscreen** (F key and double/triple/spam-click).
- Toggling PiP **on a fullscreen VLC** first makes VLC itself leave fullscreen (v2.1.1): external reshaping can't clear VLC's internal fullscreen state, so its fullscreen-controller strip would stay on screen and the fullscreen rect would be saved as the restore state.
- **Minimal look** (default on): hide VLC's menu bar + control bar - exactly the Ctrl+H view - so PiP shows just video.

**Non-goals**
- Mirroring/duplicating the video (DWM thumbnail) - rejected: adds >=1 frame latency and is read-only.
- A separate standalone player - rejected: must reuse VLC's features/codecs/keybindings.

---

## 2. Core design decision

**Reshape VLC's own top-level window via Win32.** A small external helper:
- removes the title bar + sizing border (`SetWindowLongPtrW(GWL_STYLE)` clearing `WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZE`, followed by `SetWindowPos(..., SWP_FRAMECHANGED)`),
- sets it always-on-top (`HWND_TOPMOST`) and parks it in a work-area corner,
- restores saved styles + rect on exit.

VLC's Lua extension API **cannot** do any of this (no window-geometry API). So the Lua extension only acts as a trigger; all Win32 work happens in the compiled helper.

---

## 3. Hard constraints & verified facts (read before coding)

### VLC Lua extensions
- Lua extensions have **no window-management API**. `os.execute`/`io.popen` work but flash a console (cmd.exe) - the menu must **never spawn a process** on the normal path.
- `capabilities = {"trigger"}` → VLC calls **`trigger()` on every click**, no activation/checkmark state. ← USE THIS (gotcha #1).
- **The extension probe runs the chunk top level** to read `descriptor()`. Any top-level error (e.g. `os.getenv(x) .. "..."`) makes the extension silently vanish from the menu. Env lookups stay lazy inside functions.
- **Only `.lua` belongs in the extensions folder** - a stray `.exe` there breaks the extension scan.

### Win32 reshape
- After `SetWindowLongPtrW(GWL_STYLE, ...)` you **must** `SetWindowPos(..., SWP_FRAMECHANGED)` or the frame change won't apply.
- Save and restore both `GWL_STYLE` and `GWL_EXSTYLE`, plus the window rect.
- VLC 3.x embedded video is a `WS_CHILD` window (class prefix `"VLC video main"`) inside the Qt main window; resizing the parent re-fits the child **asynchronously** (see §8 debounce).

### Daemon message loop
- Raw Win32 thread message pump: `GetMessageW` loop; `RegisterHotKey` and `SetTimer` with NULL hwnd deliver `WM_HOTKEY`/`WM_TIMER` to the thread queue (no window needed).
- **No file I/O inside LL hook callbacks**: exceeding `LowLevelHooksTimeout` makes Windows SILENTLY remove the hook - the fullscreen-block guarantee dies with no error. Hooks read a cache refreshed on the pump thread; hook callbacks dispatch on that same thread, so no synchronization is needed.
- The exe is **GUI subsystem** (`#![windows_subsystem = "windows"]`): no console ever; stdout is invisible when Explorer-launched, so all machine-readable output goes through files (§6).

---

## 4. Architecture

```mermaid
flowchart TD
    MENU["VLC View menu<br>pip.lua, capabilities = trigger"] -- "trigger() writes 'toggle'<br>pure Lua I/O, no flash" --> REQ["vlc-pip-request.txt in TEMP"]
    HK["Ctrl+Alt+P global hotkey<br>WM_HOTKEY"] --> D
    REQ -- "consumed each 150 ms tick" --> D["pip-helper.exe daemon<br>Rust, GUI subsystem, login-started<br>raw Win32 message pump"]
    D -- "WM_TIMER 150 ms: heartbeat ~3 s,<br>consume request, refresh hook cache,<br>converge minimal-look region" --> D
    D -- "WH_KEYBOARD_LL swallows F in PiP + VLC focused<br>WH_MOUSE_LL rate-limits clicks over the PiP" --> FS["fullscreen prevented"]
    D -- "Toggle" --> WIN["Win32 reshape<br>Enter: save state, strip frame, topmost, corner<br>Exit: restore styles + rect from saved state"]
    WIN <-- "valid file = in PiP<br>single source of truth" --> STATE["vlc-pip.json in TEMP"]
```

Toggle = InPip ? Exit : Enter. Menu and hotkey BOTH call the same Toggle.

---

## 5. Components

### 5.1 `pip.lua` (the VLC extension - behavior unchanged from v1)
- `descriptor()` returns `capabilities = { "trigger" }`, title "PiP Mode".
- `trigger()` → ensure daemon alive (heartbeat check, §6.3), write `"toggle"` to the request file; errors go to `vlc.msg.err`.
- Fallback only: if the daemon is dead, `os.execute('start "" "<exe>" daemon')` (the sole case that may flash; normally never fires because of login auto-start).
- Installs to `%APPDATA%\vlc\lua\extensions\pip.lua` (ONLY the .lua here).

### 5.2 `pip-helper.exe` (Rust binary crate at `helper/`)

Single binary crate, no lib split. `windows-sys 0.61` is the only dependency; JSON is hand-rolled (§7 gotcha R2). Suggested layout:

```
helper/Cargo.toml            [package] name = "pip-helper", edition = "2024"
helper/src/main.rs           entry, panic hook, mode dispatch, one-shot region loop
helper/src/options.rs        PipOptions + argv parsing (w= h= c= m= min=)
helper/src/geometry.rs       compute_corner (pure)
helper/src/state.rs          PipState + hand-rolled JSON parse/write, load/save/delete
helper/src/request.rs        request-file consume
helper/src/native.rs         Win32: find_player, enter/exit/toggle, maintain_region, status
helper/src/daemon.rs         pump, hotkey, timer, LL hooks, heartbeat, single-instance mutex
helper/src/tests.rs          all unit tests, one cfg(test) file (tested internals are pub(crate))
```

Modes (argv[1], ASCII-lowercased; default `toggle` when absent). Options parsed from the remaining args; `w=`/`h=` accept only positive values (like `c=` normalization: a 0/negative size would park an invisible topmost window the converger can never fix):
- `toggle` | `enter` - one-shot Win32 action, then if it **entered** PiP with `min=1`: 6 × { sleep 150 ms; maintain_region } to converge the minimal look. Exit 0 on success, 1 on failure.
- `exit` - restore; no region loop. Exit 0/1.
- `status` - print status JSON to stdout (best effort) AND write it to `%TEMP%\vlc-pip-status.json` (the reliable channel). Always exit 0.
- `daemon` - run the message loop (single instance via named mutex `"VlcPipDaemon"`; a second instance exits 0 silently, touching no files). Exit 0.
- `stop` - write `"stop"` to the request file. Exit 0 (1 if the write fails; v1 crashed to 3 there).
- anything else - "unknown mode" to stderr, exit 2.
- Every mode first calls `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`.
- A panic anywhere → hook writes `%TEMP%\vlc-pip-crash.txt` (message + file:line) and the process exits 3.

### 5.3 Install layout (unchanged from v1)
```
%APPDATA%\vlc\lua\extensions\pip.lua                         (extension)
%APPDATA%\vlc\pip\pip-helper.exe                             (helper, OUT of extensions)
shell:startup\VLC PiP Daemon.lnk  ->  pip-helper.exe daemon  (login auto-start, no flash)
```

---

## 6. Runtime file contracts (all in `%TEMP%`, truncate-write, UTF-8 no BOM)

### 6.1 `vlc-pip.json` - the PiP state; its VALID existence IS "in PiP"
Written by Enter only, exactly this shape (key order, compact, no spaces; byte-compatible with v1's System.Text.Json output - old files may exist during upgrade):
```
{"Hwnd":66112,"X":100,"Y":200,"W":1000,"H":640,"Style":349110272,"ExStyle":256,"TargetW":480,"TargetH":270,"Corner":"br","Margin":16,"Min":true,"Pid":12345}
```
Types: `Hwnd`/`Style`/`ExStyle` i64; `X Y W H TargetW TargetH Margin` i32; `Corner` one of `br bl tr tl`; `Min` bool; `Pid` u32. `X..ExStyle` are the pre-PiP restore data; `TargetW..Min` are the options in effect at Enter (so daemon and one-shot CLI converge on the same geometry); `Pid` is the owner process.
- **Old 7-field files** (v1.0 pre-audit: `Hwnd..ExStyle` only) must load with defaults `TargetW=480, TargetH=270, Corner="br", Margin=16, Min=true, Pid=0`.
- Unknown keys with scalar values are skipped; **any** parse failure (torn write, corrupt, trailing garbage, JSON escapes, nested values) loads as `None` = "not in PiP" - the parser is deliberately strict, and the benign failure mode is the point.
- **Stale detection (`owns_state`)**: state is live iff `IsWindow(Hwnd)` AND `GetWindowThreadProcessId(Hwnd) == Pid != 0`. Windows recycles HWNDs - IsWindow alone would pass on a foreign window. `Pid=0` (old files) is always stale by design (one re-toggle after upgrade). Stale state is deleted on sight by InPip/Exit/maintain_region (delete failures swallowed: next caller retries).

### 6.2 `vlc-pip-request.txt` - command channel into the daemon
Bare word, trimmed on read: `toggle` | `enter` | `exit` | `stop` (case-sensitive). Consumed (read + delete) every 150 ms tick; read errors leave the file for the next tick; empty file is deleted and ignored. On daemon start, a pre-existing request is discarded **only if it is `stop`** (a `pip-helper stop` with no daemon alive leaves one that would kill the fresh daemon on its first tick; a queued user toggle survives).

### 6.3 `vlc-pip-daemon.alive` - heartbeat + arming diagnostics
Single line, no newline, rewritten on start and then every >3000 ms (checked each 150 ms tick):
```
{unix_seconds_utc} pid={pid} hotkey={0|1} timer={0|1} kb={0|1} mouse={0|1}
```
Flags = did RegisterHotKey/SetTimer/each SetWindowsHookExW succeed (their failure is NOT fatal - it is only reported here). Write failures are swallowed and retried next beat: NEVER let the heartbeat kill the pump. Deleted on clean daemon exit AND by the crash handler when the daemon panics (else pip.lua would treat the dead daemon as alive for up to 15 s and drop menu toggles).
**Consumer contract (pip.lua)**: reads the leading number with Lua `read("*n")`; alive iff the parse yields nil (mid-truncate read = daemon IS alive, never respawn) OR `abs(os.time() - ts) < 15`. So the line MUST start with the epoch number.

### 6.4 `vlc-pip-status.json` - `status` mode output (stdout is unreliable for a GUI exe)
Exactly (key order, lowercase booleans): `{"found":false}` or
```
{"found":true,"hwnd":N,"x":N,"y":N,"w":N,"h":N,"caption":B,"topmost":B,"inPip":B,"minimal":B}
```
`caption` = `(style & WS_CAPTION) == WS_CAPTION` (BOTH bits of 0x00C00000); `topmost` = `exstyle & WS_EX_TOPMOST != 0`; `minimal` = window has a region (`GetWindowRgn` probe). The smoke test drives everything through this file.

### 6.5 `vlc-pip-crash.txt` - panic message + location, best-effort write from the panic hook; process exits 3. The only diagnostics channel.

---

## 7. Behavioral contract - Win32 sequences

### find_player
1. Toolhelp process snapshot → set of PIDs whose exe name == `vlc.exe` (case-insensitive). Empty → null.
2. `EnumWindows`: skip invisible; skip PIDs not in the set; skip **empty titles** (filters VLC's hidden/extension windows); first window whose title contains `"VLC media player"` (case-insensitive) wins and stops enumeration; else track the biggest-area window as fallback.

### enter(h, o) - all steps in this order
1. Guard: null h or already InPip → false.
2. `IsIconic(h)` → `ShowWindow(h, SW_RESTORE)` (else the off-screen iconic rect gets saved as the restore state).
3. Read rect, `GWL_STYLE`, `GWL_EXSTYLE`, owner pid; **save state FIRST** (before any mutation).
4. With `min=1` and a video child present, measure the client-relative chrome around the child (menu above, controller below - Qt client-area widgets, so the offsets survive the border strip; sanity: per-axis sums within 0..=300, else fall back to step 6's plain path).
5. Strip `WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZE` (WS_MAXIMIZE too: a zoomed window keeps IsZoomed, so Win+Down/Aero would snap the PiP back to Qt's normal placement rect).
6. Corner from the **work area** (`GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST)).rcWork`, taskbar excluded): `left = work.left+margin; top = work.top+margin; right = work.right-w-margin; bottom = work.bottom-h-margin`; `tl/tr/bl` as named, anything else = `br`. With measured chrome, one `SetWindowPos(h, HWND_TOPMOST, vx-cl, vy-ct, w+cl+cr, h+ct+cb, SWP_FRAMECHANGED|SWP_SHOWWINDOW)` followed immediately by the region `(cl, ct, cl+w, ct+h)` - the PiP lands fully formed, no visible grow-then-clip pass (the converger only verifies). Without chrome (not playing, `min=0`, garbage measurement): plain `SetWindowPos(..., o.w, o.h, ...)` and the converger takes over.
7. **Rollback on failure** (e.g. UIPI vs elevated VLC): restore the original style, delete state, never claim in-PiP (the region is only applied after a successful SetWindowPos).

### exit() - all steps in this order
1. Load state; null → false. `owns_state` fails → delete state, false.
2. `SetWindowRgn(h, null, true)` FIRST - drop the minimal-look clip before restoring.
3. Restore `GWL_STYLE`, then `GWL_EXSTYLE`.
4. `SetWindowPos(h, saved ExStyle & WS_EX_TOPMOST ? HWND_TOPMOST : HWND_NOTOPMOST, saved rect, SWP_FRAMECHANGED|SWP_SHOWWINDOW)` - honors the user's own always-on-top.
5. Delete state iff `ok || !IsWindow(h)` - a failed restore on a still-live window keeps the state so the next toggle retries.

### Fullscreen handoff (v2.1.1)
Entering PiP from a fullscreen VLC must first make VLC itself leave fullscreen.
- **Detect**: `WS_CAPTION` fully absent AND the window rect covers its monitor's whole `rcMonitor` (fail toward the plain enter if `GetMonitorInfoW` fails).
- **Leave**: post `Esc` (`WM_KEYDOWN`+`WM_KEYUP`, scan 0x01) to the video child - its win32 vout window proc feeds VLC's core hotkey engine, so it works unfocused (~20ms observed; Esc is leave-fullscreen only, a no-op otherwise). Qt-level keys are unreliable via PostMessage (gotcha #7); the vout proc is not Qt. No child → post to the top-level window.
- **Daemon path (deferred)**: the pump must never block (LL-hook timeout, §3). Toggle/enter on a fullscreen VLC posts Esc and arms a pending enter; each 150ms tick then requires the caption present on TWO consecutive ticks (Qt's style AND rect restores both landed) before calling enter. 2s deadline (Esc eaten, e.g. by a modal → give up, never enter), VLC dying drops it, and a second toggle or an exit request cancels it.
- **One-shot path (blocking)**: `toggle`/`enter` CLI modes poll for the caption up to 2s then settle 100ms; timeout → exit 1 without entering (never save the fullscreen rect as restore state).

### maintain_region() - minimal look, converging per-tick (daemon timer + one-shot loop)
Cross-tick state: previous window rect + child rect + have_prev flag (reset on missing/stale state, no child, and after our own resize).
1. Load state; missing → reset, return. Stale → reset, delete, return. `Min=false` → return.
2. Find the video child: first visible child (recursive) whose class starts with `"VLC video main"`. None (playback stopped) → reset, clear region if present, return.
3. **Two-tick stability debounce**: read window + child rects; act only if both are UNCHANGED since the previous tick (VLC re-fits the child asynchronously after our resize; acting on unsettled rects caused perpetual resize thrash in v1). Always record current rects.
4. **Chrome sanity clamp**: chrome = window minus child size; if any dimension is negative or > 300 px → stale rects, return.
5. Child not at target size (tolerance ±2 px): recompute corner for the video, resize window to `target + chrome` positioned so the CHILD lands at the corner (`SetWindowPos(h, HWND_TOPMOST, tx, ty, tw, th, SWP_FRAMECHANGED)` - no SWP_SHOWWINDOW here), invalidate have_prev (our own resize), return. Skip if the rect is already correct or target+chrome is non-positive.
6. Child at target and no region yet: `CreateRectRgn(child rel rect)` + `SetWindowRgn`; **on failure `DeleteObject` the region - the system owns it only on success**.

### Fullscreen prevention (prevent, don't auto-exit; poll-and-snap-back flickers)
- **F key** (`WH_KEYBOARD_LL`): swallow iff `code >= 0` AND (WM_KEYDOWN or WM_SYSKEYDOWN) AND vk == F AND hook cache says in-PiP AND `GetForegroundWindow() == cached hwnd`. Key-ups pass.
- **Clicks** (`WH_MOUSE_LL`) - the rate-limit, exact bookkeeping (statics: last ALLOWED down time+point, swallow_next_up flag):
  - On `WM_LBUTTONDOWN` over the PiP (root ancestor of `WindowFromPoint` == cached hwnd): `burst = (evt.time - last_allowed_time <= GetDoubleClickTime()) && |dx| <= SM_CXDOUBLECLK && |dy| <= SM_CYDOUBLECLK`. Burst → set swallow_next_up, swallow. Else record this down as the new ALLOWED reference and pass.
  - On `WM_LBUTTONUP` with swallow_next_up set: clear the flag, swallow (keeps the input stream paired).
  - The reference point is the last **ALLOWED** down - so EVERY down inside the window/rect of the last allowed down is swallowed, and no two clicks the OS actually delivers can pair into `WM_LBUTTONDBLCLK`. (v1 bug: swallowing only the 2nd click let the OS pair clicks 1+3 - TRIPLE click fullscreened.)
  - `GetDoubleClickTime`/`GetSystemMetrics` queried live per event; timestamps are u32 ms with wrapping subtraction.
- Hooks never touch the disk: they read a **pump-thread cache** (the hwnd of a loaded state passing `IsWindow`, refreshed before the loop and after every hotkey/timer action). Deletion of stale files stays in the toggle paths + maintain_region.

### Daemon loop
1. Named mutex `"VlcPipDaemon"` → second instance exits 0 before touching any file.
2. Discard pre-launch `stop` request (only `stop`).
3. `RegisterHotKey(null, 1, MOD_CONTROL|MOD_ALT|MOD_NOREPEAT, 'P')`; `SetTimer(null, 0, 150, null)`; install both LL hooks. Failures recorded in heartbeat flags only.
4. Beat once; refresh hook cache once (a daemon restarted while already in PiP must be guarded from the first message).
5. Pump: `WM_HOTKEY` → Toggle (deferring through the fullscreen handoff) + refresh cache. `WM_TIMER` → beat if >3 s, consume request (`toggle`/`enter`/`exit` act; `stop` → `PostQuitMessage(0)`), tick any pending handoff enter, refresh cache, maintain_region - in that order (the cache must reflect a request- or handoff-triggered toggle within the same tick). Transient file-I/O errors are swallowed (retry next tick); anything else propagates to the crash handler. `TranslateMessage`/`DispatchMessageW` always run.
6. Cleanup on loop exit: unhook both, unregister hotkey, delete the alive file.

---

## 8. Gotchas that caused real bugs (do not repeat)

From v1 development:
1. **Menu/hotkey desync.** VLC's `activate()/deactivate()` checkmark state + separate hotkey state = "many bad states". FIX: `trigger` capability + single state file; both paths call Toggle.
2. **Top-level `os.getenv` in the extension** made it vanish from the menu (probe error). FIX: lazy env lookups.
3. **Exe in the extensions folder** broke the extension scan. FIX: helper lives in `%APPDATA%\vlc\pip\`.
4. **Console flashes**: `os.execute` always flashes via cmd. FIX: request file + login-started GUI-subsystem daemon.
5. **Double-click snap-back flicker**: poll-and-snap-back reacts after VLC fullscreens (big → corner flicker). FIX: mouse-hook swallow - prevent before, not after.
6. **Triple-click fullscreened**: swallowing only the 2nd click let the OS pair clicks 1+3. FIX: rate-limit against the last ALLOWED down (§7).
7. **Ctrl+H via PostMessage/SendInput** is ignored/blind-toggles. FIX: `SetWindowRgn` clip (§7 maintain_region).
8. **Region thrash**: acting on fresh-but-unsettled rects (VLC re-fits the child async) caused perpetual resize. FIX: two-tick stability debounce + chrome sanity clamp.
9. **`start /B` ties the daemon to the launching console.** Launch detached: `start "" "<exe>" daemon`, or the login shortcut.

New, Rust-specific (verified 2026-07-02):
- **R1. `lto` must be set explicitly.** With `codegen-units = 1` and the default `lto = false`, Cargo performs NO LTO at all (Cargo book). The size profile needs `lto = true`.
- **R2. JSON is hand-rolled** (`state.rs`): measured 125,440 B vs 169,472 B (serde_json) / 174,080 B (nanoserde) for the full spike - the crates add ~26-28% to the exe for one flat frozen schema. The writer does NOT escape strings → **`c=` option values must be normalized to `br|bl|tr|tl` at parse time** (unknown → `br`; same effective geometry as v1, which stored the raw string but treated unknown as `br`).
- **R3. `CreateMutexW` is feature-gated on `Win32_Security`** (its `SECURITY_ATTRIBUTES` param), on top of `Win32_System_Threading`. Without both, the fn doesn't exist.
- **R4. windows-sys 0.61 handles (`HWND`, `HHOOK`, ...) are `*mut c_void`** - not `Send`/`Sync`, can't live in statics. Store `isize` (atomics for hook-shared state) and cast at call sites. `hwnd as isize as i64` round-trips through the state file exactly on x64.
- **R5. Module surprises**: `SetWindowRgn`/`GetWindowRgn`/`MonitorFromWindow`/`GetMonitorInfoW` are in `Win32::Graphics::Gdi`; `GetDoubleClickTime` is in `Win32::UI::Input::KeyboardAndMouse`; `GetWindowLongPtrW`/`SetWindowLongPtrW` exist only on 64-bit targets (fine here).
- **R6. Panic hook runs under `panic = "abort"`** and `Location` (file:line) survives `strip = true` (std docs + verified locally). Write the crash file with `let _ = fs::write(...)` (the hook must never panic) and `process::exit(3)` to match v1's crash exit code.
- **R7. Hook callbacks are plain `unsafe extern "system" fn`s** - `'static` by nature, so C#'s "delegate must be a static field or it gets GC'd" pinning does not translate; nothing to hold. Pass state via atomics (see R4); everything runs on the pump thread.
- **R8. `cargo test` is unaffected** by `panic = "abort"` (tests ignore the panic setting) and by `#![windows_subsystem = "windows"]` (output flows through inherited handles).

PowerShell (from v1 dev): `if` is not an expression; single-letter functions collide with aliases; `Remove-Item` on non-literal paths can be blocked - prefer literal paths.

---

## 9. Build / install / uninstall

- **Build:** `cargo build --release` in `helper/` (rustc 1.96+, MSVC toolchain located automatically - no vswhere/PATH tricks needed, unlike v1's NativeAOT). Artifact: `helper/target/release/pip-helper.exe` (~157KB).
  Profile: `opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`.
- **Install:** `scripts/install.ps1` - builds, stops a running daemon (process-gated: request `stop`, 5 s poll, force-kill fallback), removes a stale request file, copies exe + pip.lua, creates the Startup shortcut, starts the daemon, waits up to 5 s for the alive file.
- **Test:** `cargo test` in `helper/` (pure logic: state JSON, geometry, options, request), then `scripts/smoke-test.ps1` (34 end-to-end checks against live VLC; requires install first and VLC closed).
- **Uninstall:** `scripts/uninstall.ps1` - restores a PiP'd VLC FIRST (one-shot `exit`), then stops the daemon, then deletes the three install paths and the five `%TEMP%\vlc-pip*` files.

---

## 10. Known limitations & future

- If the Windows profile path contains characters not representable in the system ANSI codepage, the Lua trigger cannot resolve `%TEMP%`/`%APPDATA%` and errors into VLC's log; the hotkey path is unaffected.
- VLC 4.0 changes the video window architecture (DirectComposition) and needs re-validation.
- The release exe is not Authenticode-signed: SmartScreen shows "Windows protected your PC" on downloaded copies.

---

## 11. Acceptance test checklist

Automated: `cargo test` green, then `scripts/smoke-test.ps1` → 38/38 PASS (enter/exit geometry + styles, topmost, minimal-look region, double/triple/spam-click immunity, hotkey + request-file interleaving without desync, exact rect restore, drag-move, edge drag-resize with the minimal look held, config.txt persistence + re-enter at persisted size, band-click/wheel no-ops, fullscreen-handoff enter/exit, close-in-PiP reopen heal).

Manual (once per release):
- [ ] View → "PiP Mode" appears after a VLC restart; repeated menu clicks alternate enter/exit.
- [ ] No console flash on menu toggle (daemon already running).
- [ ] F / double-click work normally when NOT in PiP; F swallowed only while VLC focused in PiP.
- [ ] Playback stop while in PiP shows the mini UI (region cleared); restart re-clips.
- [ ] Daemon survives VLC close and re-arms on next VLC launch; login shortcut starts it flash-free.
- [ ] Drag-resize works from all four corners (the smoke test can only reach the right edge); drags feel smooth; the 256px min / 80% max clamps hold.
- [ ] Wheel-volume and Ctrl+wheel subtitle scale still reach VLC over the unfocused PiP.

---

## 12. Drag gestures & persistence (v2.1)

New in v2.1; everything above is unchanged. No modifier keys and nothing new is swallowed: a single left click on video is a no-op in stock VLC 3.x, so press-drag-release is free to repurpose.

### Gesture contract
- **Interior drag = free move.** Press inside the visible PiP, drag past the system drag threshold, release: the window follows live and stays where dropped. Free placement survives the region converger (`plan_region` only repositions during a size correction).
- **Band drag = aspect-locked resize.** A drag starting in the outer 16px band (DPI-scaled: 16 x dpi/96) of the **visible** rect - the region box, not the window rect - resizes live from that edge or corner, opposite side anchored; pure edges keep the perpendicular center fixed. Corners win where bands overlap. Aspect is the window's at drag start; width clamps to 256px min, capped so neither dimension exceeds 80% of the work area.
- **Release** derives `Corner` = work-area quadrant of the window center (tie = `br`) and, for a resize, `TargetW/H` = final size minus the chrome measured at drag start; both go to the state file and `config.txt`. The next enter and any convergence-driven re-park use them.
- **Wheel: never touched.** Plain wheel = VLC volume - Windows ships "scroll inactive windows" on, so this already works over the unfocused PiP - and Ctrl+wheel = subtitle scale. The hook intercepts no wheel message.
- Fullscreen guards unchanged: burst-swallowed downs never arm a drag.

### `config.txt` - persisted size + corner
`%APPDATA%\vlc\pip\config.txt`, one line of ordinary option tokens:
```
w=640 h=360 c=br
```
Written on every drag release (from the pump, never the hook; the `pip` folder is created if missing; write failures swallowed - the gesture still holds via the state file). Read at **every** enter, layered defaults < config < argv, so startup-shortcut args still win and the daemon sees its own writes without a restart. Missing or unreadable config = exact v2.0 behavior. Uninstall removes it with the pip folder.

### Mechanics
- The mouse hook arms on an **allowed** button-down over the PiP (cursor origin, window + visible rects, zone) and activates past `SM_CX/CYDRAG`; while active it stores the latest cursor position and posts one **coalesced** `WM_APP` drag message. Idle mouse-move cost is one atomic load, and the hook still never touches the disk.
- The pump computes the target rect itself (move = start + delta; resize = aspect plan) and applies it with an async `SetWindowPos`. Every message carries a generation counter, so a rapid release-and-repress can't mix a stale message with re-armed state.
- The minimal look stays live through a resize: each tick re-clips to the start chrome offsets applied to the new size; after release, convergence verifies the region **box** (not just presence) against the actual video child and corrects it.
- Drag-end finalizes **from the computed rect** - the async `SetWindowPos` may not have landed in VLC yet, so a fresh `GetWindowRect` would read stale.
- `maintain_region` is skipped while a drag is active; after a resize, convergence re-clips with at most a ±2px correction.

### Close-in-PiP heal
VLC that closes while in PiP persists the PiP geometry as its own (Qt saves on exit), so its next launch would open full-size at the PiP origin, overflowing the screen. The daemon keeps the stale state file as a pending-restore record: when a new VLC player window appears, it applies the saved pre-PiP rect and deletes the state only after observing the rect stick (VLC's own startup positioning cannot win the race).
Hardening: `in_pip` is read-only (a status query can't destroy the record; an explicit enter consumes it by overwriting, the one-shot `exit` drops it); the heal skips iconic windows, fires only when the recorded VLC process is truly gone (legacy `Pid=0` records are dropped), refuses rects on monitors that no longer exist, and gives up after ~6s of non-convergence (e.g. elevated VLC, UIPI). Because pending records can now outlive VLC indefinitely, the hook's HWND cache uses the full owner-PID guard - a recycled handle can never re-arm the guards or drags on a foreign window.

### Accepted edges
No sizing cursors over the band (cursor feedback needs input injection; Firefox's PiP is equally unmarked). Sizes are raw pixels across mixed-DPI monitors. While a pending heal waits for a VLC relaunch, the tick polls `find_player` (one process snapshot per 150ms). With two VLC instances, the surviving instance may receive the dead one's pre-PiP rect once.
