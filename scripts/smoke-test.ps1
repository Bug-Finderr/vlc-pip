# End-to-end smoke test against live VLC. Run AFTER scripts\install.ps1 (daemon must be running).
# One check per behavioral contract; waits poll for their condition instead of sleeping a
# guessed duration - faster, and a late condition fails the check instead of flaking it.
$ErrorActionPreference = "Stop"
$exe = "$env:APPDATA\vlc\pip\pip-helper.exe"

# WinExe stdout is invisible to PowerShell capture; run the helper and read its
# status-file channel instead of capturing output.
function Status {
    Start-Process $exe status -Wait
    Get-Content "$env:TEMP\vlc-pip-status.json" -Raw | ConvertFrom-Json
}
function Req($cmd) { Set-Content "$env:TEMP\vlc-pip-request.txt" $cmd }
# poll until the condition holds (true) or the cap passes (returns the final evaluation)
function WaitFor([scriptblock]$cond, [int]$capMs = 3000, [int]$stepMs = 60) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $capMs) {
        if (& $cond) { return $true }
        Start-Sleep -Milliseconds $stepMs
    }
    & $cond
}
$fail = @()
function Check($name, $cond) { if ($cond) { Write-Host "PASS $name" } else { Write-Host "FAIL $name"; $script:fail += $name } }

