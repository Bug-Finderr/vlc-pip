using System.Runtime.InteropServices;

namespace PipHelper;

public static class Daemon
{
    // ---- constants ----
    const uint MOD_ALT = 0x1, MOD_CONTROL = 0x2, MOD_NOREPEAT = 0x4000;
    const uint VK_P = 0x50, VK_F = 0x46;
    const uint WM_HOTKEY = 0x0312, WM_TIMER = 0x0113;
    const int WH_KEYBOARD_LL = 13, WH_MOUSE_LL = 14;
    const int WM_KEYDOWN = 0x0100, WM_SYSKEYDOWN = 0x0104, WM_LBUTTONDOWN = 0x0201, WM_LBUTTONUP = 0x0202;
    const uint GA_ROOT = 2;
    const int SM_CXDOUBLECLK = 36, SM_CYDOUBLECLK = 37;

    // ---- P/Invoke ----
    [DllImport("user32.dll")] static extern bool RegisterHotKey(IntPtr hWnd, int id, uint mods, uint vk);
    [DllImport("user32.dll")] static extern bool UnregisterHotKey(IntPtr hWnd, int id);
    [DllImport("user32.dll")] static extern UIntPtr SetTimer(IntPtr hWnd, UIntPtr id, uint elapseMs, IntPtr proc);
    [DllImport("user32.dll")] static extern int GetMessage(out MSG msg, IntPtr hWnd, uint min, uint max);
    [DllImport("user32.dll")] static extern bool TranslateMessage(ref MSG msg);
    [DllImport("user32.dll")] static extern IntPtr DispatchMessage(ref MSG msg);
    [DllImport("user32.dll")] static extern void PostQuitMessage(int code);
    [DllImport("user32.dll")] static extern IntPtr SetWindowsHookExW(int type, HookProc proc, IntPtr mod, uint threadId);
    [DllImport("user32.dll")] static extern bool UnhookWindowsHookEx(IntPtr hook);
    [DllImport("user32.dll")] static extern IntPtr CallNextHookEx(IntPtr hook, int code, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] static extern uint GetDoubleClickTime();
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern IntPtr GetModuleHandleW(string? name);

    delegate IntPtr HookProc(int code, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)] struct POINT { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam, lParam; public uint time; public POINT pt; }
    [StructLayout(LayoutKind.Sequential)] struct KBDLLHOOKSTRUCT { public uint vkCode, scanCode, flags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)] struct MSLLHOOKSTRUCT { public POINT pt; public uint mouseData, flags, time; public IntPtr dwExtraInfo; }

    static string AlivePath => Path.Combine(Path.GetTempPath(), "vlc-pip-daemon.alive");

    // hooks MUST be static fields: a local delegate gets GC'd and crashes the hook
    static HookProc? _kbProc, _mouseProc;
    static IntPtr _kbHook, _mouseHook;
    static PipOptions _options = new();
    static uint _lastAllowedClickTime;
    static POINT _lastAllowedClickPt;
    static bool _swallowNextUp;

    public static int Run(PipOptions o)
    {
        _options = o;
        using var mutex = new Mutex(initiallyOwned: true, "VlcPipDaemon", out var isNew);
        if (!isNew) return 0; // already running

        File.WriteAllText(AlivePath, Environment.ProcessId.ToString());
        RegisterHotKey(IntPtr.Zero, 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_P); // WM_HOTKEY -> thread queue
        SetTimer(IntPtr.Zero, UIntPtr.Zero, 150, IntPtr.Zero);                      // WM_TIMER  -> thread queue

        _kbProc = KeyboardHook;
        _mouseProc = MouseHook;
        var mod = GetModuleHandleW(null);
        _kbHook = SetWindowsHookExW(WH_KEYBOARD_LL, _kbProc, mod, 0);
        _mouseHook = SetWindowsHookExW(WH_MOUSE_LL, _mouseProc, mod, 0);

        try
        {
            while (GetMessage(out var msg, IntPtr.Zero, 0, 0) > 0)
            {
                if (msg.message == WM_HOTKEY) Native.Toggle(_options);
                else if (msg.message == WM_TIMER) { PollRequest(); Native.MaintainRegion(_options); }
                TranslateMessage(ref msg);
                DispatchMessage(ref msg);
            }
        }
        finally
        {
            if (_kbHook != IntPtr.Zero) UnhookWindowsHookEx(_kbHook);
            if (_mouseHook != IntPtr.Zero) UnhookWindowsHookEx(_mouseHook);
            UnregisterHotKey(IntPtr.Zero, 1);
            File.Delete(AlivePath);
        }
        return 0;
    }

    static void PollRequest()
    {
        switch (RequestFile.Consume(RequestFile.RequestPath))
        {
            case "toggle": Native.Toggle(_options); break;
            case "enter": Native.Enter(Native.FindPlayer(), _options); break;
            case "exit": Native.Exit(); break;
            case "stop": PostQuitMessage(0); break;
        }
    }

    static bool VlcIsForeground()
    {
        var s = PipState.Load(PipState.StatePath);
        return s is not null && GetForegroundWindow() == new IntPtr(s.Hwnd);
    }

    static bool OverPipWindow(POINT pt)
    {
        var s = PipState.Load(PipState.StatePath);
        if (s is null) return false;
        return GetAncestor(WindowFromPoint(pt), GA_ROOT) == new IntPtr(s.Hwnd);
    }

    static IntPtr KeyboardHook(int code, IntPtr wParam, IntPtr lParam)
    {
        if (code >= 0 && ((long)wParam == WM_KEYDOWN || (long)wParam == WM_SYSKEYDOWN))
        {
            var k = Marshal.PtrToStructure<KBDLLHOOKSTRUCT>(lParam);
            if (k.vkCode == VK_F && Native.InPip() && VlcIsForeground())
                return new IntPtr(1); // swallow F -> no fullscreen while in PiP
        }
        return CallNextHookEx(_kbHook, code, wParam, lParam);
    }

    // Rate-limit clicks over the PiP window: swallow every button-down within double-click
    // time+rect of the last ALLOWED button-down, so no two clicks the OS actually receives
    // can ever pair into a synthesized WM_LBUTTONDBLCLK. (Swallowing only the 2nd click let
    // a TRIPLE click through: the OS paired clicks 1+3 and VLC fullscreened.)
    static IntPtr MouseHook(int code, IntPtr wParam, IntPtr lParam)
    {
        if (code >= 0)
        {
            if ((long)wParam == WM_LBUTTONDOWN)
            {
                var m = Marshal.PtrToStructure<MSLLHOOKSTRUCT>(lParam);
                if (Native.InPip() && OverPipWindow(m.pt))
                {
                    bool burst = m.time - _lastAllowedClickTime <= GetDoubleClickTime()
                                 && Math.Abs(m.pt.X - _lastAllowedClickPt.X) <= GetSystemMetrics(SM_CXDOUBLECLK)
                                 && Math.Abs(m.pt.Y - _lastAllowedClickPt.Y) <= GetSystemMetrics(SM_CYDOUBLECLK);
                    if (burst) { _swallowNextUp = true; return new IntPtr(1); }
                    _lastAllowedClickTime = m.time;
                    _lastAllowedClickPt = m.pt;
                }
            }
            else if ((long)wParam == WM_LBUTTONUP && _swallowNextUp)
            {
                _swallowNextUp = false; // keep the input stream paired: drop the up of a dropped down
                return new IntPtr(1);
            }
        }
        return CallNextHookEx(_mouseHook, code, wParam, lParam);
    }
}
