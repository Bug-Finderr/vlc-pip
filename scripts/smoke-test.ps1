# Run after install.ps1 with no VLC instance open.
$ErrorActionPreference = "Stop"
. "$PSScriptRoot\common.ps1"

$exe = "$env:APPDATA\vlc\pip\pip-helper.exe"
$heartbeatPath = "$env:TEMP\vlc-pip-daemon.alive"
$statePath = "$env:TEMP\vlc-pip.state"
$luaPath = "$env:APPDATA\vlc\lua\extensions\pip.lua"
$startupLink = Join-Path ([Environment]::GetFolderPath("Startup")) "VLC PiP Daemon.lnk"

# WinExe stdout is invisible to PowerShell capture; run the helper and read its
# status-file channel instead of capturing output.
function Status {
    $path = "$env:TEMP\vlc-pip-status.json"
    if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
    $process = Start-Process $exe status -PassThru -Wait
    if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "helper status failed"
    }
    Get-Content $path -Raw | ConvertFrom-Json
}
function Req($cmd) {
    Set-Content -LiteralPath "$env:TEMP\vlc-pip-request.txt" -Value $cmd -NoNewline
}
function WaitFor([scriptblock]$cond, [int]$capMs = 3000, [int]$stepMs = 60) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $capMs) {
        if (& $cond) { return $true }
        Start-Sleep -Milliseconds $stepMs
    }
    & $cond
}
function WaitForStatus([scriptblock]$cond, [int]$capMs = 3000, [int]$stepMs = 60) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $capMs) {
        $status = Status
        if (& $cond $status) { return [pscustomobject]@{ Matched = $true; Status = $status } }
        Start-Sleep -Milliseconds $stepMs
    }
    $status = Status
    [pscustomobject]@{ Matched = [bool](& $cond $status); Status = $status }
}
function SameRect($left, $right) {
    $left.x -eq $right.x -and $left.y -eq $right.y -and
        $left.w -eq $right.w -and $left.h -eq $right.h
}
function IsFullscreenRestore($status, $baseline) {
    $status.found -and (-not $status.caption) -and (-not $status.inPip) -and
        (-not $status.minimal) -and $status.topmost -eq $baseline.topmost -and
        (-not (Test-Path "$env:TEMP\vlc-pip.state")) -and (SameRect $status $baseline)
}
function WaitForStableStatus([scriptblock]$cond, [int]$capMs = 3000, [int]$stepMs = 150) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $previous = $null
    while ($sw.ElapsedMilliseconds -lt $capMs) {
        $status = Status
        if (& $cond $status) {
            if ($previous -and (SameRect $status $previous)) {
                return [pscustomobject]@{ Matched = $true; Status = $status }
            }
            $previous = $status
        }
        else { $previous = $null }
        Start-Sleep -Milliseconds $stepMs
    }
    $status = Status
    [pscustomobject]@{
        Matched = [bool]((& $cond $status) -and $previous -and (SameRect $status $previous))
        Status = $status
    }
}
function ReadHeartbeat {
    try {
        $raw = (Get-Content $heartbeatPath -Raw).Trim()
        if ($raw -notmatch '^(?<time>\d+) pid=(?<pid>\d+) hotkey=(?<hotkey>[01]) timer=(?<timer>[01]) kb=(?<kb>[01]) mouse=(?<mouse>[01])$') {
            return $null
        }
        [pscustomobject]@{
            Time = [int64]$matches.time
            Pid = [uint32]$matches.pid
            Hotkey = [int]$matches.hotkey
            Timer = [int]$matches.timer
            Keyboard = [int]$matches.kb
            Mouse = [int]$matches.mouse
        }
    }
    catch { $null }
}
function VerifiedHeartbeat([long]$notBefore = 0) {
    $heartbeat = ReadHeartbeat
    if (-not $heartbeat -or $heartbeat.Time -lt $notBefore -or
        [math]::Abs([DateTimeOffset]::UtcNow.ToUnixTimeSeconds() - $heartbeat.Time) -ge 15) { return }
    $process = Get-Process -Id ([int]$heartbeat.Pid) -ErrorAction SilentlyContinue
    if ($process -and (Test-InstalledHelperProcess $process $exe)) { $heartbeat }
}
function WaitForVerifiedHeartbeat(
    [long]$notBefore = 0,
    [uint32]$differentPid = 0,
    [int]$capMs = 3000
) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        $heartbeat = VerifiedHeartbeat $notBefore
        if ($heartbeat -and ($differentPid -eq 0 -or $heartbeat.Pid -ne $differentPid)) { return $heartbeat }
        Start-Sleep -Milliseconds 50
    } while ($sw.ElapsedMilliseconds -lt $capMs)
}
$fail = @()
function Check($name, $cond) {
    if ($cond) { Write-Host "PASS $name" }
    else {
        Write-Host "FAIL $name"
        try { Write-Host "  status: $(Get-Content "$env:TEMP\vlc-pip-status.json" -Raw)" } catch {}
        $script:fail += $name
    }
}

