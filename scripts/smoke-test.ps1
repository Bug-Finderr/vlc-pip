# End-to-end smoke test against live VLC. Run AFTER scripts\install.ps1 (daemon must be running).
$ErrorActionPreference = "Stop"
$exe = "$env:APPDATA\vlc\pip\pip-helper.exe"

function Status {
    Start-Process $exe status -Wait
    Get-Content "$env:TEMP\vlc-pip-status.json" -Raw | ConvertFrom-Json
}
function Req($cmd) { Set-Content "$env:TEMP\vlc-pip-request.txt" $cmd; Start-Sleep -Milliseconds 600 }
$fail = @()
function Check($name, $cond) { if ($cond) { Write-Host "PASS $name" } else { Write-Host "FAIL $name"; $script:fail += $name } }

Add-Type -Namespace Smoke -Name Keys -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
'@
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

if (-not (Get-Process vlc -ErrorAction SilentlyContinue)) {
    Start-Process "C:\Program Files\VideoLAN\VLC\vlc.exe"; Start-Sleep 3
}
$before = Status
Check "vlc found" $before.found
Check "starts with caption" $before.caption

Req "toggle"; $pip = Status
Check "enter: borderless" (-not $pip.caption)
Check "enter: topmost" $pip.topmost
Check "enter: 480x270" ($pip.w -eq 480 -and $pip.h -eq 270)
Check "enter: inPip" $pip.inPip

Req "toggle"; $after = Status
Check "exit: caption restored" $after.caption
Check "exit: topmost cleared" (-not $after.topmost)
Check "exit: exact rect" ($after.x -eq $before.x -and $after.y -eq $before.y -and $after.w -eq $before.w -and $after.h -eq $before.h)
Check "exit: not inPip" (-not $after.inPip)

# global hotkey enters, request-file exits: both paths share one state
SendCtrlAltP; $hot = Status
Check "hotkey enters pip" $hot.inPip
Req "toggle"; $s = Status
Check "interleaved hotkey+menu do not desync" (-not $s.inPip)
Check "interleave restored exact rect" ($s.x -eq $before.x -and $s.y -eq $before.y -and $s.w -eq $before.w -and $s.h -eq $before.h)

Stop-Process -Name vlc -Force -Confirm:$false
if ($fail.Count) { Write-Host "`n$($fail.Count) FAILURES"; exit 1 } else { Write-Host "`nALL PASS"; exit 0 }
