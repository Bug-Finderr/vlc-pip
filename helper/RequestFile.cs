namespace PipHelper;

public static class RequestFile
{
    public static string RequestPath => Path.Combine(Path.GetTempPath(), "vlc-pip-request.txt");

    public static string? Consume(string path)
    {
        if (!File.Exists(path)) return null;
        try
        {
            var cmd = File.ReadAllText(path).Trim();
            File.Delete(path);
            return cmd.Length == 0 ? null : cmd;
        }
        catch { return null; } // mid-write race: retry on next poll
    }
}
