using PipHelper;

namespace PipHelper.Tests;

public class StateTests
{
    [Fact]
    public void State_round_trips_via_file()
    {
        var path = Path.Combine(Path.GetTempPath(), $"pip-state-test-{Guid.NewGuid():N}.json");
        var s = new PipState(0x1234, 100, 200, 1000, 640, 0x14CF0000, 0x100);
        try
        {
            PipState.Save(s, path);
            var loaded = PipState.Load(path);
            Assert.Equal(s, loaded);
        }
        finally { File.Delete(path); }
    }

    [Fact]
    public void Load_missing_file_returns_null()
    {
        Assert.Null(PipState.Load(Path.Combine(Path.GetTempPath(), $"nope-{Guid.NewGuid():N}.json")));
    }

    [Fact]
    public void Old_format_state_file_loads_with_defaults()
    {
        // pre-options/pre-Pid schema: missing fields must come from constructor defaults
        // (source-gen JSON) so an in-PiP state survives a helper upgrade; Pid=0 reads as stale
        var path = Path.Combine(Path.GetTempPath(), $"pip-state-test-{Guid.NewGuid():N}.json");
        File.WriteAllText(path, """{"Hwnd":4660,"X":100,"Y":200,"W":1000,"H":640,"Style":349110272,"ExStyle":256}""");
        try
        {
            var s = PipState.Load(path);
            Assert.NotNull(s);
            Assert.Equal(4660, s!.Hwnd);
            Assert.Equal(1000, s.W);
            Assert.Equal(480, s.TargetW);
            Assert.Equal(270, s.TargetH);
            Assert.Equal("br", s.Corner);
            Assert.Equal(16, s.Margin);
            Assert.True(s.Min);
            Assert.Equal(0u, s.Pid);
        }
        finally { File.Delete(path); }
    }
}