if (-not ('Smoke.Keys' -as [type])) {
    Add-Type -Namespace Smoke -Name Keys -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
[DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
[DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr top, EnumProc cb, IntPtr l);
[DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder sb, int max);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
public delegate bool EnumProc(IntPtr h, IntPtr l);
// VLC's vout event thread processes POSTED keys regardless of focus - keybd_event would
// need VLC foreground, and injected focus-clicks from a background session are flaky
public static IntPtr VoutChild(IntPtr top) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(top, (h, l) => {
        var sb = new System.Text.StringBuilder(128);
        GetClassNameW(h, sb, 128);
        if (sb.ToString().StartsWith("VLC video main")) { found = h; return false; }
        return true;
    }, IntPtr.Zero);
    return found;
}
// VLC's fullscreen controller strip: a separate topmost Qt window the helper must keep
// hidden while a fullscreen-origin PiP is active (SPEC section 7)
public static bool FscVisible(IntPtr vlcTop) {
    uint pid; GetWindowThreadProcessId(vlcTop, out pid);
    bool vis = false;
    EnumWindows((h, l) => {
        var sb = new System.Text.StringBuilder(128);
        GetClassNameW(h, sb, 128);
        if (sb.ToString().StartsWith("Qt5QWindowToolSaveBits")) {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p == pid && IsWindowVisible(h)) { vis = true; return false; }
        }
        return true;
    }, IntPtr.Zero);
    return vis;
}
'@
}
function ClickAt($x, $y, $times) {
    [Smoke.Keys]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 100
    for ($i = 0; $i -lt $times; $i++) {
        [Smoke.Keys]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)  # LEFTDOWN
        [Smoke.Keys]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)  # LEFTUP
        Start-Sleep -Milliseconds 80                              # well inside double-click time
    }
    # the NEXT injected button-down must fall outside double-click time of this burst's
    # last ALLOWED down, or the guard swallows it (and a drag would never arm)
    Start-Sleep -Milliseconds 600
}
# post a key to VLC's vout (focus-independent); scan code matters for VLC's translation
function PostKey([long]$top, [int]$vk, [int]$scan) {
    $t = [Smoke.Keys]::VoutChild([IntPtr]::new($top))
    if ($t -eq [IntPtr]::Zero) { $t = [IntPtr]::new($top) }
    [Smoke.Keys]::PostMessageW($t, 0x100, [IntPtr]::new($vk), [IntPtr]::new(0x00000001 -bor ($scan -shl 16))) | Out-Null
    [Smoke.Keys]::PostMessageW($t, 0x101, [IntPtr]::new($vk), [IntPtr]::new(0xC0000001 -bor ($scan -shl 16))) | Out-Null
}
function SendCtrlAltP {
    [Smoke.Keys]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)      # Ctrl down
    [Smoke.Keys]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)      # Alt down
    [Smoke.Keys]::keybd_event(0x50, 0, 0, [UIntPtr]::Zero)      # P down
    [Smoke.Keys]::keybd_event(0x50, 0, 2, [UIntPtr]::Zero)      # P up
    [Smoke.Keys]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)      # Alt up
    [Smoke.Keys]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)      # Ctrl up
}
function DragFrom($x1, $y1, $x2, $y2) {
    # movement must go through injected mouse_event MOVEs: SetCursorPos repositions the
    # cursor without generating input events, so WH_MOUSE_LL (the daemon) never sees it
    $sw = [Smoke.Keys]::GetSystemMetrics(0); $sh = [Smoke.Keys]::GetSystemMetrics(1)
    [Smoke.Keys]::SetCursorPos($x1, $y1) | Out-Null
    Start-Sleep -Milliseconds 150
    [Smoke.Keys]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
    Start-Sleep -Milliseconds 80
    for ($i = 1; $i -le 10; $i++) {
        $px = $x1 + [int](($x2 - $x1) * $i / 10); $py = $y1 + [int](($y2 - $y1) * $i / 10)
        # MOVE|ABSOLUTE (0x8001), coords normalized to 0..65535 over the primary screen
        [Smoke.Keys]::mouse_event(0x8001, [uint32]($px * 65535 / ($sw - 1)), [uint32]($py * 65535 / ($sh - 1)), 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 25
    }
    [Smoke.Keys]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
    Start-Sleep -Milliseconds 400                             # drag-end write lands
}
function HoverWiggle($cx, $cy) {
    $sw = [Smoke.Keys]::GetSystemMetrics(0); $sh = [Smoke.Keys]::GetSystemMetrics(1)
    for ($i = 0; $i -lt 8; $i++) {
        $px = $cx - 40 + $i * 10; $py = $cy - 15 + ($i % 3) * 10
        [Smoke.Keys]::mouse_event(0x8001, [uint32]($px * 65535 / ($sw - 1)), [uint32]($py * 65535 / ($sh - 1)), 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 60
    }
}

Check "daemon alive" (Test-Path "$env:TEMP\vlc-pip-daemon.alive")

$vlcDir = (Get-ItemProperty 'HKLM:\SOFTWARE\VideoLAN\VLC' -ErrorAction SilentlyContinue).InstallDir
$vlcPath = if ($vlcDir) { Join-Path $vlcDir 'vlc.exe' } else { 'C:\Program Files\VideoLAN\VLC\vlc.exe' }
if (-not (Test-Path $vlcPath)) { throw "vlc.exe not found" }
if (Get-Process vlc -ErrorAction SilentlyContinue) {
    throw "Close VLC first: this test resizes, clicks, and kills the VLC instance it targets"
}
# v2.1: gestures persist to config.txt - park it so the run starts from defaults
$cfg = "$env:APPDATA\vlc\pip\config.txt"
$cfgBak = "$cfg.smoke-bak"
if (Test-Path $cfg) { Move-Item $cfg $cfgBak -Force }

# screen:// = live playing video, so the video child window and minimal-look region exist
$vlcProc = Start-Process $vlcPath 'screen://' -PassThru
# $before anchors every exact-rect check: wait for Qt's startup autoresize to settle
# (two identical consecutive rect samples), not a guessed duration
$prevR = $null
for ($i = 0; $i -lt 20; $i++) {
    $cur = Status
    if ($cur.found -and $prevR -and $cur.x -eq $prevR.x -and $cur.y -eq $prevR.y -and $cur.w -eq $prevR.w -and $cur.h -eq $prevR.h) { break }
    $prevR = $cur
    Start-Sleep -Milliseconds 300
}

try {
    $before = Status
    Check "vlc ready (windowed, caption)" ($before.found -and $before.caption)

    Req "toggle"
    $null = WaitFor { (Status).minimal } 4000 150
    $pip = Status
    Check "enter: pip formed (borderless, topmost, video 480w, region, state)" `
        ((-not $pip.caption) -and $pip.topmost -and $pip.w -eq 480 -and $pip.inPip -and $pip.minimal)

    # fullscreen prevention: a 5-click burst subsumes double and triple click - every
    # down after the first ALLOWED one must be swallowed, so no OS double-click can
    # ever synthesize (v1 bugs: dblclick fullscreened; clicks 1+3 paired on triple)
    $cx = $pip.x + [int]($pip.w / 2); $cy = $pip.y + [int]($pip.h / 2)
    ClickAt $cx $cy 5; $afterSpam = Status
    Check "click burst (dbl/triple/spam): rect unchanged, still pip" `
        ($afterSpam.x -eq $pip.x -and $afterSpam.w -eq $pip.w -and $afterSpam.h -eq $pip.h -and $afterSpam.inPip)

    # v2.1 gestures: interior drag = free move; band drag = aspect-locked resize; wheel untouched
    DragFrom $cx $cy ($cx - 220) ($cy - 160)
    $moved = Status
    Check "drag-move: at delta, size held, still pip" `
        ([math]::Abs($moved.x - ($pip.x - 220)) -le 2 -and [math]::Abs($moved.y - ($pip.y - 160)) -le 2 -and $moved.w -eq $pip.w -and $moved.h -eq $pip.h -and $moved.inPip)
    Check "drag-move: config persisted (w/h + corner)" `
        ((Test-Path $cfg) -and ((Get-Content $cfg -Raw).Trim() -match '^w=\d+ h=\d+ c=br$'))

    ClickAt ($moved.x + $moved.w - 8) ($moved.y + [int]($moved.h / 2)) 1
    $bandClick = Status
    Check "band click: no resize, no move" ($bandClick.x -eq $moved.x -and $bandClick.w -eq $moved.w -and $bandClick.inPip)

    # right edge at mid-height: horizontal chrome is 0 so window right == visible right,
    # while the top/bottom strips are region-clipped chrome (corner drags are manual)
    DragFrom ($moved.x + $moved.w - 8) ($moved.y + [int]($moved.h / 2)) ($moved.x + $moved.w - 108) ($moved.y + [int]($moved.h / 2))
    $null = WaitFor { $s = Status; $s.w -lt $moved.w -and $s.minimal } 4000 150   # convergence re-clips
    $rs = Status
    Check "drag-resize (right edge): width shrank, minimal look held" ($rs.w -lt $moved.w -and $rs.inPip -and $rs.minimal)

    [Smoke.Keys]::SetCursorPos(($rs.x + [int]($rs.w / 2)), ($rs.y + [int]($rs.h / 2))) | Out-Null
    Start-Sleep -Milliseconds 100
    [Smoke.Keys]::mouse_event(0x0800, 0, 0, 120, [UIntPtr]::Zero)   # WHEEL up one notch
    Start-Sleep -Milliseconds 400
    $wheeled = Status
    Check "wheel: size untouched (volume, not resize)" ($wheeled.w -eq $rs.w -and $wheeled.h -eq $rs.h)

    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $after = Status
    Check "exit: exact windowed restore (caption, rect, topmost/region/state cleared)" `
        ($after.caption -and (-not $after.topmost) -and (-not $after.inPip) -and (-not $after.minimal) -and $after.x -eq $before.x -and $after.y -eq $before.y -and $after.w -eq $before.w -and $after.h -eq $before.h)

    # persistence: re-enter picks the gestured size from config.txt
    Req "toggle"
    $null = WaitFor { (Status).inPip } 3000 150
    $re = Status
    Check "persist: re-enter at gestured width" ([math]::Abs($re.w - $rs.w) -le 2)
    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150

    # global hotkey enters, request-file exits: both paths share one state
    SendCtrlAltP
    $null = WaitFor { (Status).inPip } 3000 150
    Check "hotkey enters pip" ((Status).inPip)
    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $s = Status
    Check "interleave hotkey+request: no desync, exact rect" `
        ((-not $s.inPip) -and $s.x -eq $before.x -and $s.y -eq $before.y -and $s.w -eq $before.w -and $s.h -eq $before.h)

    # v2.1.1 fullscreen-origin PiP (SPEC section 7). F posted to the vout = VLC's
    # fullscreen hotkey, focus-independent.
    PostKey $s.hwnd 0x46 0x21
    # caption-only: Qt autoresize can make the windowed size already match the monitor
    $null = WaitFor { -not (Status).caption } 2500 150
    $fs = Status
    Check "fullscreen: engaged" (-not $fs.caption)

    # summon the controller strip (hover the fullscreen video): the realistic toggle
    # happens with the strip on screen, and enter() must hide it before the reshape
    $mw = [Smoke.Keys]::GetSystemMetrics(0); $mh = [Smoke.Keys]::GetSystemMetrics(1)
    HoverWiggle ([int]($mw / 2)) ([int]($mh / 2))
    $stripUp = [Smoke.Keys]::FscVisible([IntPtr]::new([long]$fs.hwnd))

    # the enter is one immediate reshape - no handoff wait
    Req "toggle"
    $hsw = [System.Diagnostics.Stopwatch]::StartNew()
    $null = WaitFor { Test-Path "$env:TEMP\vlc-pip.json" } 2000 15
    $enterMs = $hsw.ElapsedMilliseconds
    Check "strip: summoned, then gone before the pip lands" `
        ($stripUp -and -not [Smoke.Keys]::FscVisible([IntPtr]::new([long]$fs.hwnd)))
    $null = WaitFor { (Status).inPip } 2000 150
    $fpip = Status
    Check "fullscreen enter: immediate (<500ms), pip-sized" `
        ($enterMs -lt 500 -and $fpip.inPip -and (-not $fpip.caption) -and $fpip.w -lt [int]($fs.w / 2))

    # VLC still believes it is fullscreen underneath: hovering the PiP must not surface
    # the strip (enter hid it once; the tick re-hides after VLC's own resync)
    HoverWiggle ($fpip.x + [int]($fpip.w / 2)) ($fpip.y + [int]($fpip.h / 2))
    Start-Sleep -Milliseconds 400   # a first-hover strip may blink for one tick (SPEC)
    Check "strip: stays hidden through hover" (-not [Smoke.Keys]::FscVisible([IntPtr]::new([long]$fs.hwnd)))

    # exit returns the user to fullscreen - where they came from, internally consistent
    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $fout = Status
    Check "fullscreen exit: fullscreen restored" ((-not $fout.caption) -and (-not $fout.inPip) -and $fout.w -eq $fs.w -and $fout.h -eq $fs.h)

    # both toggles are instant: two rapid presses = a full round trip, no half-states
    Req "toggle"
    $null = WaitFor { -not (Test-Path "$env:TEMP\vlc-pip-request.txt") } 1500 25    # first consumed
    Req "toggle"
    $null = WaitFor { $d = Status; (-not $d.inPip) -and (-not $d.caption) } 3000 150
    $fdbl = Status
    Check "fullscreen double-toggle: clean round trip" ((-not $fdbl.inPip) -and (-not $fdbl.caption) -and $fdbl.w -eq $fs.w)

    # leave fullscreen via VLC itself: the window was untouched while fullscreen, so
    # Qt's own restore must land the exact pre-fullscreen rect
    PostKey $fdbl.hwnd 0x46 0x21
    $null = WaitFor { $w = Status; $w.caption -and $w.w -eq $before.w } 3000 150
    $fw = Status
    Check "fullscreen left: original windowed rect intact" `
        ($fw.caption -and $fw.x -eq $before.x -and $fw.y -eq $before.y -and $fw.w -eq $before.w -and $fw.h -eq $before.h)

    # stopping playback inside a fullscreen-origin PiP: VLC leaves fullscreen by itself
    # and balloons the window - the daemon must dissolve the session into a plain
    # windowed VLC (frame back, state dropped), never restore the fullscreen shell
    PostKey $fw.hwnd 0x46 0x21
    $null = WaitFor { -not (Status).caption } 2500 150
    Req "toggle"
    $null = WaitFor { (Status).inPip } 2000 150
    $fsp = Status
    PostKey $fsp.hwnd 0x53 0x1F                                  # S = VLC stop
    $null = WaitFor { $d = Status; $d.caption -and -not (Test-Path "$env:TEMP\vlc-pip.json") } 3000 150
    $dis = Status
    Check "stop in fullscreen pip: session dissolves windowed" ($dis.caption -and -not $dis.inPip -and -not (Test-Path "$env:TEMP\vlc-pip.json"))

    # v2.1 heal: a CLEAN close while in PiP makes Qt persist the PiP geometry as VLC's own
    # (a kill persists nothing and would pass even without the heal - verified), so the
    # reopened window would sit full-size at the PiP origin; the daemon heals it back to
    # the pre-PiP rect and deletes the state once the rect sticks. The state-file check
    # polls Test-Path so nothing races the heal's own delete.
    $pre = Status
    Req "enter"
    $null = WaitFor { (Status).inPip } 3000 150
    $vlcProc.CloseMainWindow() | Out-Null
    if (-not $vlcProc.WaitForExit(8000)) { Stop-Process -Id $vlcProc.Id -Force -Confirm:$false }
    Start-Sleep 1
    $vlcProc = Start-Process $vlcPath 'screen://' -PassThru
    $healDone = WaitFor { -not (Test-Path "$env:TEMP\vlc-pip.json") } 12000 300   # startup + apply + stick + delete
    Start-Sleep -Milliseconds 400
    $healed = Status
    Check "reopen heal: state cleared, window at pre-pip position" `
        ($healDone -and [math]::Abs($healed.x - $pre.x) -le 16 -and [math]::Abs($healed.y - $pre.y) -le 16)
    # clean-close this instance too so VLC persists the HEALED geometry (the finally
    # force-kill would strand the PiP rect in vlc-qt-interface.ini for the next launch)
    $vlcProc.CloseMainWindow() | Out-Null
    $vlcProc.WaitForExit(8000) | Out-Null
}
finally {
    # restore the window if the test aborted mid-PiP (no-op otherwise, works without the daemon)
    try { Start-Process $exe exit -Wait } catch {}
    # put the user's config back (or clear the one this run wrote) before tearing VLC down
    if (Test-Path $cfgBak) { Move-Item $cfgBak $cfg -Force }
    elseif (Test-Path $cfg) { Remove-Item $cfg -Force -ErrorAction SilentlyContinue }
    if ($vlcProc -and -not $vlcProc.HasExited) { Stop-Process -Id $vlcProc.Id -Force -Confirm:$false -ErrorAction SilentlyContinue }
}

if ($fail.Count) { Write-Host "`n$($fail.Count) FAILURES"; exit 1 } else { Write-Host "`nALL PASS"; exit 0 }
