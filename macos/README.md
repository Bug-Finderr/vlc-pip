# VLC PiP for macOS

Puts the playing VLC video into real system Picture in Picture, which floats over every
Space, including other apps' fullscreen ones. It is a separate menu-bar app: it records
VLC's video area with ScreenCaptureKit and drives VLC's own play/pause and seek through
Apple Events. VLC itself is untouched.

Download the zip from the [releases](https://github.com/Jenish-1235/vlc-pip/releases), unzip,
and move **VLC PiP.app** to Applications. Or build it (universal, Apple Silicon + Intel):

```sh
./build.sh           # → build/VLC PiP.app
./build.sh install   # also copies it to /Applications
./build.sh package   # also zips it for a release
```

The app is not notarized: on first open macOS says it can't verify it. Go to System
Settings → Privacy & Security and click **Open Anyway** (or run
`xattr -dr com.apple.quarantine "/Applications/VLC PiP.app"`).

On first use, allow **Screen Recording** (then reopen the app) and the **Automation → VLC**
prompt. The build is ad-hoc signed, so macOS asks for Screen Recording again after a rebuild.

## Use

- Toggle with **⌃⌥P**, the menu-bar icon, reopening the app, or
  `"/Applications/VLC PiP.app/Contents/MacOS/VLCPiP" toggle`.
- The PiP's play/pause, ±10 s and timeline drive VLC; its return button brings VLC forward.
- **Open at Login** and **Use System PiP** live in the menu. With system PiP off, a floating
  panel is used instead: drag to move, edges to resize, double-click to return to VLC.

## Behavior

- Follows VLC between windowed and fullscreen (native and non-native), window moves and
  resizes, and displays with different scaling.
- Trims black bars exactly from the file's real shape (Matroska/WebM headers, or
  AVFoundation for MP4/MOV and others), so the PiP opens at the right shape. A detector
  checks the screen: picture where bars should be (an aspect ratio forced in VLC) wins at
  once; extra bars (letterboxing encoded in the file, or streams with no file) must hold
  a standard shape over 10 s of footage, so dark scenes never crop the picture.
- When playback stops, the PiP holds the last real frame and resumes with the next video.
  Quitting VLC ends the PiP; the app stays in the menu bar.

Requires macOS 14+ and VLC 3.x (verified on 3.0.23, macOS 26.6).
