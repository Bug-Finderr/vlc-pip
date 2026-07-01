namespace PipHelper;

public static class Program
{
    public static int Main(string[] args)
    {
        Native.EnableDpiAwareness();
        var mode = args.Length > 0 ? args[0].ToLowerInvariant() : "toggle";
        var o = ParseOptions(args.Skip(1));
        switch (mode)
        {
            case "toggle": return Native.Toggle(o) ? 0 : 1;
            case "enter": return Native.Enter(Native.FindPlayer(), o) ? 0 : 1;
            case "exit": return Native.Exit() ? 0 : 1;
            case "status":
                var status = Native.Status();
                Console.WriteLine(status);                    // visible when stdout is a real pipe
                File.WriteAllText(Native.StatusPath, status); // reliable channel for scripts (WinExe stdout is unreliable)
                return 0;
            case "daemon": return Daemon.Run(o);
            case "stop": File.WriteAllText(RequestFile.RequestPath, "stop"); return 0;
            default: Console.Error.WriteLine($"unknown mode: {mode}"); return 2;
        }
    }

    public static PipOptions ParseOptions(IEnumerable<string> args)
    {
        var o = new PipOptions();
        foreach (var a in args)
        {
            var i = a.IndexOf('=');
            if (i < 1) continue;
            var (k, v) = (a[..i], a[(i + 1)..]);
            o = k switch
            {
                "w" when int.TryParse(v, out var n) => o with { W = n },
                "h" when int.TryParse(v, out var n) => o with { H = n },
                "c" => o with { Corner = v },
                "m" when int.TryParse(v, out var n) => o with { Margin = n },
                _ => o,
            };
        }
        return o;
    }
}
