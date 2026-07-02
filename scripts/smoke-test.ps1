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

Check "daemon alive" (Test-Path "$env:TEMP\vlc-pip-daemon.alive")

$vlcDir = (Get-ItemProperty 'HKLM:\SOFTWARE\VideoLAN\VLC' -ErrorAction SilentlyContinue).InstallDir
$vlcPath = if ($vlcDir) { Join-Path $vlcDir 'vlc.exe' } else { 'C:\Program Files\VideoLAN\VLC\vlc.exe' }
if (-not (Test-Path $vlcPath)) { throw "vlc.exe not found" }
if (Get-Process vlc -ErrorAction SilentlyContinue) {
    throw "Close VLC first: this test resizes, clicks, and kills the VLC instance it targets"
}
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

    Req "toggle"; $after = Status
    Check "exit: caption restored" $after.caption
    Check "exit: topmost cleared" (-not $after.topmost)
    Check "exit: exact rect" ($after.x -eq $before.x -and $after.y -eq $before.y -and $after.w -eq $before.w -and $after.h -eq $before.h)
    Check "exit: not inPip" (-not $after.inPip)
    Check "exit: region cleared" (-not $after.minimal)

    # global hotkey enters, request-file exits: both paths share one state
    SendCtrlAltP; $hot = Status
    Check "hotkey enters pip" $hot.inPip
    Req "toggle"; $s = Status
    Check "interleaved hotkey+menu do not desync" (-not $s.inPip)
    Check "interleave restored exact rect" ($s.x -eq $before.x -and $s.y -eq $before.y -and $s.w -eq $before.w -and $s.h -eq $before.h)
}
finally {
    # restore the window if the test aborted mid-PiP (no-op otherwise, works without the daemon)
    try { Start-Process $exe exit -Wait } catch {}
    if ($vlcProc -and -not $vlcProc.HasExited) { Stop-Process -Id $vlcProc.Id -Force -Confirm:$false -ErrorAction SilentlyContinue }
}

if ($fail.Count) { Write-Host "`n$($fail.Count) FAILURES"; exit 1 } else { Write-Host "`nALL PASS"; exit 0 }
