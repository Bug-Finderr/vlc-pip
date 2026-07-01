# VLC Picture-in-Picture for Windows - Build Spec

A spec for a **Picture-in-Picture** on Windows that turns the *real* VLC player window into a borderless, always-on-top, corner-parked mini window, toggled from VLC's own View menu and a global hotkey, then restores VLC exactly. This document captures the requirements **and** the hard-won gotchas from a prior implementation so a rewrite avoids the same bugs.

Target: VLC 3.0.x (3.0.23 verified), Windows 11, single monitor primary use.

---

## 1. Goal & non-goals

**Goal**
- Toggle the current VLC window into a small, borderless, always-on-top window in a screen corner ("PiP"), and toggle back to the exact original size/position/border/z-order.
- Trigger from **VLC's View menu** and a **global hotkey** (e.g. Ctrl+Alt+P), consistently.
- **Zero added video latency / quality loss** (it's the real decoding window, not a mirror).
- **No terminal/console flashes** on toggle.
- While in PiP, **don't let the video go fullscreen** (F key and double-click).
- Optional: a **minimal look** (hide VLC's menu bar + control bar; exactly the Ctrl+H view) so PiP shows just video.

**Non-goals**
- Mirroring/duplicating the video (DWM thumbnail) - rejected: adds >=1 frame latency and is read-only. We reshape the real window instead.
- A separate standalone player - rejected: must reuse VLC's features/codecs/keybindings.

---

## 2. Core design decision

**Reshape VLC's own top-level window via Win32.** A small external helper:
- removes the title bar + sizing border (`WS_CAPTION | WS_THICKFRAME` via `SetWindowLongPtr`/`SetWindowLong(GWL_STYLE)`, followed by `SetWindowPos(..., SWP_FRAMECHANGED)`),
- sets it always-on-top (`SetWindowPos` with `HWND_TOPMOST`),
- resizes/repositions it to a corner.

Because it's the genuine hardware-decoding window, there is no re-decode and no extra compositing hop (just normal DWM windowed composition). Original geometry + window styles are saved before entering and restored on exit.

VLC's Lua extension API **cannot** do any of this (no window-geometry API). So the Lua extension only acts as a trigger; all Win32 work happens in a compiled helper.

---

## 3. Hard constraints & verified facts (read before coding)

### VLC Lua extensions
- Lua extensions have **no window-management API** (cannot move/resize/de-border/raise the window). Only `vlc.video.fullscreen()` exists, and `vlc.var.set(vlc.object.vout(), "video-on-top", true)` sets always-on-top of the *vout* surface only (not the Qt window).
- VLC does **not** sandbox the Lua stdlib: `os.execute` and `io.popen` work. But a script that blocks the Lua thread for ~5 s triggers VLC's "Extension not responding" - so any external launch must be **fire-and-forget**, never awaited.
- **Extension lifecycle (verified from VLC source):**
  - `capabilities = {}` → the View-menu entry is a **toggle**: 1st click `activate()`, 2nd click `deactivate()`, with a checkmark. VLC tracks the active state.
  - `capabilities = {"trigger"}` → VLC calls **`trigger()` on every click**, with **no activation/checkmark state**. ← USE THIS (see §9 gotcha #1).
- **The extension probe runs the chunk to read `descriptor()`.** Any error at file top level (e.g. `os.getenv(x) .. "..."` if it ever returns nil) makes the extension fail to load and silently disappear from the menu. Keep top level to function definitions only; do all env lookups lazily inside functions.
- **Only `.lua` belongs in the extensions folder.** A stray `.exe` next to it can break the extension scan so the menu entry never appears. Put the helper elsewhere.

### Win32 reshape
- After `SetWindowLong(GWL_STYLE, ...)` you **must** call `SetWindowPos(..., SWP_FRAMECHANGED)` or the frame change won't apply (window data is cached).
- Save and restore both `GWL_STYLE` and `GWL_EXSTYLE`, plus the window rect. Restore with `HWND_NOTOPMOST` to clear always-on-top.
- VLC 3.x embedded video is a `WS_CHILD` window inside the Qt main window; resizing the parent re-fits the video automatically. Keep `WS_CLIPCHILDREN` on the parent.

### Process launch & flashes
- `os.execute(...)` / `io.popen(...)` from VLC go through `cmd.exe`, which **flashes a console** on a GUI-subsystem parent. There is **no flashless launch from Lua**.
- Therefore: the menu must **not spawn a process**. It writes a request file; a long-lived helper daemon reads it. The daemon is started **at login** (launched by Explorer = no console). The exe must be **GUI subsystem** (`/target:winexe`) so it never shows a console.

### Daemon message loop
- A hidden WinForms `Form` whose `SetVisibleCore` always returns false **exits `Application.Run` immediately**. Use an **`ApplicationContext` + `NativeWindow`** for a hidden message loop instead.

---

## 4. Architecture

```
VLC View menu  (pip.lua, capabilities={"trigger"})
      |  trigger()  ->  write %TEMP%\vlc-pip-request.txt = "toggle"     (pure Lua I/O, no flash)
      v
pip-helper.exe  (GUI winexe, started at app login; ApplicationContext message loop)
      |  reads request file every ~150 ms  ->  Native.Toggle()
      |  global hotkey Ctrl+Alt+P          ->  Native.Toggle()
      |  WH_KEYBOARD_LL  -> swallow F while in PiP & VLC focused (no fullscreen)
      |  WH_MOUSE_LL     -> swallow 2nd click of a double-click on PiP (no fullscreen)
      v
Win32: find player by title -> save state -> strip frame + topmost + corner  (Enter)
                            -> restore styles + rect from saved state          (Exit)

Single source of truth: %TEMP%\vlc-pip.json exists  <=>  currently in PiP.
Toggle = InPip ? Exit : Enter.  BOTH menu and hotkey call the same Toggle.
```

---

## 5. Components

### 5.1 `pip.lua` (the VLC extension)
- `descriptor()` returns `capabilities = { "trigger" }`, title "PiP Mode".
- `trigger()` → write `"toggle"` to `%TEMP%\vlc-pip-request.txt`.
- All `os.getenv` lazy, inside functions (probe-safe). No top-level work besides defs.
- Fallback only: if the daemon's alive file is missing, `os.execute('start "" "<exe>" daemon')` (the sole case that may briefly flash; normally never fires because of login auto-start).
- Installs to `%APPDATA%\vlc\lua\extensions\pip.lua` (ONLY the .lua here).

### 5.2 `pip-helper.exe` (compiled, GUI subsystem)
Single C# exe, `csc /target:winexe`, references System / System.Drawing / System.Windows.Forms.

Modes (argv[0]):
- `toggle` | `enter` | `exit` - one-shot Win32 action (also used by tests).
- `daemon` - run the background message loop (single instance via named `Mutex`).
- `stop` - write `"stop"` to the request file so a running daemon exits.

`Native` static class:
- `FindPlayer()` - enumerate visible top-level windows, filter by VLC pid(s), skip the extension window, return the one whose title contains "VLC media player" (fallback: largest).
- `Enter(h)` - if not already in PiP, save `{x,y,w,h,style,exstyle}` to `%TEMP%\vlc-pip.json`; strip `WS_CAPTION|WS_THICKFRAME`; `SetWindowPos(HWND_TOPMOST, corner, W,H, SWP_FRAMECHANGED)`.
- `Exit(h)` - read saved state; restore style+exstyle; `SetWindowPos(HWND_NOTOPMOST, saved rect)`; delete the state file.
- `Toggle(h)` - `InPip() ? Exit : Enter`.
- Config via argv: `w=`, `h=`, `c=br|bl|tr|tl`, `m=` (margin).

`DaemonContext : ApplicationContext`:
- Hidden `NativeWindow` for `WM_HOTKEY`; `RegisterHotKey(Ctrl+Alt+P, MOD_NOREPEAT)`.
- `System.Threading.Timer` ~150 ms: process the request file (`enter`/`exit`/`toggle`/`stop`).
- `WH_KEYBOARD_LL`: if in PiP and VLC is foreground and key == F → return 1 (swallow).
- `WH_MOUSE_LL`: if in PiP and a `WM_LBUTTONDOWN` is over a VLC window within `GetDoubleClickTime()` of the previous one → return 1 (swallow the 2nd click).
- Writes `%TEMP%\vlc-pip-daemon.alive` on start, deletes on exit. **Persists** when VLC is closed (idles); exits on `stop`.
- Installs to `%APPDATA%\vlc\pip\pip-helper.exe`.

### 5.3 Install layout
```
%APPDATA%\vlc\lua\extensions\pip.lua                         (extension)
%APPDATA%\vlc\pip\pip-helper.exe                             (helper, OUT of extensions)
shell:startup\VLC PiP Daemon.lnk  ->  pip-helper.exe daemon  (login auto-start, no flash)
```
Runtime files in `%TEMP%`: `vlc-pip.json` (state), `vlc-pip-request.txt` (menu→daemon),
`vlc-pip-daemon.alive` (heartbeat).

---

## 6. State management
- **One** source of truth: presence of `%TEMP%\vlc-pip.json` means "in PiP".
- Menu and hotkey **both** call `Toggle` (never separate enter/exit), so they can't desync.
- `trigger` capability avoids VLC's own checkmark state entirely (no second state machine).

---

## 7. Fullscreen prevention (chosen behavior: prevent, don't auto-exit)
- **F key**: `WH_KEYBOARD_LL` swallows F while in PiP and VLC is foreground. (Safe: only F, only when VLC focused, so other apps' F still work.)
- **Double-click**: `WH_MOUSE_LL` swallows the *second* click of a double-click over the PiP window, so VLC never forms a double-click → never fullscreens. Single click (pause) passes.
- Do **not** use a poll-and-snap-back guard - it reacts after VLC fullscreens and visibly flickers (big → corner).

---

## 8. Gotchas that caused real bugs (do not repeat)
1. **Menu/hotkey desync.** Using `activate()/deactivate()` (VLC checkmark state) for the menu while the hotkey toggled a separate state → "many bad states". FIX: `trigger` capability + single state file; both paths call `Toggle`.
2. **Top-level `os.getenv` in the extension.** Made the extension vanish from the menu (probe error). FIX: lazy env lookups inside functions.
3. **Exe in the extensions folder.** Broke the extension scan. FIX: helper lives in `%APPDATA%\vlc\pip\`.
4. **Console flashes.** `os.execute` always flashes via cmd. FIX: request file + login-started GUI-subsystem daemon; menu never spawns a process.
5. **Hidden Form exits `Application.Run`.** FIX: `ApplicationContext` + `NativeWindow`.
6. **Double-click snap-back flicker.** FIX: mouse-hook swallow (prevent before, not after).
7. **Ctrl+H minimal view inverts / is ignored.** `PostMessage` ignored by Qt; `SendInput` needs foreground and blind-toggles into inverted states. FIX: use `SetWindowRgn` (§8).
8. **`start /B` ties the daemon to the launching console** (hangs/kills it). Launch detached with `start "" "<exe>" daemon` (no `/B`), or via the login shortcut.
9. **PowerShell pitfalls during dev/testing:** `if` is not an expression; single-letter functions like `R`/`r` collide with aliases (Invoke-History); some sandboxes block `Remove-Item` on non-literal paths (use `[IO.File]::Delete` with literal paths).

---

## 9. Build / install / uninstall
- **Build:** `csc /target:winexe /out:pip-helper.exe /r:System.dll /r:System.Drawing.dll /r:System.Windows.Forms.dll pip.cs`.
- **Install:** copy `pip.lua` → extensions folder; copy `pip-helper.exe` → `%APPDATA%\vlc\pip\`; create the `shell:startup` shortcut to `pip-helper.exe daemon`; start the daemon; restart VLC.
- **Uninstall:** `pip-helper.exe stop`; delete the 3 install paths and the `%TEMP%\vlc-pip*` files.

---

## 10. Known limitations & future
- Multi-monitor / per-monitor-DPI: position via the monitor under the window (`Screen.FromHandle`) and make the helper PerMonitorV2 DPI-aware for crisp sizing.
- If the daemon is force-killed while in PiP, a stale `vlc-pip.json` remains; next toggle reads it as "in PiP". Consider validating the saved window still exists on Enter/Exit.

---

## 12. Acceptance test checklist
- [ ] View → "PiP Mode" appears after a VLC restart.
- [ ] Menu click toggles enter/exit; repeated clicks alternate correctly.
- [ ] Ctrl+Alt+P toggles; interleaving menu + hotkey never desyncs.
- [ ] Enter = borderless, always-on-top, correct corner, correct size.
- [ ] Exit = exact original rect, border restored, topmost cleared.
- [ ] No console/terminal flash on toggle (daemon already running).
- [ ] In PiP: F does nothing; double-click does nothing (no fullscreen, no flicker).
- [ ] F / double-click still work normally when NOT in PiP.
- [ ] (If clean look) PiP shows only the video; exit restores menu + controls.
- [ ] Daemon survives VLC close and is armed again on next VLC launch / at login.
