$ErrorActionPreference = "SilentlyContinue"
Set-Content "$env:TEMP\vlc-pip-request.txt" "stop"
Start-Sleep 1
Remove-Item "$env:APPDATA\vlc\lua\extensions\pip.lua" -Force
Remove-Item "$env:APPDATA\vlc\pip" -Recurse -Force
Remove-Item ([Environment]::GetFolderPath("Startup") + "\VLC PiP Daemon.lnk") -Force
Remove-Item "$env:TEMP\vlc-pip.json", "$env:TEMP\vlc-pip-request.txt", "$env:TEMP\vlc-pip-daemon.alive", "$env:TEMP\vlc-pip-status.json" -Force
Write-Host "Uninstalled."
