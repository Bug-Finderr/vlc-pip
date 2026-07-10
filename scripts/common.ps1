function Read-PipStatePid([string]$path) {
    $raw = [IO.File]::ReadAllText($path)
    if (-not $raw.EndsWith("`n")) { throw "PiP state is incomplete; refusing to continue" }

    $tokens = @($raw.Trim() -split '\s+')
    $processId = [uint32]0
    if ($tokens.Count -ne 13 -or $tokens[12] -notmatch '^\d+$' -or
        -not [uint32]::TryParse($tokens[12], [ref]$processId)) {
        throw "PiP state is corrupt; refusing to continue"
    }
    $processId
}

function Get-PrebuiltHelper([string]$root) {
    if (Test-Path -LiteralPath "$root\helper\Cargo.toml" -PathType Leaf) { return }
    $executable = "$root\pip-helper.exe"
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "prebuilt package is missing pip-helper.exe"
    }
    $executable
}

function Test-SamePath([string]$left, [string]$right) {
    try {
        [string]::Equals(
            [IO.Path]::GetFullPath($left),
            [IO.Path]::GetFullPath($right),
            [StringComparison]::OrdinalIgnoreCase
        )
    } catch { $false }
}

function Test-InstalledHelperProcess($process, [string]$executable) {
    try { Test-SamePath $process.Path $executable } catch { $false }
}

function Get-InstalledHelperProcess([string]$executable) {
    foreach ($process in @(Get-Process -Name pip-helper -ErrorAction SilentlyContinue)) {
        if (Test-InstalledHelperProcess $process $executable) { $process }
    }
}

function Test-ProcessAlive([uint32]$processId) {
    if ($processId -eq 0 -or $processId -gt [int]::MaxValue) { return $false }
    $null -ne (Get-Process -Id ([int]$processId) -ErrorAction SilentlyContinue)
}

function Resolve-PipState([string]$statePath, [string]$executable, [switch]$requireRestore) {
    if (-not (Test-Path -LiteralPath $statePath -PathType Leaf)) { return }
    $ownerProcess = Read-PipStatePid $statePath
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "PiP state exists but the installed helper is missing"
    }
    if (Test-ProcessAlive $ownerProcess) {
        $null = Start-Process -FilePath $executable -ArgumentList "exit" -PassThru -Wait -WindowStyle Hidden
        if (Test-Path -LiteralPath $statePath) { throw "the active PiP window could not be restored" }
    } elseif ($requireRestore -and (Test-Path -LiteralPath $statePath)) {
        throw "PiP restore is pending; launch VLC, wait for restore, then retry"
    }
}

function Stop-InstalledHelper([string]$executable, [string]$requestPath) {
    $running = @(Get-InstalledHelperProcess $executable)
    if ($running.Count -eq 0) { return }

    Set-Content -LiteralPath $requestPath -Value "stop" -NoNewline
    $deadline = (Get-Date).AddSeconds(5)
    do {
        Start-Sleep -Milliseconds 100
        $running = @(Get-InstalledHelperProcess $executable)
    } while ($running.Count -gt 0 -and (Get-Date) -lt $deadline)

    foreach ($process in $running) {
        if (-not (Test-InstalledHelperProcess $process $executable)) { continue }
        try { $process.Kill() } catch [InvalidOperationException] { continue }
        if (-not $process.WaitForExit(3000)) { throw "pip-helper process $($process.Id) did not stop" }
    }
    if (@(Get-InstalledHelperProcess $executable).Count -gt 0) { throw "installed pip-helper is still running" }
}

function Test-DaemonHeartbeat(
    [string]$line,
    [uint32]$processId,
    [long]$notBefore,
    [long]$now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
) {
    if ($line -notmatch '\A(?<epoch>\d+) pid=(?<process>\d+) hotkey=[01] timer=[01] kb=[01] mouse=[01]\z') {
        return $false
    }
    $epoch = [long]0
    $reportedProcess = [long]0
    if (-not [long]::TryParse($Matches.epoch, [ref]$epoch) -or
        -not [long]::TryParse($Matches.process, [ref]$reportedProcess)) {
        return $false
    }
    $reportedProcess -eq $processId -and $epoch -ge $notBefore -and [Math]::Abs($now - $epoch) -lt 15
}
