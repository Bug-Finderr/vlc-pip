# Installs VLC PiP: helper exe, Lua extension, login autostart, and starts the daemon.
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$dotnet = "$env:LOCALAPPDATA\Microsoft\dotnet\dotnet.exe"
if (-not (Test-Path $dotnet)) { $dotnet = "dotnet" }

& $dotnet publish "$root\helper" -c Release -r win-x64 --self-contained true -p:PublishSingleFile=true -o "$root\publish"
if ($LASTEXITCODE -ne 0) { throw "publish failed" }

$pipDir = "$env:APPDATA\vlc\pip"
$extDir = "$env:APPDATA\vlc\lua\extensions"
New-Item -ItemType Directory -Force $pipDir | Out-Null
New-Item -ItemType Directory -Force $extDir | Out-Null

# stop a running daemon so the exe is not locked
if (Test-Path "$env:TEMP\vlc-pip-daemon.alive") {
    Set-Content "$env:TEMP\vlc-pip-request.txt" "stop"
    Start-Sleep 1
}

Copy-Item "$root\publish\pip-helper.exe" "$pipDir\pip-helper.exe" -Force
Copy-Item "$root\extension\pip.lua" "$extDir\pip.lua" -Force   # ONLY the .lua in extensions

# login autostart shortcut (Explorer-launched = GUI, no console)
$startup = [Environment]::GetFolderPath("Startup")
$ws = New-Object -ComObject WScript.Shell
$lnk = $ws.CreateShortcut("$startup\VLC PiP Daemon.lnk")
$lnk.TargetPath = "$pipDir\pip-helper.exe"
$lnk.Arguments = "daemon"
$lnk.WorkingDirectory = $pipDir
$lnk.Save()

Start-Process "$pipDir\pip-helper.exe" daemon
Write-Host "Installed. Restart VLC to see View > PiP Mode. Hotkey: Ctrl+Alt+P"
