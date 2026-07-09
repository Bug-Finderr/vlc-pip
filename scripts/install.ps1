# Installs VLC PiP: helper exe, Lua extension, login autostart, and starts the daemon.
# Release zips ship a prebuilt pip-helper.exe at the root; a source clone builds it.
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$exeSrc = "$root\pip-helper.exe"
if ((Test-Path $exeSrc) -and -not (Test-Path "$root\helper\Cargo.toml")) {
    # no source tree = release zip: install the shipped exe. A clone always builds -
    # a stray root exe must never silently shadow the source.
    Write-Host "Installing prebuilt pip-helper.exe"
} else {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw "cargo not found - install Rust (MSVC toolchain) from https://rustup.rs" }
    cargo build --release --manifest-path "$root\helper\Cargo.toml"
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
    $exeSrc = "$root\helper\target\release\pip-helper.exe"
}

$pipDir = "$env:APPDATA\vlc\pip"
$extDir = "$env:APPDATA\vlc\lua\extensions"
New-Item -ItemType Directory -Force $pipDir | Out-Null
New-Item -ItemType Directory -Force $extDir | Out-Null


# stop a running daemon so the exe is not locked (gate on the PROCESS: the alive file
# can be stale after a force-kill, or purged by Storage Sense while the daemon runs)
if (Get-Process pip-helper -ErrorAction SilentlyContinue) {
    Set-Content "$env:TEMP\vlc-pip-request.txt" "stop"
    $deadline = (Get-Date).AddSeconds(5)
    while ((Get-Process pip-helper -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
    $left = Get-Process pip-helper -ErrorAction SilentlyContinue
    if ($left) { $left | Stop-Process -Force -Confirm:$false; $left | Wait-Process -Timeout 3 -ErrorAction SilentlyContinue }
}
# a stale request (an unconsumed "stop") would make the fresh daemon act on it within 150ms;
# a stale heartbeat (survives any force-kill) would make the start verification below vacuous
Remove-Item "$env:TEMP\vlc-pip-request.txt", "$env:TEMP\vlc-pip-daemon.alive" -Force -ErrorAction SilentlyContinue

Copy-Item $exeSrc "$pipDir\pip-helper.exe" -Force
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
$deadline = (Get-Date).AddSeconds(5)
while (-not (Test-Path "$env:TEMP\vlc-pip-daemon.alive") -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
if (-not (Test-Path "$env:TEMP\vlc-pip-daemon.alive")) { throw "daemon did not start (no $env:TEMP\vlc-pip-daemon.alive after 5s)" }
Write-Host "Installed. Restart VLC to see View > PiP Mode. Hotkey: Ctrl+Alt+P"
