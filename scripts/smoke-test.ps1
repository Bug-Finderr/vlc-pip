# End-to-end smoke test against live VLC. Run AFTER scripts\install.ps1 (daemon must be running).
# One check per behavioral contract; waits poll for their condition instead of sleeping a
# guessed duration - faster, and a late condition fails the check instead of flaking it.
$ErrorActionPreference = "Stop"
$exe = "$env:APPDATA\vlc\pip\pip-helper.exe"
$heartbeatPath = "$env:TEMP\vlc-pip-daemon.alive"

# WinExe stdout is invisible to PowerShell capture; run the helper and read its
# status-file channel instead of capturing output.
function Status {
    Start-Process $exe status -Wait
    Get-Content "$env:TEMP\vlc-pip-status.json" -Raw | ConvertFrom-Json
}
function Req($cmd) { Set-Content "$env:TEMP\vlc-pip-request.txt" $cmd }
# poll until the condition holds (true) or the cap passes (returns the final evaluation)
function WaitFor([scriptblock]$cond, [int]$capMs = 3000, [int]$stepMs = 60) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $capMs) {
        if (& $cond) { return $true }
        Start-Sleep -Milliseconds $stepMs
    }
    & $cond
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
function SamePath($left, $right) {
    try {
        [string]::Equals(
            (Resolve-Path -LiteralPath $left).Path,
            (Resolve-Path -LiteralPath $right).Path,
            [StringComparison]::OrdinalIgnoreCase
        )
    }
    catch { $false }
}
$fail = @()
function Check($name, $cond) {
    if ($cond) { Write-Host "PASS $name" }
    else {
        # the status file still holds the snapshot the check asserted on: dump it so a
        # FAIL is self-diagnosing without re-running the whole live session
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
[DllImport("user32.dll")] public static extern int GetWindowRgn(IntPtr h, IntPtr rgn);
[DllImport("gdi32.dll")] public static extern int GetRgnBox(IntPtr rgn, out RECT box);
[DllImport("gdi32.dll")] public static extern IntPtr CreateRectRgn(int left, int top, int right, int bottom);
[DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr obj);
public delegate bool EnumProc(IntPtr h, IntPtr l);
private const int NULLREGION = 1;
public struct RECT { public int Left, Top, Right, Bottom; }
public sealed class ControllerState {
    public int Found;
    public int Veiled;
    public int Rendered;
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
# post a key to VLC's vout (focus-independent); scan code matters for VLC's translation
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
function InsideVirtualDesktop($rect, $desktop) {
    $rect.x -ge $desktop.X -and $rect.y -ge $desktop.Y -and
        $rect.x + $rect.w -le $desktop.X + $desktop.Width -and
        $rect.y + $rect.h -le $desktop.Y + $desktop.Height
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
$daemonIdentity = $daemon -and $daemon.Path -and (SamePath $daemon.Path $exe)
Check "daemon heartbeat: pid owns installed helper" `
    $daemonIdentity
# Idle session hooks may be absent; the global hotkey and timer are mandatory daemon arms.
$daemonArmed = $heartbeat -and $heartbeat.Hotkey -eq 1 -and $heartbeat.Timer -eq 1
Check "daemon heartbeat: hotkey/timer armed" `
    $daemonArmed
if (-not ($heartbeatReady -and $daemonIdentity -and $daemonArmed)) {
    throw "daemon heartbeat preflight failed"
}

$vlcDir = (Get-ItemProperty 'HKLM:\SOFTWARE\VideoLAN\VLC' -ErrorAction SilentlyContinue).InstallDir
$vlcPath = if ($vlcDir) { Join-Path $vlcDir 'vlc.exe' } else { 'C:\Program Files\VideoLAN\VLC\vlc.exe' }
if (-not (Test-Path $vlcPath)) { throw "vlc.exe not found" }
if (Get-Process vlc -ErrorAction SilentlyContinue) {
    throw "Close VLC first: this test resizes, clicks, and kills the VLC instance it targets"
}
# v2.1: gestures persist to config.txt - park it so the run starts from defaults
$cfg = "$env:APPDATA\vlc\pip\config.txt"
$cfgBak = "$cfg.smoke-bak"
$vlcProc = $null

try {
    # Park inside the protected block so every later throw restores the user's config.
    if (Test-Path $cfg) { Move-Item $cfg $cfgBak -Force }

    # screen:// = live playing video, so the video child window and minimal-look region exist
    $vlcProc = Start-Process $vlcPath 'screen://' -PassThru
    # $before anchors exact-rect checks: wait for two identical startup samples.
    $prevR = $null
    for ($i = 0; $i -lt 20; $i++) {
        $cur = Status
        if ($cur.found -and $prevR -and $cur.x -eq $prevR.x -and $cur.y -eq $prevR.y -and $cur.w -eq $prevR.w -and $cur.h -eq $prevR.h) { break }
        $prevR = $cur
        Start-Sleep -Milliseconds 300
    }

    $before = Status
    Check "vlc ready (windowed, caption)" ($before.found -and $before.caption)
    $desktop = VirtualDesktop
    Check "virtual desktop: coordinate bounds available" `
        ($desktop.Width -gt 1 -and $desktop.Height -gt 1 -and $desktop.Monitors -ge 1)
    if ($desktop.Monitors -gt 1) {
        $primaryWidth = [Smoke.Keys]::GetSystemMetrics(0); $primaryHeight = [Smoke.Keys]::GetSystemMetrics(1)
        Check "multi-monitor: virtual bounds extend beyond primary display" `
            ($desktop.X -lt 0 -or $desktop.Y -lt 0 -or $desktop.Width -gt $primaryWidth -or $desktop.Height -gt $primaryHeight)
    }

    Req "toggle"
    $null = WaitFor { (Status).minimal } 4000 150
    $pip = Status
    Check "enter: pip formed (borderless, topmost, video 480w, region, state)" `
        ((-not $pip.caption) -and $pip.topmost -and $pip.w -eq 480 -and $pip.inPip -and $pip.minimal)

    # fullscreen prevention: a 5-click burst subsumes double and triple click - every
    # down after the first ALLOWED one must be swallowed, so no OS double-click can
    # ever synthesize (v1 bugs: dblclick fullscreened; clicks 1+3 paired on triple)
    $cx = $pip.x + [int]($pip.w / 2); $cy = $pip.y + [int]($pip.h / 2)
    ClickAt $cx $cy 5; $afterSpam = Status
    Check "click burst (dbl/triple/spam): rect unchanged, still pip" `
        ($afterSpam.x -eq $pip.x -and $afterSpam.w -eq $pip.w -and $afterSpam.h -eq $pip.h -and $afterSpam.inPip)

    # v2.1 gestures: interior drag = free move; band drag = aspect-locked resize; wheel untouched
    DragFrom $cx $cy ($cx - 220) ($cy - 160)
    $moved = Status
    Check "drag-move: at delta, size held, still pip" `
        ([math]::Abs($moved.x - ($pip.x - 220)) -le 2 -and [math]::Abs($moved.y - ($pip.y - 160)) -le 2 -and $moved.w -eq $pip.w -and $moved.h -eq $pip.h -and $moved.inPip)
    if ($desktop.Monitors -gt 1) {
        Check "multi-monitor: injected drag stays inside virtual desktop" `
            (InsideVirtualDesktop $moved $desktop)
    }
    Check "drag-move: config persisted (w/h + corner)" `
        ((Test-Path $cfg) -and ((Get-Content $cfg -Raw).Trim() -match '^w=\d+ h=\d+ c=br$'))

    ClickAt ($moved.x + $moved.w - 8) ($moved.y + [int]($moved.h / 2)) 1
    $bandClick = Status
    Check "band click: no resize, no move" ($bandClick.x -eq $moved.x -and $bandClick.w -eq $moved.w -and $bandClick.inPip)

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

    # persistence: re-enter picks the gestured size from config.txt
    Req "toggle"
    $null = WaitFor { (Status).inPip } 3000 150
    $re = Status
    Check "persist: re-enter at gestured width" ([math]::Abs($re.w - $rs.w) -le 2)
    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150

    # global hotkey enters, request-file exits: both paths share one state
    SendCtrlAltP
    Check "hotkey enters pip" (WaitFor { (Status).inPip } 3000 150)
    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $s = Status
    Check "interleave hotkey+request: no desync, exact rect" `
        ((-not $s.inPip) -and $s.topmost -eq $before.topmost -and $s.x -eq $before.x -and $s.y -eq $before.y -and $s.w -eq $before.w -and $s.h -eq $before.h)

    # F posted to the vout = VLC's fullscreen hotkey, focus-independent (SPEC section 7).
    PostKey $s.hwnd 0x46 0x21
    # caption-only: Qt autoresize can make the windowed size already match the monitor
    $null = WaitFor { -not (Status).caption } 2500 150
    $fs = Status
    Check "fullscreen: engaged" (-not $fs.caption)

    # Summon the strip first: the realistic toggle starts with it rendered. Use the
    # fullscreen rect center rather than the primary monitor center.
    $fcx = $fs.x + [int]($fs.w / 2); $fcy = $fs.y + [int]($fs.h / 2)
    if ($desktop.Monitors -gt 1) {
        Check "multi-monitor: fullscreen hover target is inside virtual desktop" `
            ($fcx -ge $desktop.X -and $fcx -lt $desktop.X + $desktop.Width -and $fcy -ge $desktop.Y -and $fcy -lt $desktop.Y + $desktop.Height)
    }
    HoverWiggle $fcx $fcy
    $stripUp = WaitFor { ([Smoke.Keys]::FscState([IntPtr]::new([long]$fs.hwnd))).Rendered -gt 0 } 2500 60
    Check "controller: rendered before fullscreen PiP enter" $stripUp

    # the enter is one immediate reshape - no handoff wait
    $hsw = [System.Diagnostics.Stopwatch]::StartNew()
    Req "toggle"
    $null = WaitFor { Test-Path "$env:TEMP\vlc-pip.state" } 2000 15
    $enterMs = $hsw.ElapsedMilliseconds
    $null = WaitFor { (Status).inPip } 2000 150
    $fpip = Status
    Check "fullscreen enter: immediate (<500ms), pip-sized" `
        ($enterMs -lt 500 -and $fpip.inPip -and (-not $fpip.caption) -and $fpip.w -lt [int]($fs.w / 2))
    Check "controller: every matching window veiled in fullscreen PiP" `
        (WaitFor { AllControllersVeiled $fs.hwnd } 750 25)

    # VLC can show or recreate several controller windows on hover; every survivor must
    # retain an empty region, including hidden instances.
    HoverWiggle ($fpip.x + [int]($fpip.w / 2)) ($fpip.y + [int]($fpip.h / 2))
    Check "controller: every matching window remains veiled through hover" `
        (WaitFor { AllControllersVeiled $fs.hwnd } 1000 50)

    # Unfocus/refocus forces VLC's visibility state without waiting on a guessed delay.
    ClickAt $fcx $fcy 1
    ClickAt ($fpip.x + [int]($fpip.w / 2)) ($fpip.y + [int]($fpip.h / 2)) 1
    Check "controller: every matching window remains veiled through refocus" `
        (WaitFor { AllControllersVeiled $fs.hwnd } 1000 50)

    # exit returns the user to fullscreen - where they came from, internally consistent
    Req "toggle"
    $null = WaitFor { -not (Status).inPip } 3000 150
    $fout = Status
    Check "fullscreen exit: fullscreen and topmost state restored" `
        ((-not $fout.caption) -and (-not $fout.inPip) -and $fout.topmost -eq $fs.topmost -and $fout.w -eq $fs.w -and $fout.h -eq $fs.h)
    # Absence is acceptable; any matching controller that survived must have no veil.
    HoverWiggle $fcx $fcy
    Check "controller: no empty-region veil survives exit" `
        (WaitFor { ([Smoke.Keys]::FscState([IntPtr]::new([long]$fs.hwnd))).Veiled -eq 0 } 2500 60)

    # both toggles are instant: two rapid presses = a full round trip, no half-states
    Req "toggle"
    $null = WaitFor { -not (Test-Path "$env:TEMP\vlc-pip-request.txt") } 1500 25    # first consumed
    Req "toggle"
    $null = WaitFor { $d = Status; (-not $d.inPip) -and (-not $d.caption) } 3000 150
    $fdbl = Status
    Check "fullscreen double-toggle: clean round trip" `
        ((-not $fdbl.inPip) -and (-not $fdbl.caption) -and $fdbl.topmost -eq $fs.topmost -and $fdbl.w -eq $fs.w)

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
    $null = WaitFor { -not (Status).caption } 2500 150
    Req "toggle"
    $null = WaitFor { (Status).inPip } 2000 150
    $fsp = Status
    PostKey $fsp.hwnd 0x53 0x1F                                  # S = VLC stop
    $null = WaitFor { $d = Status; $d.caption -and -not (Test-Path "$env:TEMP\vlc-pip.state") } 3000 150
    $dis = Status
    Check "stop in fullscreen pip: session dissolves windowed" ($dis.caption -and -not $dis.inPip -and -not (Test-Path "$env:TEMP\vlc-pip.state"))

    # v2.1 heal: a CLEAN close while in PiP makes Qt persist the PiP geometry as VLC's own
    # (a kill persists nothing and would pass even without the heal - verified), so the
    # reopened window would sit full-size at the PiP origin; the daemon heals it back to
    # the pre-PiP rect and deletes the state once the rect sticks. The state-file check
    # polls Test-Path so nothing races the heal's own delete.
    $pre = Status
    Req "enter"
    $null = WaitFor { (Status).inPip } 3000 150
    $vlcProc.CloseMainWindow() | Out-Null
    if (-not $vlcProc.WaitForExit(8000)) { Stop-Process -Id $vlcProc.Id -Force -Confirm:$false }
    Start-Sleep 1
    $vlcProc = Start-Process $vlcPath 'screen://' -PassThru
    $healDone = WaitFor { -not (Test-Path "$env:TEMP\vlc-pip.state") } 12000 300   # startup + apply + stick + delete
    Start-Sleep -Milliseconds 400
    $healed = Status
    Check "reopen heal: state cleared, window at pre-pip position" `
        ($healDone -and [math]::Abs($healed.x - $pre.x) -le 16 -and [math]::Abs($healed.y - $pre.y) -le 16)
    # clean-close this instance too so VLC persists the HEALED geometry (the finally
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
