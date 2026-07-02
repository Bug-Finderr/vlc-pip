# VLC PiP

Turns the **real** VLC 3.0.x window into a borderless, always-on-top, corner-parked mini window ("PiP"), toggled from **VLC's View menu** ("PiP Mode") or **Ctrl+Alt+P**, and restores VLC to its exact original size, position, and borders, clearing always-on-top on exit.

No mirroring, no second player: the genuine hardware-decoding VLC window is reshaped via Win32, so there is zero added latency and every VLC feature/shortcut keeps working inside the PiP.

## How it works

```
VLC View menu (pip.lua, capabilities={"trigger"})
   trigger() -> writes %TEMP%\vlc-pip-request.txt = "toggle"   (pure Lua I/O, no console flash)
pip-helper.exe daemon (GUI subsystem, started at login)
   polls request file every 150 ms | global hotkey Ctrl+Alt+P | LL hooks block F/double-click fullscreen in PiP
Win32: strip WS_CAPTION|WS_THICKFRAME -> SetWindowPos(HWND_TOPMOST, corner) on enter
       restore saved styles + rect  -> SetWindowPos(HWND_NOTOPMOST)         on exit
State: %TEMP%\vlc-pip.json exists <=> in PiP (single source of truth for menu + hotkey)
```

## Install

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install.ps1
```

Builds a NativeAOT `pip-helper.exe` (~2.3MB, no runtime dependency; building needs the .NET 10 SDK plus the MSVC C++ toolchain from Visual Studio or Build Tools), then installs:

| Path | What |
|---|---|
| `%APPDATA%\vlc\lua\extensions\pip.lua` | View-menu extension (only the .lua lives here) |
| `%APPDATA%\vlc\pip\pip-helper.exe` | helper + daemon |
| `shell:startup\VLC PiP Daemon.lnk` | starts the daemon at login (no console) |

Restart VLC afterwards, then use **View → PiP Mode** or **Ctrl+Alt+P**.

## Configure

The daemon and one-shot modes accept `w= h= c=br|bl|tr|tl m= min=` (size, corner, margin, minimal look), e.g. in the startup shortcut arguments: `daemon w=640 h=360 c=tr m=24`. Defaults: 480x270, bottom-right, margin 16, `min=1`.

**Minimal look:** while video is playing, the PiP window is clipped (`SetWindowRgn`) to just the video area - no menu bar, no control bar, like Ctrl+H - and the window is grown by the chrome height so the visible video is exactly `w x h`. Stops/starts of playback while in PiP are handled automatically (the daemon re-checks every 150 ms). Pass `min=0` to keep VLC's controls visible in PiP.

CLI modes: `pip-helper.exe toggle|enter|exit|status|daemon|stop` (`status` also writes `%TEMP%\vlc-pip-status.json`).

## Test

```powershell
powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1
```

## Uninstall

```powershell
powershell -ExecutionPolicy Bypass -File scripts\uninstall.ps1
```

## Notes

- VLC 3.x only (3.0.23 verified). VLC 4.0 changes the video window architecture (DirectComposition) and needs re-validation.
- While in PiP, the `F` key is swallowed and clicks over the PiP are rate-limited to one per double-click interval, so double/triple/spam clicks can never form a fullscreen double-click; everything works normally outside PiP.
- If VLC is closed while in PiP, the stale state is detected and cleared on the next toggle (the saved owner PID also guards against Windows recycling the window handle to another app).
- If the Windows profile path contains characters not representable in the system ANSI codepage, the Lua trigger (View menu) cannot resolve %TEMP%/%APPDATA% and reports an error in VLC's log; the Ctrl+Alt+P hotkey is handled entirely inside the daemon and is unaffected.
- Unhandled helper crashes leave a trace at `%TEMP%\vlc-pip-crash.txt`.
- The release exe is not code-signed: on first run of a downloaded copy, SmartScreen shows "Windows protected your PC" - click "More info" then "Run anyway" (or build from source with `scripts\install.ps1`).
