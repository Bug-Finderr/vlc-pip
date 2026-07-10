# Run after install.ps1 with no VLC instance open.
$ErrorActionPreference = "Stop"
. "$PSScriptRoot\common.ps1"

$exe = "$env:APPDATA\vlc\pip\pip-helper.exe"
$heartbeatPath = "$env:TEMP\vlc-pip-daemon.alive"

# WinExe stdout is invisible to PowerShell capture; run the helper and read its
# status-file channel instead of capturing output.
function Status {
    Start-Process $exe status -Wait
    Get-Content "$env:TEMP\vlc-pip-status.json" -Raw | ConvertFrom-Json
}
function Req($cmd) { Set-Content "$env:TEMP\vlc-pip-request.txt" $cmd }
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
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT rect);
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
public static bool PipLanded(IntPtr h, int maxWidth) {
    RECT rect;
    return GetWindowRect(h, out rect)
        && (GetWindowLongW(h, -16) & WS_CAPTION) != WS_CAPTION
        && (GetWindowLongW(h, -20) & WS_EX_TOPMOST) != 0
        && rect.Right - rect.Left < maxWidth;
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
    Start-Sleep -Milliseconds 400                             # drag-end write lands
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
$daemonIdentity = $daemon -and $daemon.Path -and (Test-SamePath $daemon.Path $exe)
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
$cfg = "$env:APPDATA\vlc\pip\config.txt"
$cfgBak = "$cfg.smoke-bak"
$vlcProc = $null

try {
    # Park inside the protected block so every later throw restores the user's config.
    if (Test-Path $cfg) { Move-Item $cfg $cfgBak -Force }

    # screen:// = live playing video, so the video child window and minimal-look region exist
    $vlcProc = Start-Process $vlcPath 'screen://' -PassThru
    $startup = WaitForStableStatus { param($status) $status.found -and $status.caption } 6000 300
    $before = $startup.Status
    Check "vlc ready (windowed, caption)" ($startup.Matched -and $before.found -and $before.caption)

    Req "toggle"
    $null = WaitFor { (Status).minimal } 4000 150
    $pip = Status
    Check "enter: pip formed (borderless, topmost, video 480w, region, state)" `
        ((-not $pip.caption) -and $pip.topmost -and $pip.w -eq 480 -and $pip.inPip -and $pip.minimal)

    # Five clicks cover adjacent and nonadjacent double/triple-click pairing.
    $cx = $pip.x + [int]($pip.w / 2); $cy = $pip.y + [int]($pip.h / 2)
    ClickAt $cx $cy 5; $afterSpam = Status
    Check "click burst (dbl/triple/spam): rect unchanged, still pip" `
        ((SameRect $afterSpam $pip) -and $afterSpam.inPip)

    DragFrom $cx $cy ($cx - 220) ($cy - 160)
    $moved = Status
    Check "drag-move: at delta, size held, still pip" `
        ([math]::Abs($moved.x - ($pip.x - 220)) -le 2 -and [math]::Abs($moved.y - ($pip.y - 160)) -le 2 -and $moved.w -eq $pip.w -and $moved.h -eq $pip.h -and $moved.inPip)
    Check "drag-move: config persisted (w/h + corner)" `
        ((Test-Path $cfg) -and ((Get-Content $cfg -Raw).Trim() -match '^w=\d+ h=\d+ c=br$'))

    ClickAt ($moved.x + $moved.w - 8) ($moved.y + [int]($moved.h / 2)) 1
    $bandClick = Status
    Check "band click: no resize, no move" ((SameRect $bandClick $moved) -and $bandClick.inPip)

    # right edge at mid-height: horizontal chrome is 0 so window right == visible right,
    # while the top/bottom strips are region-clipped chrome (corner drags are manual)
    DragFrom ($moved.x + $moved.w - 8) ($moved.y + [int]($moved.h / 2)) ($moved.x + $moved.w - 108) ($moved.y + [int]($moved.h / 2))
    $null = WaitFor { $s = Status; $s.w -lt $moved.w -and $s.minimal } 4000 150   # convergence re-clips
    $rs = Status
    Check "drag-resize (right edge): width shrank, minimal look held" ($rs.w -lt $moved.w -and $rs.inPip -and $rs.minimal)

    [Smoke.Keys]::SetCursorPos(($rs.x + [int]($rs.w / 2)), ($rs.y + [int]($rs.h / 2))) | Out-Null
    Start-Sleep -Milliseconds 100
    [Smoke.Keys]::mouse_event(0x0800, 0, 0, 120, [UIntPtr]::Zero)   # WHEEL up one notch
    Start-Sleep -Milliseconds 400
    $wheeled = Status
    Check "wheel: size untouched (volume, not resize)" ($wheeled.w -eq $rs.w -and $wheeled.h -eq $rs.h)

    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $after = Status
    Check "exit: exact windowed restore (caption, rect, topmost, region/state)" `
        ($after.caption -and $after.topmost -eq $before.topmost -and (-not $after.inPip) -and (-not $after.minimal) -and $after.x -eq $before.x -and $after.y -eq $before.y -and $after.w -eq $before.w -and $after.h -eq $before.h)

    Req "toggle"
    $null = WaitFor { (Status).inPip } 3000 150
    $re = Status
    Check "persist: re-enter at gestured width" ([math]::Abs($re.w - $rs.w) -le 2)
    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150

    SendCtrlAltP
    Check "hotkey enters pip" (WaitFor { (Status).inPip } 3000 150)
    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $s = Status
    Check "interleave hotkey+request: no desync, exact rect" `
        ((-not $s.inPip) -and $s.topmost -eq $before.topmost -and $s.x -eq $before.x -and $s.y -eq $before.y -and $s.w -eq $before.w -and $s.h -eq $before.h)

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
    HoverWiggle ($fpip.x + [int]($fpip.w / 2)) ($fpip.y + [int]($fpip.h / 2))
    Check "controller: every matching window remains veiled through hover" `
        (WaitFor { AllControllersVeiled $fs.hwnd } 1000 50)

    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $fout = Status
    Check "fullscreen exit: fullscreen and topmost state restored" `
        ((-not $fout.caption) -and (-not $fout.inPip) -and $fout.topmost -eq $fs.topmost -and (SameRect $fout $fs))
    # Absence is acceptable; any matching controller that survived must have no veil.
    HoverWiggle $fcx $fcy
    Check "controller: no empty-region veil survives exit" `
        (WaitFor { ([Smoke.Keys]::FscState([IntPtr]::new([long]$fs.hwnd))).Veiled -eq 0 } 2500 60)

    Req "toggle"
    $null = WaitFor { -not (Test-Path "$env:TEMP\vlc-pip-request.txt") } 1500 25    # first consumed
    Req "toggle"
    $null = WaitFor { $d = Status; (-not $d.inPip) -and (-not $d.caption) } 3000 150
    $fdbl = Status
    Check "fullscreen double-toggle: clean round trip" `
        ((-not $fdbl.inPip) -and (-not $fdbl.caption) -and $fdbl.topmost -eq $fs.topmost -and (SameRect $fdbl $fs))

    # leave fullscreen via VLC itself: the window was untouched while fullscreen, so
    # Qt's own restore must land the exact pre-fullscreen rect
    PostKey $fdbl.hwnd 0x46 0x21
    $null = WaitFor { $w = Status; $w.caption -and $w.topmost -eq $before.topmost -and $w.w -eq $before.w } 3000 150
    $fw = Status
    Check "fullscreen left: original windowed rect intact" `
        ($fw.caption -and $fw.topmost -eq $before.topmost -and $fw.x -eq $before.x -and $fw.y -eq $before.y -and $fw.w -eq $before.w -and $fw.h -eq $before.h)

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
    $vlcProc.CloseMainWindow() | Out-Null
    $vlcProc.WaitForExit(8000) | Out-Null
}
finally {
    # restore the window if the test aborted mid-PiP (no-op otherwise, works without the daemon)
    try { Start-Process $exe exit -Wait } catch {}
    # put the user's config back (or clear the one this run wrote) before tearing VLC down
    if (Test-Path $cfgBak) { Move-Item $cfgBak $cfg -Force }
    elseif (Test-Path $cfg) { Remove-Item $cfg -Force -ErrorAction SilentlyContinue }
    if ($vlcProc -and -not $vlcProc.HasExited) { Stop-Process -Id $vlcProc.Id -Force -Confirm:$false -ErrorAction SilentlyContinue }
}

if ($fail.Count) { Write-Host "`n$($fail.Count) FAILURES"; exit 1 } else { Write-Host "`nALL PASS"; exit 0 }
