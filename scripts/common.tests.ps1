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
    Assert-True (@(Get-InstalledHelperProcess "$state-never-an-exe").Count -eq 0) "process name alone matched"

    $line = "100 pid=42 hotkey=0 timer=1 kb=0 mouse=1"
    Assert-True (Test-DaemonHeartbeat $line 42 100 105) "valid heartbeat was rejected"
    Assert-True (-not (Test-DaemonHeartbeat $line 41 100 105)) "wrong heartbeat PID was accepted"
    Assert-True (-not (Test-DaemonHeartbeat $line 42 101 105)) "pre-launch heartbeat was accepted"
    Assert-True (-not (Test-DaemonHeartbeat $line 42 100 116)) "stale heartbeat was accepted"
    Assert-True (-not (Test-DaemonHeartbeat ($line -replace "kb=0", "kb=2") 42 100 105)) "invalid flag was accepted"
    Assert-True (-not (Test-DaemonHeartbeat "$line`n" 42 100 105)) "non-exact heartbeat was accepted"

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
    Resolve-PipState $state $oldHelper
    Assert-True (Test-Path -LiteralPath $state) "installer path deleted a pending-heal state"
    Assert-Throws { Resolve-PipState $state $oldHelper -RequireRestore } "uninstall accepted a pending-heal state"
    Remove-Item -LiteralPath $oldHelper
    Assert-Throws { Resolve-PipState $state $oldHelper } "state without its installed helper was accepted"

    [IO.File]::WriteAllText($oldHelper, "fake")
    $script:removeStateOnExit = $true
    function Start-Process {
        param($FilePath, $ArgumentList, [switch]$PassThru, [switch]$Wait, $WindowStyle)
        if ($script:removeStateOnExit) { Remove-Item -LiteralPath $state }
        [pscustomobject]@{ ExitCode = 9 }
    }
    try {
        [IO.File]::WriteAllText($state, "1 2 3 4 5 6 7 8 9 br 10 1 $PID`n")
        Resolve-PipState $state $oldHelper
        Assert-True (-not (Test-Path -LiteralPath $state)) "nonzero exit blocked a completed restore"

        $script:removeStateOnExit = $false
        [IO.File]::WriteAllText($state, "1 2 3 4 5 6 7 8 9 br 10 1 $PID`n")
        Assert-Throws { Resolve-PipState $state $oldHelper } "live state survived exit without aborting"
    } finally {
        Remove-Item Function:\Start-Process
    }
} finally {
    Remove-Item -LiteralPath $state -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $package -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "Script contracts PASS"
