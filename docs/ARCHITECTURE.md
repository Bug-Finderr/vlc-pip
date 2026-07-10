# Architecture

VLC's Lua extension API has no window-management surface, so the extension is only a trigger. All real work happens in `pip-helper.exe` - a tiny Rust GUI-subsystem daemon with no extra runtime dependency (`windows-sys` is its only crate dependency). It reshapes VLC's **own** top-level window via Win32, adding no mirror, second player, or video latency.

## Toggle flow

```mermaid
sequenceDiagram
    participant U as User
    participant L as pip.lua (VLC View menu)
    participant R as vlc-pip-request.txt
    participant D as pip-helper.exe daemon
    participant V as VLC window

    U->>L: click "PiP Mode" (or Ctrl+Alt+P straight to D)
    L->>R: write "toggle" (pure Lua I/O, no console flash)
    D->>R: consume on next 150 ms tick
    D->>V: Enter: save rect+styles to vlc-pip.state,<br>strip caption/frame, topmost, park in corner
    D->>V: Exit: restore saved styles, topmost state,<br>and exact rect; delete state on success
```

A valid `%TEMP%\vlc-pip.state` is either an owned live PiP or a stale pending reopen-heal record. `owns_state` checks the recorded HWND and PID to decide whether PiP is active, so a recycled handle cannot activate hooks or input guards. Enter creates the record; drag release can update its target size and corner. Menu and hotkey both call the same toggle path, while a pending record remains available for reopen heal.

## Daemon internals

```mermaid
flowchart LR
    subgraph D["daemon (single instance via named mutex, raw GetMessage pump)"]
        T["WM_TIMER 150 ms<br>consume request, sync, converge region,<br>resync on drop, heartbeat ~3 s"]
        H["WM_HOTKEY<br>Ctrl+Alt+P = toggle"]
        K["WH_KEYBOARD_LL<br>swallow F while in PiP + VLC focused"]
        M["WH_MOUSE_LL<br>rate-limit clicks over the PiP,<br>arm + track drag gestures"]
        G["WM_APP drag (coalesced)<br>pump computes rect, applies async,<br>release: persist size + corner"]
    end
    M --> G
    T --> FILES["runtime files in TEMP"]
    K & M -.->|read pump-thread cache,<br>never the disk| CACHE["cached HWND + PID"]
```

Key mechanisms, each earned by a v1 bug (details in [SPEC.md](SPEC.md) §7-8):

- **Fullscreen prevention is preventive, not reactive.** A poll-and-snap-back guard flickers; instead the mouse hook swallows every button-down within double-click time/rect of the last *allowed* down, so the OS can never synthesize `WM_LBUTTONDBLCLK` (swallowing only the 2nd click let clicks 1+3 pair - triple-click fullscreened). Every keyboard/mouse suppression first revalidates that the cached HWND still belongs to the cached PID.
- **Hooks never touch the disk.** File I/O in a low-level hook risks `LowLevelHooksTimeout`, after which Windows silently removes the hook. Hooks read an HWND cache refreshed on the pump thread, including immediately after maintenance ends a session.
- **Hooks follow the owned PiP session.** The idle daemon has no LL hooks. Each null slot installs independently while a session is active; failed installs retry without duplicating a live hook. If an external one-shot exit races a timer holding the old state, the next sync reapplies the cached restore snapshot only after confirming the state file is absent. Terminal daemon transitions skip that repair. Failed hook removals retry, with the cleared cache making any retained hook pass input through.
- **The pump handles thread messages directly.** It creates no windows and accepts no text input, so its `WM_HOTKEY`, `WM_TIMER`, and coalesced `WM_APP` messages need neither `TranslateMessage` nor `DispatchMessageW`.
- **Minimal look** (menu/controls hidden, like Ctrl+H) clips the window to VLC's video child via `SetWindowRgn`, growing the window by the chrome delta so the visible video is exactly the target size. VLC re-fits the child asynchronously, so the converger acts only on measurements stable across two ticks, with a 0-300px chrome sanity clamp.
- **The heartbeat file** (`vlc-pip-daemon.alive`, epoch + arming flags, rewritten ~3 s) is how `pip.lua` decides liveness - a force-killed daemon can't delete a marker file, so existence alone is not liveness.
- **Fullscreen-origin PiP (v2.1.1).** Entering PiP from a fullscreen VLC is the same instant reshape as any enter - VLC's internal fullscreen state stays on for the whole PiP session because Qt only restores its windowed geometry from an untouched fullscreen window. Entry hides a currently visible controller strip once and applies an empty-region veil; ticks only repair that persistent veil if VLC recreates or reshapes a controller. The keyboard hook swallows Esc alongside F. Exit restores the saved fullscreen style, topmost state, and rect. When playback ends or stops, the daemon detects Qt's own windowed re-layout and dissolves the session there ([SPEC.md](SPEC.md) §7).
- **Drag gestures (v2.1) ride the same mouse hook.** An allowed button-down over the PiP arms a gesture (interior `(0, 0)` = free move; each `-1|0|1` axis selects a low edge, interior, or high edge for aspect-locked resize); the hook stores the latest cursor position and posts one coalesced `WM_APP` message with a generation counter. The pump widens pointer deltas, revalidates HWND ownership, rejects unrepresentable move rects, computes and applies the target - finalizing on release from its own computed rect, never `GetWindowRect` after the async `SetWindowPos` - and persists size + nearest corner to `config.txt`. Full contract: [SPEC.md](SPEC.md) §12.
- **Close-in-PiP heal (v2.1).** VLC closed while in PiP saves the PiP geometry as its own; the daemon keeps the stale state as a pending-restore record and re-applies the pre-PiP rect once a new player window appears, deleting the state only when the rect sticks ([SPEC.md](SPEC.md) §12).

## Layout

| Piece | Lives at |
|---|---|
| `pip.lua` trigger extension | `%APPDATA%\vlc\lua\extensions\` (only the .lua - a stray exe breaks VLC's extension scan) |
| `pip-helper.exe` | `%APPDATA%\vlc\pip\` |
| Autostart | `shell:startup\VLC PiP Daemon.lnk` → `pip-helper.exe daemon` |
| Runtime state | `%TEMP%\vlc-pip*` (state, request, heartbeat, status, crash) |
| Persisted size/corner | `%APPDATA%\vlc\pip\config.txt` (written on drag release) |

CLI modes: `toggle|enter|exit|restore|status|daemon|stop`. `restore` is the installer's non-destructive owned-state restore; `status` writes `%TEMP%\vlc-pip-status.json` because a GUI-subsystem exe's stdout is unreliable.

## Development

```powershell
cargo test --manifest-path helper\Cargo.toml          # pure-logic unit tests
powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1   # end-to-end against live VLC
```

The smoke gate verifies daemon identity and the live geometry, topmost restore, input guards, gestures, fullscreen veil, and reopen-heal paths. Its non-primary-monitor absolute-input probe runs only on multi-monitor hosts. [SPEC.md](SPEC.md) is the full contract and acceptance checklist.
