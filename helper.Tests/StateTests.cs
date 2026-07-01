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
}
