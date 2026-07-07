# vlc-pip

Turns the **real** VLC 3.x window into a borderless, always-on-top, corner-parked mini player - toggled from **View → PiP Mode** or **Ctrl+Alt+P** - and restores it to its exact original size, position, and borders on toggle back.

No mirroring, no second player: the genuine hardware-decoding VLC window is reshaped via Win32, so there is zero added latency and every VLC feature and shortcut keeps working inside the PiP. A ~157KB dependency-free Rust daemon does the work; a tiny Lua extension adds the menu entry. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how it works.

## Install

Download the [latest release](https://github.com/Bug-Finderr/vlc-pip/releases) zip and follow the steps on the release page, or build from source (needs the Rust MSVC toolchain):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install.ps1
```

Restart VLC afterwards. Uninstall the same way with `scripts\uninstall.ps1`.

> Downloaded copies are not code-signed: when SmartScreen shows "Windows protected your PC", click "More info" → "Run anyway" (or build from source).

## Configure

The daemon accepts `w= h= c=br|bl|tr|tl m= min=` (size, corner, margin, minimal look) as startup-shortcut arguments, e.g. `daemon w=640 h=360 c=tr`. Defaults: 480x270, bottom-right, margin 16, `min=1` - minimal look clips the PiP to just the video, no menu or control bar.

## Controls

- **Move**: drag anywhere inside the PiP - it stays where you drop it.
- **Resize**: drag the outer 16px edge or corner band - aspect-locked, from 256px wide up to 80% of the screen's work area.
- Size and nearest corner persist to `%APPDATA%\vlc\pip\config.txt` on release and are reused on the next PiP enter (startup arguments still win; delete the file to reset).
- **Volume**: the mouse wheel already works over the PiP without focusing it (Windows' "scroll inactive windows" is on by default); Ctrl+wheel scales subtitles.

## Notes

- Windows 10/11 x64; VLC 3.x only (3.0.23 verified). VLC 4.0 changes the video window architecture and needs re-validation.
- While in PiP, the F key and double/triple/spam-clicks cannot fullscreen the video; everything behaves normally outside PiP.
- Security model: the helper's IPC files live in per-user `%TEMP%`, so any same-user process can drive the helper - which grants nothing it couldn't already do directly via Win32. For that reason, never run the helper - daemon or one-shot commands - elevated.
- Crashes leave a trace at `%TEMP%\vlc-pip-crash.txt`.

## Contributing

Issues are welcome. PRs are not accepted and will be auto-closed.
