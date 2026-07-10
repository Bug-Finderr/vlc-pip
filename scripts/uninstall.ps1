$ErrorActionPreference = "Stop"
. "$PSScriptRoot\common.ps1"

$pipDir = "$env:APPDATA\vlc\pip"
$installedExe = "$pipDir\pip-helper.exe"
$statePath = "$env:TEMP\vlc-pip.state"
$requestPath = "$env:TEMP\vlc-pip-request.txt"

# A pending reopen-heal record must survive until VLC relaunches and is restored.
Resolve-PipState $statePath $installedExe -RequireRestore
Stop-InstalledHelper $installedExe $requestPath
Resolve-PipState $statePath $installedExe -RequireRestore

$files = @(
    "$env:APPDATA\vlc\lua\extensions\pip.lua",
    ([Environment]::GetFolderPath("Startup") + "\VLC PiP Daemon.lnk"),
    $requestPath,
    "$env:TEMP\vlc-pip-daemon.alive",
    "$env:TEMP\vlc-pip-status.json",
    "$env:TEMP\vlc-pip-crash.txt",
    "$env:TEMP\vlc-pip.json"
)
foreach ($path in $files) {
    if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
}
if (Test-Path -LiteralPath $pipDir) { Remove-Item -LiteralPath $pipDir -Recurse -Force }

Write-Host "Uninstalled."
