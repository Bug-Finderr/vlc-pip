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
            case "toggle": return OneShot(Native.Toggle(o), o);
            case "enter": return OneShot(Native.Enter(Native.FindPlayer(), o), o);
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

    // one-shot (no daemon ticks): converge the minimal-look region here, sleeps are harmless
    static int OneShot(bool ok, PipOptions o)
    {
        if (ok && Native.InPip())
            for (var i = 0; i < 6; i++) { Thread.Sleep(150); Native.MaintainRegion(o); } // debounce needs ~4 ticks: measure, resize, measure, region
        return ok ? 0 : 1;
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
                "min" => o with { Min = v != "0" && !v.Equals("false", StringComparison.OrdinalIgnoreCase) },
                _ => o,
            };
        }
        return o;
    }
}
