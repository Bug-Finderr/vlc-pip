$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
. "$PSScriptRoot\common.ps1"

$exeSource = Get-PrebuiltHelper $root
if ($null -eq $exeSource) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "cargo not found - install the Rust MSVC toolchain from https://rustup.rs"
    }
    cargo build --release --manifest-path "$root\helper\Cargo.toml"
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
    $exeSource = "$root\helper\target\release\pip-helper.exe"
}

$luaSource = "$root\extension\pip.lua"
if (-not (Test-Path -LiteralPath $luaSource -PathType Leaf)) { throw "extension\pip.lua is missing" }

$pipDir = "$env:APPDATA\vlc\pip"
$installedExe = "$pipDir\pip-helper.exe"
$statePath = "$env:TEMP\vlc-pip.state"
$requestPath = "$env:TEMP\vlc-pip-request.txt"
$alivePath = "$env:TEMP\vlc-pip-daemon.alive"

# Stop first so no timer tick can reapply a stale fullscreen veil after restoration.
Assert-PipStatePrerequisites $statePath $installedExe
Stop-InstalledHelper $installedExe $requestPath
try { Resolve-PipState $statePath $installedExe }
catch {
    $restoreError = $_
    Start-InstalledDaemon $installedExe $alivePath
    throw $restoreError
}

foreach ($path in @($requestPath, $alivePath, "$env:TEMP\vlc-pip.json")) {
    if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
}

$extensionDir = "$env:APPDATA\vlc\lua\extensions"
New-Item -ItemType Directory -Path $pipDir -Force | Out-Null
New-Item -ItemType Directory -Path $extensionDir -Force | Out-Null
Copy-Item -LiteralPath $exeSource -Destination $installedExe -Force
Copy-Item -LiteralPath $luaSource -Destination "$extensionDir\pip.lua" -Force

$startup = [Environment]::GetFolderPath("Startup")
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut("$startup\VLC PiP Daemon.lnk")
$shortcut.TargetPath = $installedExe
$shortcut.Arguments = "daemon"
$shortcut.Save()

Start-InstalledDaemon $installedExe $alivePath

Write-Host "Installed. Restart VLC to see View > PiP Mode. Hotkey: Ctrl+Alt+P"
