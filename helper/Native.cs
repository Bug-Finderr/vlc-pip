using System.Runtime.InteropServices;
using System.Text;

namespace PipHelper;

public record PipOptions(int W = 480, int H = 270, string Corner = "br", int Margin = 16);

public static class Native
{
    // ---- constants ----
    const int GWL_STYLE = -16, GWL_EXSTYLE = -20;
    const long WS_CAPTION = 0x00C00000, WS_THICKFRAME = 0x00040000;
    const uint SWP_FRAMECHANGED = 0x0020, SWP_SHOWWINDOW = 0x0040;
    static readonly IntPtr HWND_TOPMOST = new(-1), HWND_NOTOPMOST = new(-2);
    const long WS_EX_TOPMOST = 0x00000008;
    const uint MONITOR_DEFAULTTONEAREST = 2;

    // ---- P/Invoke ----
    delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] static extern IntPtr SetProcessDpiAwarenessContext(IntPtr ctx);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] static extern long GetWindowLongPtrW(IntPtr h, int idx);
    [DllImport("user32.dll")] static extern long SetWindowLongPtrW(IntPtr h, int idx, long val);
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint flags);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern IntPtr MonitorFromWindow(IntPtr h, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool GetMonitorInfoW(IntPtr mon, ref MONITORINFO mi);

    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] struct MONITORINFO { public int cbSize; public RECT rcMonitor, rcWork; public uint dwFlags; }

    public static void EnableDpiAwareness() => SetProcessDpiAwarenessContext(new IntPtr(-4)); // PER_MONITOR_AWARE_V2

    // ---- find the VLC player window ----
    public static IntPtr FindPlayer()
    {
        var vlcPids = System.Diagnostics.Process.GetProcessesByName("vlc").Select(p => (uint)p.Id).ToHashSet();
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

    public static bool InPip()
    {
        var s = PipState.Load(PipState.StatePath);
        if (s is null) return false;
        if (!IsWindow(new IntPtr(s.Hwnd))) { File.Delete(PipState.StatePath); return false; } // stale
        return true;
    }

    public static bool Enter(IntPtr h, PipOptions o)
    {
        if (h == IntPtr.Zero || InPip()) return false;
        GetWindowRect(h, out var r);
        long style = GetWindowLongPtrW(h, GWL_STYLE);
        long ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        PipState.Save(new PipState(h.ToInt64(), r.Left, r.Top, r.Right - r.Left, r.Bottom - r.Top, style, ex), PipState.StatePath);

        SetWindowLongPtrW(h, GWL_STYLE, style & ~(WS_CAPTION | WS_THICKFRAME));
        var mi = new MONITORINFO { cbSize = Marshal.SizeOf<MONITORINFO>() };
        GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), ref mi);
        var (x, y) = PipGeometry.ComputeCorner(mi.rcWork.Left, mi.rcWork.Top, mi.rcWork.Right, mi.rcWork.Bottom, o.W, o.H, o.Corner, o.Margin);
        return SetWindowPos(h, HWND_TOPMOST, x, y, o.W, o.H, SWP_FRAMECHANGED | SWP_SHOWWINDOW);
    }

    public static bool Exit()
    {
        var s = PipState.Load(PipState.StatePath);
        if (s is null) return false;
        var h = new IntPtr(s.Hwnd);
        if (!IsWindow(h)) { File.Delete(PipState.StatePath); return false; } // VLC gone; clear stale state
        SetWindowLongPtrW(h, GWL_STYLE, s.Style);
        SetWindowLongPtrW(h, GWL_EXSTYLE, s.ExStyle);
        var ok = SetWindowPos(h, HWND_NOTOPMOST, s.X, s.Y, s.W, s.H, SWP_FRAMECHANGED | SWP_SHOWWINDOW);
        File.Delete(PipState.StatePath);
        return ok;
    }

    public static bool Toggle(PipOptions o) => InPip() ? Exit() : Enter(FindPlayer(), o);

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
        return $$"""{"found":true,"hwnd":{{h.ToInt64()}},"x":{{r.Left}},"y":{{r.Top}},"w":{{r.Right - r.Left}},"h":{{r.Bottom - r.Top}},"caption":{{(caption ? "true" : "false")}},"topmost":{{(topmost ? "true" : "false")}},"inPip":{{(InPip() ? "true" : "false")}}}""";
    }
}
