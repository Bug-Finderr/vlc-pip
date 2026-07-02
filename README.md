# vlc-pip

Turns the **real** VLC 3.x window into a borderless, always-on-top, corner-parked mini player - toggled from **View → PiP Mode** or **Ctrl+Alt+P** - and restores it to its exact original size, position, and borders on toggle back.

No mirroring, no second player: the genuine hardware-decoding VLC window is reshaped via Win32, so there is zero added latency and every VLC feature and shortcut keeps working inside the PiP. A ~165KB dependency-free Rust daemon does the work; a tiny Lua extension adds the menu entry. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how it works.

## Install

Download the [latest release](https://github.com/Bug-Finderr/vlc-pip/releases) zip and follow the steps on the release page, or build from source (needs the Rust MSVC toolchain):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install.ps1
```

Restart VLC afterwards. Uninstall the same way with `scripts\uninstall.ps1`.

> Downloaded copies are not code-signed: when SmartScreen shows "Windows protected your PC", click "More info" → "Run anyway" (or build from source).

## Configure

The daemon accepts `w= h= c=br|bl|tr|tl m= min=` (size, corner, margin, minimal look) as startup-shortcut arguments, e.g. `daemon w=640 h=360 c=tr`. Defaults: 480x270, bottom-right, margin 16, `min=1` - minimal look clips the PiP to just the video, no menu or control bar.

## Notes

- Windows 10/11 x64; VLC 3.x only (3.0.23 verified). VLC 4.0 changes the video window architecture and needs re-validation.
- While in PiP, the F key and double/triple/spam-clicks cannot fullscreen the video; everything behaves normally outside PiP.
- Security model: the helper's IPC files live in per-user `%TEMP%`, so any same-user process can drive the helper - which grants nothing it couldn't already do directly via Win32. For that reason, never run the daemon elevated.
- Crashes leave a trace at `%TEMP%\vlc-pip-crash.txt`.

## Contributing

Issues are welcome. PRs are not accepted and will be auto-closed.

## Additional

- [MIT LICENSE](LICENSE).
- [ARCHITECTURE.md](docs/ARCHITECTURE.md) - how it works.
- [SPEC.md](docs/SPEC.md) - the full behavioral contract, plus the gotchas that each cost a real bug.
- [docs/plans/](docs/plans/) - implementation trails for the v1 C# build and the v2 Rust rewrite.
- v1 (C#/.NET NativeAOT, 2.26MB vs v2's 165KB) is preserved at [v1.0.0](https://github.com/Bug-Finderr/vlc-pip/releases/tag/v1.0.0).
