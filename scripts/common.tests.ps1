$ErrorActionPreference = "Stop"
. "$PSScriptRoot\common.ps1"

function Assert-True($value, $message) {
    if (-not $value) { throw $message }
}

function Assert-Throws([scriptblock]$action, $message) {
    $threw = $false
    try { & $action | Out-Null } catch { $threw = $true }
    if (-not $threw) { throw $message }
}

$state = Join-Path ([IO.Path]::GetTempPath()) "vlc-pip-script-test-$PID.state"
$package = Join-Path ([IO.Path]::GetTempPath()) "vlc-pip-package-test-$PID"
$heartbeat = Join-Path ([IO.Path]::GetTempPath()) "vlc-pip-heartbeat-test-$PID.alive"
try {
    [IO.File]::WriteAllText($state, "1 2 3 4 5 6 7 8 9 br 10 1 42`n")
    Assert-True ((Read-PipStatePid $state) -eq 42) "valid state PID was not read"

    foreach ($bad in @(
        "1 2 3 4 5 6 7 8 9 br 10 1 42",
        "1 2 3 4 5 6 7 8 9 br 10 42`n",
        "1 2 3 4 5 6 7 8 9 br 10 1 42 extra`n",
        "1 2 3 4 5 6 7 8 9 br 10 1 nope`n",
        "1 2 3 4 5 6 7 8 9 br 10 1 4294967296`n"
    )) {
        [IO.File]::WriteAllText($state, $bad)
        Assert-Throws { Read-PipStatePid $state } "corrupt state was accepted: $bad"
    }

    Assert-True (Test-SamePath "C:\Temp\PIP-HELPER.exe" "c:\temp\pip-helper.exe") "path comparison must ignore case"
    Assert-True (-not (Test-SamePath "C:\Temp\one.exe" "C:\Temp\two.exe")) "different paths matched"

    $matching = [pscustomobject]@{ Id = 1; Path = "C:\Temp\PIP-HELPER.exe" }
    $foreign = [pscustomobject]@{ Id = 2; Path = "C:\Elsewhere\pip-helper.exe" }
    $unreadable = [pscustomobject]@{ Id = 3 }
    Add-Member -InputObject $unreadable -MemberType ScriptProperty -Name Path -Value { throw "denied" }
    $script:fakeProcesses = @($matching, $foreign, $unreadable)
    function Get-Process { param($Name, $Id, $ErrorAction); $script:fakeProcesses }
    try {
        $selected = @(Get-InstalledHelperProcess "c:\temp\pip-helper.exe")
        Assert-True ($selected.Count -eq 1 -and $selected[0].Id -eq 1) "exact-path process filter selected the wrong processes"
    } finally {
        Remove-Item Function:\Get-Process
    }

    $script:fakeKilled = $false
    $launched = [pscustomobject]@{}
    Add-Member -InputObject $launched -MemberType ScriptProperty -Name HasExited -Value { $script:fakeKilled }
    Add-Member -InputObject $launched -MemberType ScriptProperty -Name Path -Value { throw "denied" }
    Add-Member -InputObject $launched -MemberType ScriptMethod -Name Kill -Value { $script:fakeKilled = $true }
    Add-Member -InputObject $launched -MemberType ScriptMethod -Name WaitForExit -Value { param($milliseconds); $script:fakeKilled }
    Stop-StartedProcess $launched
    Assert-True $script:fakeKilled "directly launched process was not stopped when Path was unreadable"

    [IO.File]::WriteAllText($heartbeat, "torn")
    $script:fakeProcesses = @()
    function Get-Process { param($Name, $Id, $ErrorAction); $script:fakeProcesses }
    try {
        Remove-OrphanedHeartbeat $heartbeat
        Assert-True (-not (Test-Path -LiteralPath $heartbeat)) "orphaned malformed heartbeat survived"
        [IO.File]::WriteAllText($heartbeat, "torn")
        $script:fakeProcesses = @($foreign)
        Remove-OrphanedHeartbeat $heartbeat
        Assert-True (Test-Path -LiteralPath $heartbeat) "heartbeat with a live pip-helper owner was removed"
    } finally {
        Remove-Item Function:\Get-Process
    }

    $line = "100 pid=42 hotkey=0 timer=1 kb=0 mouse=1"
    Assert-True (Test-DaemonHeartbeat $line 42 100 105) "valid heartbeat was rejected"
    Assert-True (-not (Test-DaemonHeartbeat $line 41 100 105)) "wrong heartbeat PID was accepted"
    Assert-True (-not (Test-DaemonHeartbeat $line 42 101 105)) "pre-launch heartbeat was accepted"
    Assert-True (-not (Test-DaemonHeartbeat $line 42 100 116)) "stale heartbeat was accepted"
    Assert-True (-not (Test-DaemonHeartbeat ($line -replace "kb=0", "kb=2") 42 100 105)) "invalid flag was accepted"
    Assert-True (-not (Test-DaemonHeartbeat "$line`n" 42 100 105)) "non-exact heartbeat was accepted"

    $script:fakeDaemon = [pscustomobject]@{ Id = $PID; Path = "C:\Temp\pip-helper.exe"; HasExited = $false }
    Add-Member -InputObject $script:fakeDaemon -MemberType ScriptMethod -Name Refresh -Value { }
    function Start-Process {
        param($FilePath, $ArgumentList, [switch]$PassThru, $WindowStyle)
        $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
        [IO.File]::WriteAllText($heartbeat, "$now pid=$PID hotkey=1 timer=1 kb=0 mouse=0")
        $script:fakeDaemon
    }
    try {
        Start-InstalledDaemon "C:\Temp\pip-helper.exe" $heartbeat
    } finally {
        Remove-Item Function:\Start-Process
    }

    New-Item -ItemType Directory -Path "$package\helper" | Out-Null
    [IO.File]::WriteAllText("$package\helper\Cargo.toml", "[package]")
    [IO.File]::WriteAllText("$package\pip-helper.exe", "fake")
    Assert-True ($null -eq (Get-PrebuiltHelper $package)) "root exe shadowed a source checkout"
    Remove-Item -LiteralPath "$package\helper\Cargo.toml"
    Assert-True (Test-SamePath (Get-PrebuiltHelper $package) "$package\pip-helper.exe") "prebuilt exe was not selected"
    Remove-Item -LiteralPath "$package\pip-helper.exe"
    Assert-Throws { Get-PrebuiltHelper $package } "incomplete prebuilt package was accepted"

    [IO.File]::WriteAllText($state, "1 2 3 4 5 6 7 8 9 br 10 1 0`n")
    $oldHelper = "$package\installed-helper.exe"
    [IO.File]::WriteAllText($oldHelper, "fake")
    Assert-PipStatePrerequisites $state $oldHelper
    Remove-Item -LiteralPath $oldHelper
    Assert-Throws { Assert-PipStatePrerequisites $state $oldHelper } "state without its installed helper was accepted"

    [IO.File]::WriteAllText($oldHelper, "fake")
    $script:restoreExitCode = 0
    $script:removeStateOnRestore = $true
    function Start-Process {
        param($FilePath, $ArgumentList, [switch]$PassThru, [switch]$Wait, $WindowStyle)
        if ($ArgumentList -ne "restore") { throw "maintenance used the wrong helper mode: $ArgumentList" }
        if ($script:removeStateOnRestore) { Remove-Item -LiteralPath $state }
        [pscustomobject]@{ ExitCode = $script:restoreExitCode }
    }
    try {
        [IO.File]::WriteAllText($state, "1 2 3 4 5 6 7 8 9 br 10 1 $PID`n")
        Resolve-PipState $state $oldHelper
        Assert-True (-not (Test-Path -LiteralPath $state)) "successful restore did not consume state"

        $script:restoreExitCode = 4
        $script:removeStateOnRestore = $false
        [IO.File]::WriteAllText($state, "1 2 3 4 5 6 7 8 9 br 10 1 $PID`n")
        Resolve-PipState $state $oldHelper
        Assert-True (Test-Path -LiteralPath $state) "installer deleted a pending-heal state"
        Assert-Throws { Resolve-PipState $state $oldHelper -RequireRestore } "uninstall accepted a pending-heal state"

        $script:restoreExitCode = 2
        Assert-Throws { Resolve-PipState $state $oldHelper } "unknown helper mode was accepted as pending heal"

        $script:restoreExitCode = 1
        $script:removeStateOnRestore = $true
        [IO.File]::WriteAllText($state, "1 2 3 4 5 6 7 8 9 br 10 1 $PID`n")
        Assert-Throws { Resolve-PipState $state $oldHelper } "failed restore was accepted because state disappeared"
    } finally {
        Remove-Item Function:\Start-Process
    }
} finally {
    Remove-Item -LiteralPath $state -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $package -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $heartbeat -Force -ErrorAction SilentlyContinue
}

Write-Host "Script contracts PASS"
