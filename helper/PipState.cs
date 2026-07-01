using System.Text.Json;

namespace PipHelper;

public record PipState(long Hwnd, int X, int Y, int W, int H, long Style, long ExStyle)
{
    public static string StatePath => Path.Combine(Path.GetTempPath(), "vlc-pip.json");

    public static void Save(PipState s, string path) =>
        File.WriteAllText(path, JsonSerializer.Serialize(s));

    public static PipState? Load(string path)
    {
        if (!File.Exists(path)) return null;
        try { return JsonSerializer.Deserialize<PipState>(File.ReadAllText(path)); }
        catch { return null; }
    }
}