if (-not ('Smoke.Keys' -as [type])) {
    Add-Type -Namespace Smoke -Name Keys -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
[DllImport("user32.dll")] public static extern uint GetDoubleClickTime();
[DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
[DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr top, EnumProc cb, IntPtr l);
[DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder sb, int max);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
[DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
[DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr h, int index);
[DllImport("user32.dll")] public static extern int GetWindowRgn(IntPtr h, IntPtr rgn);
[DllImport("gdi32.dll")] public static extern int GetRgnBox(IntPtr rgn, out RECT box);
[DllImport("gdi32.dll")] public static extern IntPtr CreateRectRgn(int left, int top, int right, int bottom);
[DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr obj);
public delegate bool EnumProc(IntPtr h, IntPtr l);
private const int NULLREGION = 1;
private const int WS_CAPTION = 0x00C00000;
private const int WS_EX_TOPMOST = 0x00000008;
public struct RECT { public int Left, Top, Right, Bottom; }
public struct POINT { public int X, Y; }
public sealed class ScreenPoint {
    public bool Found;
    public int X;
    public int Y;
}
public sealed class ScreenRect {
    public bool Found;
    public int Left;
    public int Top;
    public int Right;
    public int Bottom;
}
public sealed class ControllerState {
    public int Found;
    public int Veiled;
    public int Rendered;
}
public static ScreenPoint CursorPosition() {
    POINT point;
    bool found = GetCursorPos(out point);
    return new ScreenPoint { Found = found, X = point.X, Y = point.Y };
}
public static bool CursorNear(int x, int y, int tolerance) {
    POINT point;
    return GetCursorPos(out point)
        && Math.Abs(point.X - x) <= tolerance
        && Math.Abs(point.Y - y) <= tolerance;
}
public static ScreenRect VisibleRect(IntPtr h, int x, int y) {
    IntPtr probe = CreateRectRgn(0, 0, 0, 0);
    if (probe == IntPtr.Zero) throw new InvalidOperationException("CreateRectRgn failed");
    try {
        RECT box = new RECT();
        bool found = GetWindowRgn(h, probe) > NULLREGION
            && GetRgnBox(probe, out box) > NULLREGION;
        return new ScreenRect {
            Found = found,
            Left = x + box.Left,
            Top = y + box.Top,
            Right = x + box.Right,
            Bottom = y + box.Bottom
        };
    }
    finally { DeleteObject(probe); }
}
public static bool PipLanded(IntPtr h, int maxWidth) {
    var rect = VisibleRect(h, 0, 0);
    return rect.Found && rect.Right - rect.Left < maxWidth
        && (GetWindowLongW(h, -16) & WS_CAPTION) != WS_CAPTION
        && (GetWindowLongW(h, -20) & WS_EX_TOPMOST) != 0;
}
// VLC's vout event thread processes POSTED keys regardless of focus - keybd_event would
// need VLC foreground, and injected focus-clicks from a background session are flaky
public static IntPtr VoutChild(IntPtr top) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(top, (h, l) => {
        var sb = new System.Text.StringBuilder(128);
        GetClassNameW(h, sb, 128);
        if (sb.ToString().StartsWith("VLC video main")) { found = h; return false; }
        return true;
    }, IntPtr.Zero);
    return found;
}
// Snapshot every same-process fullscreen controller. Hidden controllers must also carry
// the veil because VLC can show them between daemon ticks.
public static ControllerState FscState(IntPtr vlcTop) {
    uint pid; GetWindowThreadProcessId(vlcTop, out pid);
    var state = new ControllerState();
    IntPtr probe = CreateRectRgn(0, 0, 0, 0);
    if (probe == IntPtr.Zero) throw new InvalidOperationException("CreateRectRgn failed");
    try {
        EnumWindows((h, l) => {
            var sb = new System.Text.StringBuilder(128);
            GetClassNameW(h, sb, 128);
            if (sb.ToString().StartsWith("Qt5QWindowToolSaveBits")) {
                uint p; GetWindowThreadProcessId(h, out p);
                if (p == pid) {
                    state.Found++;
                    int windowType = GetWindowRgn(h, probe);
                    RECT box;
                    int boxType = GetRgnBox(probe, out box);
                    bool veiled = windowType == NULLREGION && boxType == NULLREGION;
                    if (veiled) state.Veiled++;
                    if (IsWindowVisible(h) && !veiled) state.Rendered++;
                }
            }
            return true;
        }, IntPtr.Zero);
    }
    finally {
        DeleteObject(probe); // GetWindowRgn copied into our probe; we still own it
    }
    return state;
}
'@
}
function VisibleRect($status) {
    [Smoke.Keys]::VisibleRect([IntPtr]::new([long]$status.hwnd), $status.x, $status.y)
}
function SameVisibleRect($left, $right) {
    $left.Left -eq $right.Left -and $left.Top -eq $right.Top -and
        $left.Right -eq $right.Right -and $left.Bottom -eq $right.Bottom
}
function WaitForStableVisibleRect([scriptblock]$cond, [int]$capMs = 3000, [int]$stepMs = 150) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $previous = $null
    $stableTicks = 0
    while ($sw.ElapsedMilliseconds -lt $capMs) {
        $status = Status
        $rect = VisibleRect $status
        if ($rect.Found -and (& $cond $status)) {
            if ($previous -and (SameVisibleRect $rect $previous)) {
                $stableTicks++
                if ($stableTicks -ge 3) { return [pscustomobject]@{ Status = $status; Rect = $rect } }
            }
            else { $stableTicks = 0 }
            $previous = $rect
        }
        else { $previous = $null; $stableTicks = 0 }
        Start-Sleep -Milliseconds $stepMs
    }
    throw "visible PiP region did not stabilize"
}
function ClickAt($x, $y, $times) {
    $doubleClickMs = [int][Smoke.Keys]::GetDoubleClickTime()
    $burstGapMs = [math]::Max(1, [int]($doubleClickMs / ($times + 1)))
    [Smoke.Keys]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 100
    for ($i = 0; $i -lt $times; $i++) {
        [Smoke.Keys]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)  # LEFTDOWN
        [Smoke.Keys]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)  # LEFTUP
        if ($i + 1 -lt $times) { Start-Sleep -Milliseconds $burstGapMs }
    }
    # the NEXT injected button-down must fall outside double-click time of this burst's
    # last ALLOWED down, or the guard swallows it (and a drag would never arm)
    Start-Sleep -Milliseconds ($doubleClickMs + 100)
}
# The scan code matters to VLC's key translation.
function PostKey([long]$top, [int]$vk, [int]$scan) {
    $t = [Smoke.Keys]::VoutChild([IntPtr]::new($top))
    if ($t -eq [IntPtr]::Zero) { $t = [IntPtr]::new($top) }
    [Smoke.Keys]::PostMessageW($t, 0x100, [IntPtr]::new($vk), [IntPtr]::new(0x00000001 -bor ($scan -shl 16))) | Out-Null
    [Smoke.Keys]::PostMessageW($t, 0x101, [IntPtr]::new($vk), [IntPtr]::new(0xC0000001 -bor ($scan -shl 16))) | Out-Null
}
function SendCtrlAltP {
    [Smoke.Keys]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)      # Ctrl down
    [Smoke.Keys]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)      # Alt down
    [Smoke.Keys]::keybd_event(0x50, 0, 0, [UIntPtr]::Zero)      # P down
    [Smoke.Keys]::keybd_event(0x50, 0, 2, [UIntPtr]::Zero)      # P up
    [Smoke.Keys]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)      # Alt up
    [Smoke.Keys]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero)      # Ctrl up
}
# MOVE|ABSOLUTE|VIRTUALDESK, normalized over the entire virtual desktop so negative and
# secondary-monitor coordinates reach the low-level mouse hook unchanged.
function MoveAbs([int]$x, [int]$y) {
    $vx = [Smoke.Keys]::GetSystemMetrics(76); $vy = [Smoke.Keys]::GetSystemMetrics(77)
    $vw = [Smoke.Keys]::GetSystemMetrics(78); $vh = [Smoke.Keys]::GetSystemMetrics(79)
    if ($vw -le 1 -or $vh -le 1) { throw "invalid virtual desktop ${vw}x${vh}" }
    $nx = [math]::Max(0, [math]::Min(65535, [math]::Round(($x - $vx) * 65535 / ($vw - 1))))
    $ny = [math]::Max(0, [math]::Min(65535, [math]::Round(($y - $vy) * 65535 / ($vh - 1))))
    [Smoke.Keys]::mouse_event(0xC001, [uint32]$nx, [uint32]$ny, 0, [UIntPtr]::Zero)
}
function VirtualDesktop {
    [pscustomobject]@{
        X = [Smoke.Keys]::GetSystemMetrics(76)
        Y = [Smoke.Keys]::GetSystemMetrics(77)
        Width = [Smoke.Keys]::GetSystemMetrics(78)
        Height = [Smoke.Keys]::GetSystemMetrics(79)
        Monitors = [Smoke.Keys]::GetSystemMetrics(80)
    }
}
function ProbeVirtualDesktop($desktop) {
    if ($desktop.Monitors -le 1) {
        Write-Host "SKIP multi-monitor absolute input (one monitor)"
        return
    }
    Add-Type -AssemblyName System.Windows.Forms
    $screen = [System.Windows.Forms.Screen]::AllScreens | Where-Object { -not $_.Primary } | Select-Object -First 1
    $prior = [Smoke.Keys]::CursorPosition()
    $landed = $false
    $restored = $false
    if ($screen -and $prior.Found) {
        $targetX = $screen.Bounds.Left + [int]($screen.Bounds.Width / 2)
        $targetY = $screen.Bounds.Top + [int]($screen.Bounds.Height / 2)
        try {
            MoveAbs $targetX $targetY
            $landed = WaitFor { [Smoke.Keys]::CursorNear($targetX, $targetY, 2) } 750 15
        }
        finally {
            $restored = [Smoke.Keys]::SetCursorPos($prior.X, $prior.Y)
        }
    }
    Check "multi-monitor: absolute input reaches nonprimary monitor and cursor restores" `
        ($screen -and $prior.Found -and $landed -and $restored)
}
function AllControllersVeiled([long]$top) {
    $state = [Smoke.Keys]::FscState([IntPtr]::new($top))
    $state.Found -gt 0 -and $state.Veiled -eq $state.Found
}
function DragFrom($x1, $y1, $x2, $y2) {
    # movement must go through injected mouse_event MOVEs: SetCursorPos repositions the
    # cursor without generating input events, so WH_MOUSE_LL (the daemon) never sees it
    [Smoke.Keys]::SetCursorPos($x1, $y1) | Out-Null
    Start-Sleep -Milliseconds 150
    [Smoke.Keys]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
    Start-Sleep -Milliseconds 80
    for ($i = 1; $i -le 10; $i++) {
        $px = $x1 + [int](($x2 - $x1) * $i / 10); $py = $y1 + [int](($y2 - $y1) * $i / 10)
        MoveAbs $px $py
        Start-Sleep -Milliseconds 25
    }
    [Smoke.Keys]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
}
function HoverWiggle($cx, $cy) {
    for ($i = 0; $i -lt 8; $i++) {
        $px = $cx - 40 + $i * 10; $py = $cy - 15 + ($i % 3) * 10
        MoveAbs $px $py
        Start-Sleep -Milliseconds 60
    }
}

if (-not (Test-Path $exe)) { throw "installed helper not found: $exe" }
$script:heartbeat = $null
$heartbeatReady = WaitFor {
    $candidate = ReadHeartbeat
    if ($candidate -and [math]::Abs([DateTimeOffset]::UtcNow.ToUnixTimeSeconds() - $candidate.Time) -lt 15) {
        $script:heartbeat = $candidate
        return $true
    }
    $false
} 1200 50
$heartbeat = $script:heartbeat
Check "daemon heartbeat: valid and fresh" $heartbeatReady
$daemon = if ($heartbeat) { Get-Process -Id ([int]$heartbeat.Pid) -ErrorAction SilentlyContinue } else { $null }
$daemonIdentity = $daemon -and (Test-InstalledHelperProcess $daemon $exe)
Check "daemon heartbeat: pid owns installed helper" `
    $daemonIdentity
# Idle session hooks may be absent; the global hotkey and timer are mandatory daemon arms.
$daemonArmed = $heartbeat -and $heartbeat.Hotkey -eq 1 -and $heartbeat.Timer -eq 1
Check "daemon heartbeat: hotkey/timer armed" `
    $daemonArmed
if (-not ($heartbeatReady -and $daemonIdentity -and $daemonArmed)) {
    throw "daemon heartbeat preflight failed"
}
$desktop = VirtualDesktop
Check "virtual desktop: coordinate bounds available" `
    ($desktop.Width -gt 1 -and $desktop.Height -gt 1 -and $desktop.Monitors -ge 1)
ProbeVirtualDesktop $desktop

$vlcDir = (Get-ItemProperty 'HKLM:\SOFTWARE\VideoLAN\VLC' -ErrorAction SilentlyContinue).InstallDir
$vlcPath = if ($vlcDir) { Join-Path $vlcDir 'vlc.exe' } else { 'C:\Program Files\VideoLAN\VLC\vlc.exe' }
if (-not (Test-Path $vlcPath)) { throw "vlc.exe not found" }
if (Get-Process vlc -ErrorAction SilentlyContinue) {
    throw "Close VLC first: this test resizes, clicks, and kills the VLC instance it targets"
}
if (Test-Path -LiteralPath $statePath) {
    throw "Resolve the pending PiP restore before running this test"
}
$cfg = "$env:APPDATA\vlc\pip\config.txt"
$cfgBak = Join-Path ([IO.Path]::GetTempPath()) "vlc-pip-config-smoke-$PID-$([guid]::NewGuid().ToString('N')).bak"
$hadConfig = Test-Path -LiteralPath $cfg
if ($hadConfig -and -not (Test-Path -LiteralPath $cfg -PathType Leaf)) {
    throw "PiP config path is not a file: $cfg"
}
$vlcProc = $null
$preserveHealAfterStop = $false

try {
    # Park inside the protected block so every later throw restores the user's config.
    if ($hadConfig) { Move-Item $cfg $cfgBak -Force }

    # screen:// = live playing video, so the video child window and minimal-look region exist
    $vlcProc = Start-Process $vlcPath 'screen://' -PassThru
    $startup = WaitForStableStatus { param($status) $status.found -and $status.caption } 6000 300
    $before = $startup.Status
    Check "vlc ready (windowed, caption)" ($startup.Matched -and $before.found -and $before.caption)

    Req "toggle"
    $null = WaitFor { (Status).minimal } 4000 150
    $pip = Status
    $visible = VisibleRect $pip
    Check "enter: pip formed (borderless, topmost, video 480w, region, state)" `
        ((-not $pip.caption) -and $pip.topmost -and $pip.inPip -and $pip.minimal -and
            $visible.Found -and $visible.Right - $visible.Left -eq 480 -and
            $visible.Bottom - $visible.Top -eq 270)

    # Keep separate terminal assertions: a broken five-click guard can fullscreen twice
    # and land back at the starting rect, hiding the regression in its final state.
    if (-not $visible.Found) { throw "PiP visible region was not measurable" }
    $cx = [int](($visible.Left + $visible.Right) / 2)
    $cy = [int](($visible.Top + $visible.Bottom) / 2)
    foreach ($burst in 2, 3, 5) {
        ClickAt $cx $cy $burst
        $afterBurst = Status
        Check "click burst ($burst): rect unchanged, still pip" `
            ((SameRect $afterBurst $pip) -and $afterBurst.inPip)
    }

    # A cross-process transition may stall in a synchronous Win32 call. The daemon
    # must keep pumping LL-hook callbacks while it waits for the transition lock.
    $transitionBlocker = [Threading.Mutex]::new($false, "VlcPipTransition")
    $transitionBlocked = $false
    try {
        $transitionBlocked = $transitionBlocker.WaitOne(1000)
        if (-not $transitionBlocked) { throw "transition mutex could not be acquired" }
        Start-Sleep -Milliseconds 300
        $heartbeatBeforeContention = WaitForVerifiedHeartbeat
        if (-not $heartbeatBeforeContention) { throw "contention heartbeat baseline missing" }
        $heartbeatSurvivedContention = WaitFor {
            $candidate = VerifiedHeartbeat ($heartbeatBeforeContention.Time + 1)
            [bool]$candidate
        } 5000 50
        ClickAt $cx $cy 2
        SendCtrlAltP
        Start-Sleep -Milliseconds 100 # force at least one timed-out dequeue + repost
        $duringContention = Status
    }
    finally {
        if ($transitionBlocked) { $transitionBlocker.ReleaseMutex() }
        $transitionBlocker.Dispose()
    }
    $hooksSurvivedContention = (SameRect $duringContention $pip) -and $duringContention.inPip
    $delayedHotkey = WaitForStableStatus {
        param($status)
        (-not $status.inPip) -and $status.caption -and (SameRect $status $before)
    } 3000 150
    $hotkeyAppliedAfterUnlock = $delayedHotkey.Matched
    Check "transition contention: heartbeat advances" $heartbeatSurvivedContention
    Check "transition contention: input guards stay armed" $hooksSurvivedContention
    Check "transition contention: hotkey applies after unlock" $hotkeyAppliedAfterUnlock
    if (-not ($heartbeatSurvivedContention -and $hooksSurvivedContention -and $hotkeyAppliedAfterUnlock)) {
        throw "daemon stalled during transition contention"
    }

    Req "enter"
    $reentered = WaitForStableStatus { param($status) $status.inPip -and $status.minimal } 3000 150
    if (-not $reentered.Matched) { throw "contention recovery could not re-enter PiP" }
    $pip = $reentered.Status
    $visible = VisibleRect $pip
    if (-not $visible.Found) { throw "re-entered PiP visible region was not measurable" }
    $cx = [int](($visible.Left + $visible.Right) / 2)
    $cy = [int](($visible.Top + $visible.Bottom) / 2)

    DragFrom $cx $cy ($cx - 80) ($cy - 60)
    $movedResult = WaitForStatus {
        param($status)
        [math]::Abs($status.x - ($pip.x - 80)) -le 2 -and
            [math]::Abs($status.y - ($pip.y - 60)) -le 2 -and
            $status.w -eq $pip.w -and $status.h -eq $pip.h -and $status.inPip
    } 3000 60
    $moved = $movedResult.Status
    $movedVisible = VisibleRect $moved
    if (-not $movedVisible.Found) { throw "moved PiP visible region was not measurable" }
    Check "drag-move: at delta, size held, still pip" `
        $movedResult.Matched
    $visibleWidth = $movedVisible.Right - $movedVisible.Left
    $visibleHeight = $movedVisible.Bottom - $movedVisible.Top
    $configReady = WaitFor {
        try { (Get-Content -LiteralPath $cfg -Raw -ErrorAction Stop).Trim() -eq "w=$visibleWidth h=$visibleHeight c=br" }
        catch { $false }
    } 2000 50
    Check "drag-move: config persisted (w/h + corner)" `
        $configReady

    $edgeX = $movedVisible.Right - 8
    $edgeY = [int](($movedVisible.Top + $movedVisible.Bottom) / 2)
    ClickAt $edgeX $edgeY 1
    $bandClick = Status
    Check "band click: no resize, no move" ((SameRect $bandClick $moved) -and $bandClick.inPip)

    DragFrom $edgeX $edgeY ($edgeX - 100) $edgeY
    $resizeStable = WaitForStableVisibleRect { param($status) $status.w -lt $moved.w -and $status.minimal } 4000
    $rs = $resizeStable.Status
    $resizedVisible = $resizeStable.Rect
    Check "drag-resize (right edge): width shrank, minimal look held" `
        ($resizedVisible.Right - $resizedVisible.Left -lt $visibleWidth -and $rs.inPip -and $rs.minimal)

    [Smoke.Keys]::SetCursorPos(
        [int](($resizedVisible.Left + $resizedVisible.Right) / 2),
        [int](($resizedVisible.Top + $resizedVisible.Bottom) / 2)
    ) | Out-Null
    Start-Sleep -Milliseconds 100
    [Smoke.Keys]::mouse_event(0x0800, 0, 0, 120, [UIntPtr]::Zero)   # WHEEL up one notch
    Start-Sleep -Milliseconds 400
    $wheeled = Status
    $wheeledVisible = VisibleRect $wheeled
    Check "wheel: helper does not resize" `
        ($wheeledVisible.Found -and $wheeledVisible.Left -eq $resizedVisible.Left -and
            $wheeledVisible.Top -eq $resizedVisible.Top -and $wheeledVisible.Right -eq $resizedVisible.Right -and
            $wheeledVisible.Bottom -eq $resizedVisible.Bottom)

    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $after = Status
    Check "exit: exact windowed restore (caption, rect, topmost, region/state)" `
        ($after.caption -and $after.topmost -eq $before.topmost -and (-not $after.inPip) -and (-not $after.minimal) -and
            (-not (Test-Path "$env:TEMP\vlc-pip.state")) -and (SameRect $after $before))

    Req "toggle"
    $null = WaitFor { (Status).inPip } 3000 150
    $re = Status
    $reVisible = VisibleRect $re
    Check "persist: re-enter at gestured size" `
        ($reVisible.Found -and
            [math]::Abs(($reVisible.Right - $reVisible.Left) - ($resizedVisible.Right - $resizedVisible.Left)) -le 2 -and
            [math]::Abs(($reVisible.Bottom - $reVisible.Top) - ($resizedVisible.Bottom - $resizedVisible.Top)) -le 2)
    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150

    SendCtrlAltP
    Check "hotkey enters pip" (WaitFor { (Status).inPip } 3000 150)
    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $s = Status
    Check "hotkey then request: no desync, exact rect" `
        ((-not $s.inPip) -and $s.topmost -eq $before.topmost -and $s.x -eq $before.x -and $s.y -eq $before.y -and $s.w -eq $before.w -and $s.h -eq $before.h)

    # Sweep direct exit->enter across the daemon's tick phase. Without cross-process
    # serialization, a stale daemon action can restore the old frame after new state lands.
    $transitionClean = $true
    $direct = Start-Process $exe -ArgumentList 'enter min=0' -PassThru -Wait
    if ($direct.ExitCode -ne 0) { throw "transition stress setup failed" }
    foreach ($cycle in 0..49) {
        Start-Sleep -Milliseconds $cycle
        $direct = Start-Process $exe exit -PassThru -Wait
        if ($direct.ExitCode -ne 0) { throw "transition stress exit failed at cycle $cycle" }
        $direct = Start-Process $exe -ArgumentList 'enter min=0' -PassThru -Wait
        if ($direct.ExitCode -ne 0) { throw "transition stress enter failed at cycle $cycle" }
        $transition = Status
        if (-not $transition.inPip -or $transition.caption) { $transitionClean = $false; break }
    }
    $direct = Start-Process $exe exit -PassThru -Wait
    $transitionEnd = WaitForStableStatus {
        param($status)
        (-not $status.inPip) -and $status.caption -and (SameRect $status $before)
    } 3000 150
    Check "one-shot exit-enter: 50 serialized transitions" `
        ($transitionClean -and $direct.ExitCode -eq 0 -and $transitionEnd.Matched)
    if (-not ($transitionClean -and $direct.ExitCode -eq 0 -and $transitionEnd.Matched)) {
        throw "one-shot exit-enter serialization failed"
    }
    $s = $transitionEnd.Status

    PostKey $s.hwnd 0x46 0x21
    $fullscreen = WaitForStableStatus { param($status) $status.found -and (-not $status.caption) } 2500 150
    $fs = $fullscreen.Status
    Check "fullscreen: engaged at a stable rect" $fullscreen.Matched

    # Summon the strip first: the realistic toggle starts with it rendered. Use the
    # fullscreen rect center rather than the primary monitor center.
    $fcx = $fs.x + [int]($fs.w / 2); $fcy = $fs.y + [int]($fs.h / 2)
    HoverWiggle $fcx $fcy
    $stripUp = WaitFor { ([Smoke.Keys]::FscState([IntPtr]::new([long]$fs.hwnd))).Rendered -gt 0 } 2500 60
    Check "controller: rendered before fullscreen PiP enter" $stripUp

    # A status process can take about a second in fullscreen; time the window directly.
    $hsw = [System.Diagnostics.Stopwatch]::StartNew()
    Req "toggle"
    $landedFast = WaitFor {
        (Test-Path "$env:TEMP\vlc-pip.state") -and
            [Smoke.Keys]::PipLanded([IntPtr]::new([long]$fs.hwnd), [int]($fs.w / 2))
    } 500 5
    $enterMs = $hsw.ElapsedMilliseconds
    $landing = WaitForStatus {
        param($status)
        $status.inPip -and (-not $status.caption) -and $status.topmost -and $status.w -lt [int]($fs.w / 2)
    } 2500 60
    $fpip = $landing.Status
    Check "fullscreen enter: immediate (<500ms), pip-sized" `
        ($landedFast -and $enterMs -lt 500 -and $landing.Matched -and $fpip.inPip -and
            (-not $fpip.caption) -and $fpip.topmost -and $fpip.w -lt [int]($fs.w / 2))
    Check "controller: every matching window veiled in fullscreen PiP" `
        (WaitFor { AllControllersVeiled $fs.hwnd } 750 25)

    # VLC can show or recreate several controller windows on hover; every survivor must
    # retain an empty region, including hidden instances.
    $fullscreenVisible = VisibleRect $fpip
    if (-not $fullscreenVisible.Found) { throw "fullscreen PiP visible region was not measurable" }
    HoverWiggle ([int](($fullscreenVisible.Left + $fullscreenVisible.Right) / 2)) `
        ([int](($fullscreenVisible.Top + $fullscreenVisible.Bottom) / 2))
    Check "controller: every matching window remains veiled through hover" `
        (WaitFor { AllControllersVeiled $fs.hwnd } 1000 50)

    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $fout = Status
    Check "fullscreen exit: fullscreen and topmost state restored" `
        (IsFullscreenRestore $fout $fs)
    # Absence is acceptable; any matching controller that survived must have no veil.
    HoverWiggle $fcx $fcy
    Check "controller: no empty-region veil survives exit" `
        (WaitFor { ([Smoke.Keys]::FscState([IntPtr]::new([long]$fs.hwnd))).Veiled -eq 0 } 2500 60)

    $raceClean = $true
    foreach ($cycle in 1..8) {
        Req "enter"
        $entered = WaitFor {
            (Test-Path $statePath) -and
                [Smoke.Keys]::PipLanded([IntPtr]::new([long]$fs.hwnd), [int]($fs.w / 2))
        } 2000 15
        if (-not $entered) { $raceClean = $false; break }
        $oneShot = Start-Process $exe exit -PassThru -Wait
        $restored = WaitForStableStatus {
            param($status)
            IsFullscreenRestore $status $fs
        } 3000 150
        $unveiled = WaitFor { ([Smoke.Keys]::FscState([IntPtr]::new([long]$fs.hwnd))).Veiled -eq 0 } 1000 25
        if ($oneShot.ExitCode -ne 0 -or -not $restored.Matched -or -not $unveiled) {
            $raceClean = $false
            break
        }
    }
    Check "one-shot exit vs daemon tick: 8 exact clean restores" $raceClean

    Req "enter"
    $installReady = WaitFor {
        (Test-Path $statePath) -and
            [Smoke.Keys]::PipLanded([IntPtr]::new([long]$fs.hwnd), [int]($fs.w / 2))
    } 2000 15
    if (-not $installReady) { throw "install precondition failed: fullscreen PiP did not form" }
    $daemonBeforeInstall = WaitForVerifiedHeartbeat
    if (-not $daemonBeforeInstall) { throw "install precondition failed: daemon heartbeat missing" }
    $installStarted = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    & "$PSScriptRoot\install.ps1"
    $installedRestore = WaitForStableStatus {
        param($status)
        IsFullscreenRestore $status $fs
    } 3000 150
    $installedHeartbeat = WaitForVerifiedHeartbeat $installStarted $daemonBeforeInstall.Pid
    $installedUnveiled = WaitFor { ([Smoke.Keys]::FscState([IntPtr]::new([long]$fs.hwnd))).Veiled -eq 0 } 1000 25
    Check "install during fullscreen PiP: exact restore and verified daemon" `
        ($installedRestore.Matched -and $installedHeartbeat -and $installedHeartbeat.Hotkey -eq 1 -and
            $installedHeartbeat.Timer -eq 1 -and $installedHeartbeat.Pid -ne $daemonBeforeInstall.Pid -and
            $installedUnveiled)

    Req "toggle"
    $null = WaitFor {
        try { -not (Test-Path -LiteralPath "$env:TEMP\vlc-pip-request.txt" -ErrorAction Stop) }
        catch { $false } # delete/open races mean "not consumed yet", not test failure
    } 1500 25
    Req "toggle"
    $null = WaitFor { $d = Status; (-not $d.inPip) -and (-not $d.caption) } 3000 150
    $fdbl = Status
    Check "fullscreen double-toggle: clean round trip" `
        ((-not $fdbl.inPip) -and (-not $fdbl.caption) -and $fdbl.topmost -eq $fs.topmost -and (SameRect $fdbl $fs))

    # leave fullscreen via VLC itself: the window was untouched while fullscreen, so
    # Qt's own restore must land the exact pre-fullscreen rect
    PostKey $fdbl.hwnd 0x46 0x21
    $windowedAgain = WaitForStableStatus {
        param($status)
        $status.caption -and $status.topmost -eq $before.topmost -and (SameRect $status $before)
    } 6000 300
    $fw = $windowedAgain.Status
    Check "fullscreen left: original windowed rect intact" `
        $windowedAgain.Matched
    if (-not $windowedAgain.Matched) { throw "fullscreen exit did not restore the windowed baseline" }

    # stopping playback inside a fullscreen-origin PiP: VLC leaves fullscreen by itself
    # and balloons the window - the daemon must dissolve the session into a plain
    # windowed VLC (frame back, state dropped), never restore the fullscreen shell
    PostKey $fw.hwnd 0x46 0x21
    $stopFullscreen = WaitForStatus {
        param($status)
        $status.found -and (-not $status.caption) -and (-not $status.inPip) -and (SameRect $status $fs)
    } 2500 150
    if (-not $stopFullscreen.Matched) { throw "stop precondition failed: fullscreen did not stabilize" }
    Req "toggle"
    $stopReady = WaitForStatus {
        param($status)
        $status.inPip -and (-not $status.caption) -and
            (Test-Path "$env:TEMP\vlc-pip.state") -and (AllControllersVeiled $status.hwnd)
    } 2000 150
    if (-not $stopReady.Matched) { throw "stop precondition failed: fullscreen PiP state or controller veil missing" }
    $fsp = $stopReady.Status
    PostKey $fsp.hwnd 0x53 0x1F                                  # S = VLC stop
    $dissolve = WaitForStatus {
        param($status)
        $status.caption -and (-not $status.inPip) -and
            (-not (Test-Path "$env:TEMP\vlc-pip.state")) -and
            ([Smoke.Keys]::FscState([IntPtr]::new([long]$status.hwnd))).Veiled -eq 0
    } 3000 150
    $dis = $dissolve.Status
    $dissolvedControllers = [Smoke.Keys]::FscState([IntPtr]::new([long]$dis.hwnd))
    Check "stop in fullscreen pip: session dissolves windowed" `
        ($dissolve.Matched -and $dis.caption -and (-not $dis.inPip) -and
            (-not (Test-Path "$env:TEMP\vlc-pip.state")) -and $dissolvedControllers.Veiled -eq 0)

    # Restart after the stop scenario so the heal target is a stable, valid VLC rect,
    # not the captioned PiP-size shell left by the fullscreen dissolve.
    $vlcProc.CloseMainWindow() | Out-Null
    if (-not $vlcProc.WaitForExit(8000)) { Stop-Process -Id $vlcProc.Id -Force -Confirm:$false }
    $vlcProc = Start-Process $vlcPath 'screen://' -PassThru
    $healBaseline = WaitForStableStatus { param($status) $status.found -and $status.caption } 6000 300
    if (-not $healBaseline.Matched) { throw "reopen-heal setup failed: window did not stabilize" }
    $pre = $healBaseline.Status
    Req "enter"
    $healReady = WaitForStatus {
        param($status)
        $status.inPip -and (Test-Path "$env:TEMP\vlc-pip.state")
    } 3000 150
    if (-not $healReady.Matched) { throw "reopen-heal precondition failed: PiP state was not persisted" }
    $vlcProc.CloseMainWindow() | Out-Null
    if (-not $vlcProc.WaitForExit(8000)) {
        Stop-Process -Id $vlcProc.Id -Force -Confirm:$false
        throw "reopen-heal precondition failed: VLC did not close cleanly"
    }
    if (-not (Test-Path "$env:TEMP\vlc-pip.state")) {
        throw "reopen-heal precondition failed: state was deleted before recorded VLC exited"
    }
    $pendingState = [IO.File]::ReadAllText("$env:TEMP\vlc-pip.state")
    $daemonBeforeUninstall = WaitForVerifiedHeartbeat
    if (-not $daemonBeforeUninstall) { throw "pending-heal uninstall precondition failed: daemon heartbeat missing" }
    $uninstallStarted = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $uninstall = Start-Process powershell.exe `
        "-NoProfile -ExecutionPolicy Bypass -File `"$PSScriptRoot\uninstall.ps1`"" `
        -PassThru -WindowStyle Hidden
    if (-not $uninstall.WaitForExit(15000)) {
        try { $uninstall.Kill() } catch {}
        if (-not $uninstall.WaitForExit(3000)) { throw "pending-heal uninstall could not be terminated" }
        throw "pending-heal uninstall did not terminate"
    }
    $recoveryHeartbeat = WaitForVerifiedHeartbeat $uninstallStarted $daemonBeforeUninstall.Pid
    $statePreserved = (Test-Path -LiteralPath "$env:TEMP\vlc-pip.state" -PathType Leaf) -and
        [IO.File]::ReadAllText("$env:TEMP\vlc-pip.state") -eq $pendingState
    $uninstallRefused = $uninstall.ExitCode -ne 0 -and $statePreserved -and
        (Test-Path -LiteralPath $exe -PathType Leaf) -and
        (Test-Path -LiteralPath $luaPath -PathType Leaf) -and
        (Test-Path -LiteralPath $startupLink -PathType Leaf) -and $recoveryHeartbeat -and
        $recoveryHeartbeat.Hotkey -eq 1 -and $recoveryHeartbeat.Timer -eq 1
    Check "uninstall during pending heal: refused, state/artifacts preserved, daemon restarted" $uninstallRefused
    if (-not $uninstallRefused) { throw "pending-heal uninstall invariant failed" }
    Start-Sleep 1
    $vlcProc = Start-Process $vlcPath 'screen://' -PassThru
    $healDone = WaitForStatus {
        param($status)
        (-not (Test-Path "$env:TEMP\vlc-pip.state")) -and (SameRect $status $pre)
    } 12000 300   # startup + apply + stick + delete
    $healed = $healDone.Status
    Check "reopen heal: state cleared, window at pre-pip position" `
        ($healDone.Matched -and (SameRect $healed $pre) -and (-not (Test-Path "$env:TEMP\vlc-pip.state")))
    # Clean-close this instance so VLC persists the healed geometry (the finally
    # force-kill would strand the PiP rect in vlc-qt-interface.ini for the next launch)
    $preserveHealAfterStop = $true
    $vlcProc.CloseMainWindow() | Out-Null
    if (-not $vlcProc.WaitForExit(8000)) { throw "healed VLC did not close cleanly" }
    $preserveHealAfterStop = $false
}
finally {
    $cleanupErrors = @()
    # Restore a live window, but preserve pending heal state for the next VLC launch.
    try {
        if (-not $preserveHealAfterStop -and (Test-Path $exe)) {
            Resolve-PipState $statePath $exe -RequireRestore
        }
    }
    catch { $cleanupErrors += $_.Exception.Message }
    try {
        if ($vlcProc) {
            if (-not $vlcProc.HasExited) {
                try { $vlcProc.Kill() } catch {}
            }
            if (-not $vlcProc.WaitForExit(3000)) { throw "VLC did not stop" }
        }
        if ($preserveHealAfterStop -and -not (Test-Path -LiteralPath $statePath)) {
            [IO.File]::WriteAllText($statePath, $pendingState, [Text.UTF8Encoding]::new($false))
        }
        if ($preserveHealAfterStop -and
            [IO.File]::ReadAllText($statePath) -ne $pendingState) {
            throw "pending heal state could not be preserved"
        }
    } catch { $cleanupErrors += $_.Exception.Message }
    try {
        if (Test-Path $cfgBak) {
            New-Item -ItemType Directory -Path (Split-Path $cfg -Parent) -Force | Out-Null
            Move-Item $cfgBak $cfg -Force
        }
        elseif (-not $hadConfig -and (Test-Path $cfg)) { Remove-Item $cfg -Force }
    } catch { $cleanupErrors += $_.Exception.Message }
    try {
        $cleanupHeartbeat = WaitForVerifiedHeartbeat 0 0 1200
        $installationHealthy = (Test-Path -LiteralPath $exe -PathType Leaf) -and
            (Test-Path -LiteralPath $luaPath -PathType Leaf) -and
            (Test-Path -LiteralPath $startupLink -PathType Leaf) -and $cleanupHeartbeat -and
            $cleanupHeartbeat.Hotkey -eq 1 -and $cleanupHeartbeat.Timer -eq 1
        if (-not $installationHealthy) {
            & "$PSScriptRoot\install.ps1"
            $cleanupHeartbeat = WaitForVerifiedHeartbeat
            if (-not $cleanupHeartbeat -or $cleanupHeartbeat.Hotkey -ne 1 -or $cleanupHeartbeat.Timer -ne 1) {
                throw "reinstalled daemon could not be verified"
            }
        }
    } catch { $cleanupErrors += $_.Exception.Message }
    if ($cleanupErrors.Count) {
        throw "smoke cleanup failed: $($cleanupErrors -join '; ')"
    }
}

if ($fail.Count) { Write-Host "`n$($fail.Count) FAILURES"; exit 1 } else { Write-Host "`nALL PASS"; exit 0 }
