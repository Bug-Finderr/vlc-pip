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

function Assert-PipStatePrerequisites([string]$statePath, [string]$executable) {
    if (-not (Test-Path -LiteralPath $statePath -PathType Leaf)) { return }
    $null = Read-PipStatePid $statePath
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "PiP state exists but the installed helper is missing"
    }
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

function Resolve-PipState([string]$statePath, [string]$executable, [switch]$requireRestore) {
    if (-not (Test-Path -LiteralPath $statePath -PathType Leaf)) { return }
    Assert-PipStatePrerequisites $statePath $executable
    $restore = Start-Process -FilePath $executable -ArgumentList "restore" -PassThru -Wait -WindowStyle Hidden
    $stateExists = Test-Path -LiteralPath $statePath -PathType Leaf
    if ($restore.ExitCode -eq 0 -and -not $stateExists) { return }
    if ($restore.ExitCode -eq 4 -and $stateExists) {
        if ($requireRestore) { throw "PiP restore is pending; launch VLC, wait for restore, then retry" }
        return
    }
    throw "the active PiP window could not be restored safely"
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

function Stop-StartedProcess($process) {
    if (-not $process.HasExited) {
        try { $process.Kill() } catch [InvalidOperationException] { }
    }
    if (-not $process.WaitForExit(3000)) { throw "failed daemon did not stop" }
}

function Remove-OrphanedHeartbeat([string]$path) {
    if (@(Get-Process -Name pip-helper -ErrorAction SilentlyContinue).Count -eq 0 -and
        (Test-Path -LiteralPath $path -PathType Leaf)) {
        Remove-Item -LiteralPath $path -Force
    }
}

function Test-DaemonHeartbeat(
    [string]$line,
    [uint32]$processId,
    [long]$notBefore,
    [long]$now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
) {
    if ($line -notmatch '\A(?<epoch>\d+) pid=(?<process>\d+) hotkey=1 timer=1 kb=[01] mouse=[01]\z') {
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

# ---- vlcrc edits for the lua intf companion -------------------------------------------
# Surgical first-match line edits over the raw text, written back as UTF-8 without BOM
# (VLC reads vlcrc as UTF-8; PowerShell 5's Set-Content default is ANSI and would
# corrupt non-ASCII values elsewhere in the file).

function Update-FirstLine([string]$text, [string]$pattern, [string]$replacement) {
    $rx = [regex]::new($pattern, [Text.RegularExpressions.RegexOptions]::Multiline)
    if (-not $rx.IsMatch($text)) { return $null }
    $rx.Replace($text, $replacement, 1)
}

function Enable-VlcIntfCompanion([string]$vlcrc) {
    if (-not (Test-Path -LiteralPath $vlcrc -PathType Leaf)) {
        # VLC runs fine without a vlcrc; a minimal one carrying just our keys is valid
        New-Item -ItemType Directory -Path (Split-Path $vlcrc -Parent) -Force | Out-Null
        [IO.File]::WriteAllText($vlcrc, "[lua]`nlua-intf=pip`n`n[core]`nextraintf=luaintf`n",
            [Text.UTF8Encoding]::new($false))
        return $true
    }
    $text = [IO.File]::ReadAllText($vlcrc)
    $nl = if ($text -match "`r`n") { "`r`n" } else { "`n" }

    $current = [regex]::Match($text, '(?m)^lua-intf=([^\r\n]*)')
    if ($current.Success -and $current.Groups[1].Value.Trim() -ne 'pip') {
        # never displace a lua interface the user configured themselves; the menu
        # toggle still adapts, only hotkey/playlist adaptation is skipped
        Write-Warning "vlcrc already sets lua-intf=$($current.Groups[1].Value.Trim()); skipping auto-adaptation setup"
        return $false
    }
    if (-not $current.Success) {
        $updated = Update-FirstLine $text '^#lua-intf=[^\r\n]*' 'lua-intf=pip'
        if ($null -eq $updated) { $updated = Update-FirstLine $text '^\[lua\][^\r\n]*' ('$0' + $nl + 'lua-intf=pip') }
        if ($null -eq $updated) { $updated = $text.TrimEnd() + $nl + $nl + '[lua]' + $nl + 'lua-intf=pip' + $nl }
        $text = $updated
    }

    $extra = [regex]::Match($text, '(?m)^extraintf=([^\r\n]*)')
    if ($extra.Success) {
        $modules = @($extra.Groups[1].Value.Trim() -split ':' | Where-Object { $_ })
        if ($modules -notcontains 'luaintf') {
            $value = (@($modules) + 'luaintf') -join ':'
            $text = Update-FirstLine $text '^extraintf=[^\r\n]*' ('extraintf=' + $value)
        }
    }
    else {
        $updated = Update-FirstLine $text '^#extraintf=[^\r\n]*' 'extraintf=luaintf'
        if ($null -eq $updated) { $updated = Update-FirstLine $text '^\[core\][^\r\n]*' ('$0' + $nl + 'extraintf=luaintf') }
        if ($null -eq $updated) { $updated = $text.TrimEnd() + $nl + $nl + '[core]' + $nl + 'extraintf=luaintf' + $nl }
        $text = $updated
    }
    [IO.File]::WriteAllText($vlcrc, $text, [Text.UTF8Encoding]::new($false))
    $true
}

function Disable-VlcIntfCompanion([string]$vlcrc) {
    if (-not (Test-Path -LiteralPath $vlcrc -PathType Leaf)) { return }
    $text = [IO.File]::ReadAllText($vlcrc)

    # lua-intf=pip is the ownership marker: only when it reverts do we also strip
    # luaintf from extraintf. A foreign lua-intf setup (enable declined) keeps both -
    # that luaintf serves the user's own interface, not ours. The lookahead keeps a
    # CRLF file's \r out of the replacement.
    $updated = Update-FirstLine $text '^lua-intf=pip[ \t]*(?=\r?$)' '#lua-intf=dummy'
    if ($null -eq $updated) { return }
    $text = $updated

    $extra = [regex]::Match($text, '(?m)^extraintf=([^\r\n]*)')
    if ($extra.Success) {
        $modules = @($extra.Groups[1].Value.Trim() -split ':' | Where-Object { $_ })
        if ($modules -contains 'luaintf') {
            $kept = @($modules | Where-Object { $_ -ne 'luaintf' })
            $line = if ($kept.Count) { 'extraintf=' + ($kept -join ':') } else { '#extraintf=' }
            $text = Update-FirstLine $text '^extraintf=[^\r\n]*' $line
        }
    }
    [IO.File]::WriteAllText($vlcrc, $text, [Text.UTF8Encoding]::new($false))
}

function Start-InstalledDaemon([string]$executable, [string]$alivePath) {
    $startedAt = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $daemon = $null
    try {
        $daemon = Start-Process -FilePath $executable -ArgumentList "daemon" -PassThru -WindowStyle Hidden
        $deadline = (Get-Date).AddSeconds(5)
        $verified = $false
        do {
            try {
                $daemon.Refresh()
                if ($daemon.HasExited) { break }
                if (Test-Path -LiteralPath $alivePath -PathType Leaf) {
                    $heartbeat = [IO.File]::ReadAllText($alivePath)
                    $verified = (Test-InstalledHelperProcess $daemon $executable) -and
                        (Test-DaemonHeartbeat $heartbeat ([uint32]$daemon.Id) $startedAt)
                }
            } catch [IO.IOException] { }
            if (-not $verified) { Start-Sleep -Milliseconds 100 }
        } while (-not $verified -and (Get-Date) -lt $deadline)

        if (-not $verified) { throw "daemon startup could not be verified" }
    } catch {
        if ($null -ne $daemon) {
            Stop-StartedProcess $daemon
            Remove-OrphanedHeartbeat $alivePath
        }
        throw
    }
}
