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

# Restore with the old binary before replacing it; an absent owner means reopen-heal is pending.
Resolve-PipState $statePath $installedExe
Stop-InstalledHelper $installedExe $requestPath
Resolve-PipState $statePath $installedExe

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

$startedAt = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
$daemon = $null
try {
    $daemon = Start-Process -FilePath $installedExe -ArgumentList "daemon" -PassThru -WindowStyle Hidden
    $deadline = (Get-Date).AddSeconds(5)
    $verified = $false
    do {
        try {
            $daemon.Refresh()
            if ($daemon.HasExited) { break }
            if (Test-Path -LiteralPath $alivePath -PathType Leaf) {
                $heartbeat = [IO.File]::ReadAllText($alivePath)
                $verified = (Test-InstalledHelperProcess $daemon $installedExe) -and
                    (Test-DaemonHeartbeat $heartbeat ([uint32]$daemon.Id) $startedAt)
            }
        } catch [IO.IOException] { }
        if (-not $verified) { Start-Sleep -Milliseconds 100 }
    } while (-not $verified -and (Get-Date) -lt $deadline)

    if (-not $verified) { throw "daemon startup could not be verified" }
} catch {
    if ($null -ne $daemon -and -not $daemon.HasExited -and
        (Test-InstalledHelperProcess $daemon $installedExe)) {
        try { $daemon.Kill() } catch [InvalidOperationException] { }
        if (-not $daemon.WaitForExit(3000)) { throw "failed daemon did not stop" }
    }
    if (Test-Path -LiteralPath $alivePath -PathType Leaf) {
        try {
            $heartbeat = [IO.File]::ReadAllText($alivePath)
            if ($null -ne $daemon -and
                (Test-DaemonHeartbeat $heartbeat ([uint32]$daemon.Id) $startedAt)) {
                Remove-Item -LiteralPath $alivePath -Force
            }
        } catch [IO.IOException] { }
    }
    throw
}

Write-Host "Installed. Restart VLC to see View > PiP Mode. Hotkey: Ctrl+Alt+P"
