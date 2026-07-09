$ErrorActionPreference = "SilentlyContinue"
# restore VLC FIRST: stopping the daemon and deleting the state file below would strand a
# PiP'd VLC borderless/topmost with no way back (one-shot "exit" is a no-op when not in PiP)
$exe = "$env:APPDATA\vlc\pip\pip-helper.exe"
if ((Test-Path "$env:TEMP\vlc-pip.state") -and (Test-Path $exe)) { Start-Process $exe exit -Wait }
# ask the daemon to exit and wait until the process is gone, so pip-helper.exe is not
# still running (and locked) when the pip folder is deleted below
Set-Content "$env:TEMP\vlc-pip-request.txt" "stop"
$deadline = (Get-Date).AddSeconds(5)
while ((Get-Process pip-helper -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
$left = Get-Process pip-helper -ErrorAction SilentlyContinue
if ($left) { $left | Stop-Process -Force -Confirm:$false; $left | Wait-Process -Timeout 3 -ErrorAction SilentlyContinue }
Remove-Item "$env:APPDATA\vlc\lua\extensions\pip.lua" -Force
Remove-Item "$env:APPDATA\vlc\pip" -Recurse -Force
Remove-Item ([Environment]::GetFolderPath("Startup") + "\VLC PiP Daemon.lnk") -Force
Remove-Item "$env:TEMP\vlc-pip.state", "$env:TEMP\vlc-pip-request.txt", "$env:TEMP\vlc-pip-daemon.alive", "$env:TEMP\vlc-pip-status.json", "$env:TEMP\vlc-pip-crash.txt" -Force
if (Test-Path "$env:APPDATA\vlc\pip") { Write-Warning "helper dir not removed - close whatever locks it and re-run" } else { Write-Host "Uninstalled." }
