using System.Text.Json;
using System.Text.Json.Serialization;

namespace PipHelper;

// Target*/Corner/Margin/Min are the options in effect at Enter, so the daemon and one-shot
// CLI converge on the same geometry instead of fighting with different defaults. Pid guards
// against HWND recycling. New fields are appended with defaults: an old-format state file
// deserializes fine (Pid=0 reads as stale, forcing one re-toggle after upgrade).
public record PipState(long Hwnd, int X, int Y, int W, int H, long Style, long ExStyle,
    int TargetW = 480, int TargetH = 270, string Corner = "br", int Margin = 16, bool Min = true, uint Pid = 0)
{
    public static string StatePath => Path.Combine(Path.GetTempPath(), "vlc-pip.json");

    public static void Save(PipState s, string path) =>
        File.WriteAllText(path, JsonSerializer.Serialize(s, PipJsonContext.Default.PipState));

    public static PipState? Load(string path)
    {
        if (!File.Exists(path)) return null;
        try { return JsonSerializer.Deserialize(File.ReadAllText(path), PipJsonContext.Default.PipState); }
        catch { return null; } // torn/corrupt state file reads as "not in PiP"
    }
}

// source-generated (reflection JSON breaks under PublishTrimmed)
[JsonSerializable(typeof(PipState))]
internal partial class PipJsonContext : JsonSerializerContext;
