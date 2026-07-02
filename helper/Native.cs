using System.Runtime.InteropServices;
using System.Text;

namespace PipHelper;

public record PipOptions(int W = 480, int H = 270, string Corner = "br", int Margin = 16, bool Min = true);

public static class Native
{
    // ---- constants ----
    const int GWL_STYLE = -16, GWL_EXSTYLE = -20;
    const long WS_CAPTION = 0x00C00000, WS_THICKFRAME = 0x00040000, WS_MAXIMIZE = 0x01000000;
    const uint SWP_FRAMECHANGED = 0x0020, SWP_SHOWWINDOW = 0x0040;
    static readonly IntPtr HWND_TOPMOST = new(-1), HWND_NOTOPMOST = new(-2);
    const long WS_EX_TOPMOST = 0x00000008;
    const uint MONITOR_DEFAULTTONEAREST = 2;
    const int SW_RESTORE = 9;

    // ---- P/Invoke ----
    delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] static extern long GetWindowLongPtrW(IntPtr h, int idx);
    [DllImport("user32.dll")] static extern long SetWindowLongPtrW(IntPtr h, int idx, long val);
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint flags);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern IntPtr MonitorFromWindow(IntPtr h, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool GetMonitorInfoW(IntPtr mon, ref MONITORINFO mi);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr parent, EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] static extern int SetWindowRgn(IntPtr h, IntPtr rgn, bool redraw);
    [DllImport("user32.dll")] static extern int GetWindowRgn(IntPtr h, IntPtr rgn);
    [DllImport("gdi32.dll")] static extern IntPtr CreateRectRgn(int l, int t, int r, int b);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr obj);

    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] struct MONITORINFO { public int cbSize; public RECT rcMonitor, rcWork; public uint dwFlags; }

    public static void EnableDpiAwareness() => SetProcessDpiAwarenessContext(new IntPtr(-4)); // PER_MONITOR_AWARE_V2

    // ---- find the VLC player window ----
    public static IntPtr FindPlayer()
    {
        var procs = System.Diagnostics.Process.GetProcessesByName("vlc");
        var vlcPids = procs.Select(p => (uint)p.Id).ToHashSet();
        foreach (var p in procs) p.Dispose();
        if (vlcPids.Count == 0) return IntPtr.Zero;

        IntPtr best = IntPtr.Zero, biggest = IntPtr.Zero;
        long biggestArea = 0;
        EnumWindows((h, _) =>
        {
            if (!IsWindowVisible(h)) return true;
            GetWindowThreadProcessId(h, out var pid);
            if (!vlcPids.Contains(pid)) return true;
            var sb = new StringBuilder(256);
            GetWindowText(h, sb, 256);
            var title = sb.ToString();
            if (title.Length == 0) return true;
            if (title.Contains("VLC media player", StringComparison.OrdinalIgnoreCase)) { best = h; return false; }
            GetWindowRect(h, out var r);
            long area = (long)(r.Right - r.Left) * (r.Bottom - r.Top);
            if (area > biggestArea) { biggestArea = area; biggest = h; }
            return true;
        }, IntPtr.Zero);
        return best != IntPtr.Zero ? best : biggest;
    }

    // Windows recycles HWND values: after VLC dies, the saved handle can belong to another
    // app. IsWindow alone would pass and we'd reshape a foreign window; require the owner
    // PID recorded at Enter. Old state files (Pid=0) read as stale by design.
    static bool OwnsState(PipState s)
    {
        var h = new IntPtr(s.Hwnd);
        if (!IsWindow(h)) return false;
        GetWindowThreadProcessId(h, out var p);
        return p != 0 && p == s.Pid;
    }

    static void TryDeleteState() { try { File.Delete(PipState.StatePath); } catch { } } // transient lock: next caller retries

    public static bool InPip()
    {
        var s = PipState.Load(PipState.StatePath);
        if (s is null) return false;
        if (!OwnsState(s)) { TryDeleteState(); return false; } // stale: VLC gone or hwnd recycled
        return true;
    }

    public static bool Enter(IntPtr h, PipOptions o)
    {
        if (h == IntPtr.Zero || InPip()) return false;
        if (IsIconic(h)) ShowWindow(h, SW_RESTORE); // else the off-screen iconic rect gets saved as the restore state
        GetWindowRect(h, out var r);
        long style = GetWindowLongPtrW(h, GWL_STYLE);
        long ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        GetWindowThreadProcessId(h, out var pid);
        PipState.Save(new PipState(h.ToInt64(), r.Left, r.Top, r.Right - r.Left, r.Bottom - r.Top, style, ex,
            o.W, o.H, o.Corner, o.Margin, o.Min, pid), PipState.StatePath);

        // also strip WS_MAXIMIZE: a zoomed window keeps IsZoomed, so Win+Down/Aero would
        // snap the PiP back to Qt's normal placement rect
        SetWindowLongPtrW(h, GWL_STYLE, style & ~(WS_CAPTION | WS_THICKFRAME | WS_MAXIMIZE));
        var mi = new MONITORINFO { cbSize = Marshal.SizeOf<MONITORINFO>() };
        GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), ref mi);
        var (x, y) = PipGeometry.ComputeCorner(mi.rcWork.Left, mi.rcWork.Top, mi.rcWork.Right, mi.rcWork.Bottom, o.W, o.H, o.Corner, o.Margin);
        var ok = SetWindowPos(h, HWND_TOPMOST, x, y, o.W, o.H, SWP_FRAMECHANGED | SWP_SHOWWINDOW);
        if (!ok) { SetWindowLongPtrW(h, GWL_STYLE, style); TryDeleteState(); } // e.g. UIPI vs elevated VLC: don't claim in-PiP
        return ok;
    }

    public static bool Exit()
    {
        var s = PipState.Load(PipState.StatePath);
        if (s is null) return false;
        var h = new IntPtr(s.Hwnd);
        if (!OwnsState(s)) { TryDeleteState(); return false; } // stale: VLC gone or hwnd recycled
        SetWindowRgn(h, IntPtr.Zero, true); // drop the minimal-look clip before restoring
        SetWindowLongPtrW(h, GWL_STYLE, s.Style);
        SetWindowLongPtrW(h, GWL_EXSTYLE, s.ExStyle);
        // WS_EX_TOPMOST only changes via SetWindowPos: honor the user's own always-on-top
        var ok = SetWindowPos(h, (s.ExStyle & WS_EX_TOPMOST) != 0 ? HWND_TOPMOST : HWND_NOTOPMOST,
                              s.X, s.Y, s.W, s.H, SWP_FRAMECHANGED | SWP_SHOWWINDOW);
        if (ok || !IsWindow(h)) TryDeleteState(); // live-window restore failure keeps state so the next toggle retries
        return ok;
    }

    public static bool Toggle(PipOptions o) => InPip() ? Exit() : Enter(FindPlayer(), o);

    // ---- minimal look (Ctrl+H-like) via SetWindowRgn on the video child area ----
    // VLC 3.x hosts the video in a native child whose class starts with "VLC video main".
    static IntPtr FindVideoChild(IntPtr top)
    {
        IntPtr found = IntPtr.Zero;
        EnumChildWindows(top, (c, _) =>
        {
            if (!IsWindowVisible(c)) return true;
            var sb = new StringBuilder(128);
            GetClassName(c, sb, 128);
            if (sb.ToString().StartsWith("VLC video main", StringComparison.Ordinal)) { found = c; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    static bool HasRegion(IntPtr h)
    {
        var probe = CreateRectRgn(0, 0, 0, 0);
        try { return GetWindowRgn(h, probe) != 0; } // 0 = ERROR (no region)
        finally { DeleteObject(probe); }
    }

    static RECT _prevWin, _prevChild;
    static bool _havePrev;

    static bool SameRect(RECT a, RECT b) =>
        a.Left == b.Left && a.Top == b.Top && a.Right == b.Right && a.Bottom == b.Bottom;

    /// Converging per-tick maintenance, called by the daemon timer (and one-shot enter):
    /// no video -> clear region; video child not yet at target size -> resize window with
    /// chrome compensation; child at target -> clip window to the video area. Geometry
    /// targets come from the state file (recorded at Enter), so daemon and one-shot agree.
    /// Acts only on STABLE frames (window+child rects unchanged since the previous tick):
    /// VLC re-fits the child asynchronously after our resize, so a fresh measurement can be
    /// stale and yield garbage chrome (observed: perpetual resize thrash in the daemon).
    public static void MaintainRegion()
    {
        var s = PipState.Load(PipState.StatePath);
        if (s is null) { _havePrev = false; return; }
        var h = new IntPtr(s.Hwnd);
        if (!OwnsState(s)) { _havePrev = false; TryDeleteState(); return; } // stale: VLC gone or hwnd recycled
        if (!s.Min) return;
        var o = new PipOptions(s.TargetW, s.TargetH, s.Corner, s.Margin, s.Min);

        var child = FindVideoChild(h);
        if (child == IntPtr.Zero)
        {
            _havePrev = false;
            if (HasRegion(h)) SetWindowRgn(h, IntPtr.Zero, true); // playback stopped: show full mini UI
            return;
        }

        GetWindowRect(h, out var wr);
        GetWindowRect(child, out var cr);
        bool stable = _havePrev && SameRect(wr, _prevWin) && SameRect(cr, _prevChild);
        _prevWin = wr; _prevChild = cr; _havePrev = true;
        if (!stable) return; // wait until VLC's re-layout settles

        int relL = cr.Left - wr.Left, relT = cr.Top - wr.Top;
        int cw = cr.Right - cr.Left, ch = cr.Bottom - cr.Top;
        int chromeW = (wr.Right - wr.Left) - cw, chromeH = (wr.Bottom - wr.Top) - ch;
        if (chromeW < 0 || chromeW > 300 || chromeH < 0 || chromeH > 300) return; // child not re-fit yet: real chrome (menu + controller + borders) is well under 300px; negative or huge delta = stale rects from VLC's async re-layout

        if (Math.Abs(cw - o.W) > 2 || Math.Abs(ch - o.H) > 2)
        {
            // chrome = window minus video child; grow the window so the video itself is WxH
            var mi = new MONITORINFO { cbSize = Marshal.SizeOf<MONITORINFO>() };
            GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), ref mi);
            var (vx, vy) = PipGeometry.ComputeCorner(mi.rcWork.Left, mi.rcWork.Top, mi.rcWork.Right, mi.rcWork.Bottom, o.W, o.H, o.Corner, o.Margin);
            int tw = o.W + chromeW, th = o.H + chromeH, tx = vx - relL, ty = vy - relT;
            if (tw <= 0 || th <= 0) return;
            if (wr.Left != tx || wr.Top != ty || wr.Right - wr.Left != tw || wr.Bottom - wr.Top != th)
            {
                SetWindowPos(h, HWND_TOPMOST, tx, ty, tw, th, SWP_FRAMECHANGED);
                _havePrev = false; // our own resize invalidates the measurement
            }
            return;
        }

        if (!HasRegion(h))
        {
            var rgn = CreateRectRgn(relL, relT, relL + cw, relT + ch);
            if (SetWindowRgn(h, rgn, true) == 0) DeleteObject(rgn); // system owns rgn only on success
        }
    }

    public static string StatusPath => Path.Combine(Path.GetTempPath(), "vlc-pip-status.json");

    public static string Status()
    {
        var h = FindPlayer();
        if (h == IntPtr.Zero) return """{"found":false}""";
        GetWindowRect(h, out var r);
        long style = GetWindowLongPtrW(h, GWL_STYLE);
        long ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        bool caption = (style & WS_CAPTION) == WS_CAPTION;
        bool topmost = (ex & WS_EX_TOPMOST) != 0;
        bool minimal = HasRegion(h);
        return $$"""{"found":true,"hwnd":{{h.ToInt64()}},"x":{{r.Left}},"y":{{r.Top}},"w":{{r.Right - r.Left}},"h":{{r.Bottom - r.Top}},"caption":{{(caption ? "true" : "false")}},"topmost":{{(topmost ? "true" : "false")}},"inPip":{{(InPip() ? "true" : "false")}},"minimal":{{(minimal ? "true" : "false")}}}""";
    }
}
