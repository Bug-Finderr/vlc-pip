# End-to-end smoke test against live VLC. Run AFTER scripts\install.ps1 (daemon must be running).
$ErrorActionPreference = "Stop"
$exe = "$env:APPDATA\vlc\pip\pip-helper.exe"

# WinExe stdout is invisible to PowerShell capture; run the helper and read its
# status-file channel instead of capturing output.
function Status {
    Start-Process $exe status -Wait
    Get-Content "$env:TEMP\vlc-pip-status.json" -Raw | ConvertFrom-Json
}
function Req($cmd) { Set-Content "$env:TEMP\vlc-pip-request.txt" $cmd; Start-Sleep -Milliseconds 600 }
$fail = @()
function Check($name, $cond) { if ($cond) { Write-Host "PASS $name" } else { Write-Host "FAIL $name"; $script:fail += $name } }

if (-not ('Smoke.Keys' -as [type])) {
    Add-Type -Namespace Smoke -Name Keys -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
[DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
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
    Start-Sleep -Milliseconds 700
}
function SendCtrlAltP {
    [Smoke.Keys]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)      # Ctrl down
    [Smoke.Keys]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)      # Alt down
    [Smoke.Keys]::keybd_event(0x50, 0, 0, [UIntPtr]::Zero)      # P down
    [Smoke.Keys]::keybd_event(0x50, 0, 2, [UIntPtr]::Zero)      # P up
    [Smoke.Keys]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)      # Alt up
    [Smoke.Keys]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)      # Ctrl up
    Start-Sleep -Milliseconds 600
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
    Start-Sleep -Milliseconds 1500                            # drag-end + convergence
}
function WheelAt($x, $y) {
    [Smoke.Keys]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 100
    [Smoke.Keys]::mouse_event(0x0800, 0, 0, 120, [UIntPtr]::Zero)  # WHEEL up one notch
    Start-Sleep -Milliseconds 700
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
Start-Sleep 4

try {
    $before = Status
    Check "vlc found" $before.found
    Check "starts with caption" $before.caption

    Req "toggle"; Start-Sleep 1; $pip = Status
    Check "enter: borderless" (-not $pip.caption)
    Check "enter: topmost" $pip.topmost
    Check "enter: video width 480" ($pip.w -eq 480)
    Check "enter: inPip" $pip.inPip
    Check "enter: minimal look (region)" $pip.minimal

    # the two reported bugs: double-click and TRIPLE-click over the PiP video must not fullscreen/resize
    $cx = $pip.x + [int]($pip.w / 2); $cy = $pip.y + [int]($pip.h / 2)
    ClickAt $cx $cy 2; $afterDbl = Status
    Check "double-click: rect unchanged" ($afterDbl.x -eq $pip.x -and $afterDbl.w -eq $pip.w -and $afterDbl.h -eq $pip.h)
    Check "double-click: still inPip" $afterDbl.inPip
    ClickAt $cx $cy 3; $afterTri = Status
    Check "triple-click: rect unchanged" ($afterTri.x -eq $pip.x -and $afterTri.w -eq $pip.w -and $afterTri.h -eq $pip.h)
    Check "triple-click: still inPip" $afterTri.inPip
    ClickAt $cx $cy 5; $afterSpam = Status
    Check "click-spam: rect unchanged" ($afterSpam.x -eq $pip.x -and $afterSpam.w -eq $pip.w -and $afterSpam.h -eq $pip.h)

    # v2.1 gestures: interior drag = free move; band drag = aspect-locked resize; wheel untouched
    DragFrom ($pip.x + [int]($pip.w / 2)) ($pip.y + [int]($pip.h / 2)) ($pip.x + [int]($pip.w / 2) - 220) ($pip.y + [int]($pip.h / 2) - 160)
    $moved = Status
    Check "drag-move: moved by delta" ([math]::Abs($moved.x - ($pip.x - 220)) -le 2 -and [math]::Abs($moved.y - ($pip.y - 160)) -le 2)
    Check "drag-move: size unchanged" ($moved.w -eq $pip.w -and $moved.h -eq $pip.h)
    Check "drag-move: still inPip" $moved.inPip
    Check "drag-move: config has w/h and derived corner" ((Test-Path $cfg) -and ((Get-Content $cfg -Raw).Trim() -match '^w=\d+ h=\d+ c=br$'))

    ClickAt ($moved.x + $moved.w - 8) ($moved.y + [int]($moved.h / 2)) 1
    $bandClick = Status
    Check "band click: rect unchanged" ($bandClick.x -eq $moved.x -and $bandClick.w -eq $moved.w)
    Check "band click: still inPip" $bandClick.inPip

    # right edge at mid-height: horizontal chrome is 0 so window right == visible right,
    # while the top/bottom strips are region-clipped chrome (corner drags are manual)
    DragFrom ($moved.x + $moved.w - 8) ($moved.y + [int]($moved.h / 2)) ($moved.x + $moved.w - 108) ($moved.y + [int]($moved.h / 2))
    Start-Sleep -Milliseconds 1200   # convergence re-clips at the new size
    $rs = Status
    Check "drag-resize (right edge): width shrank" ($rs.w -lt $moved.w)
    Check "drag-resize: still inPip" $rs.inPip
    Check "drag-resize: minimal look held" $rs.minimal

    WheelAt ($rs.x + [int]($rs.w / 2)) ($rs.y + [int]($rs.h / 2))
    $wheeled = Status
    Check "wheel: size untouched" ($wheeled.w -eq $rs.w -and $wheeled.h -eq $rs.h)

    Req "toggle"; $after = Status
    Check "exit: caption restored" $after.caption
    Check "exit: topmost cleared" (-not $after.topmost)
    Check "exit: exact rect" ($after.x -eq $before.x -and $after.y -eq $before.y -and $after.w -eq $before.w -and $after.h -eq $before.h)
    Check "exit: not inPip" (-not $after.inPip)
    Check "exit: region cleared" (-not $after.minimal)

    # persistence: re-enter picks the gestured size from config.txt
    Req "toggle"; Start-Sleep 1; $re = Status
    Check "persist: re-enter at gestured width" ([math]::Abs($re.w - $rs.w) -le 2)
    Req "toggle"

    # global hotkey enters, request-file exits: both paths share one state
    SendCtrlAltP; $hot = Status
    Check "hotkey enters pip" $hot.inPip
    Req "toggle"; $s = Status
    Check "interleaved hotkey+menu do not desync" (-not $s.inPip)
    Check "interleave restored exact rect" ($s.x -eq $before.x -and $s.y -eq $before.y -and $s.w -eq $before.w -and $s.h -eq $before.h)

    # v2.1.1 fullscreen handoff (SPEC section 7). Click the video (a no-op) to focus,
    # then F = VLC's fullscreen hotkey.
    ClickAt ($s.x + [int]($s.w / 2)) ($s.y + [int]($s.h / 2)) 1
    [Smoke.Keys]::keybd_event(0x46, 0, 0, [UIntPtr]::Zero)      # F down
    [Smoke.Keys]::keybd_event(0x46, 0, 2, [UIntPtr]::Zero)      # F up
    Start-Sleep -Milliseconds 800
    $fs = Status
    # caption-only: Qt autoresize can make the windowed size already match the monitor
    Check "fullscreen: engaged" (-not $fs.caption)
    Req "toggle"; Start-Sleep -Milliseconds 1200                 # Esc + two windowed ticks + enter
    $fpip = Status
    Check "fullscreen toggle: enters pip" ($fpip.inPip -and -not $fpip.caption)
    Check "fullscreen toggle: pip-sized, not fullscreen" ($fpip.w -lt [int]($fs.w / 2))
    Req "toggle"; $fout = Status
    Check "fullscreen exit: windowed rect restored" ($fout.caption -and $fout.x -eq $before.x -and $fout.y -eq $before.y -and $fout.w -eq $before.w -and $fout.h -eq $before.h)

    # v2.1 heal: a CLEAN close while in PiP makes Qt persist the PiP geometry as VLC's own
    # (a kill persists nothing and would pass even without the heal - verified), so the
    # reopened window would sit full-size at the PiP origin; the daemon heals it back to
    # the pre-PiP rect and deletes the state once the rect sticks. The state-file check
    # runs via Test-Path BEFORE Status so nothing races the heal's own delete.
    $pre = Status
    Req "enter"; Start-Sleep 1
    $vlcProc.CloseMainWindow() | Out-Null
    if (-not $vlcProc.WaitForExit(8000)) { Stop-Process -Id $vlcProc.Id -Force -Confirm:$false }
    Start-Sleep 1
    $vlcProc = Start-Process $vlcPath 'screen://' -PassThru
    Start-Sleep 8   # startup + heal ticks (apply, verify sticks, delete state)
    Check "reopen heal: state cleared by the heal" (-not (Test-Path "$env:TEMP\vlc-pip.json"))
    $healed = Status
    Check "reopen heal: pre-pip position" ([math]::Abs($healed.x - $pre.x) -le 8 -and [math]::Abs($healed.y - $pre.y) -le 8)
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
