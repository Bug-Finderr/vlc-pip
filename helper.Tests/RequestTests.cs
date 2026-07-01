using PipHelper;

namespace PipHelper.Tests;

public class RequestTests
{
    [Fact]
    public void Consume_reads_command_and_deletes_file()
    {
        var path = Path.Combine(Path.GetTempPath(), $"pip-req-test-{Guid.NewGuid():N}.txt");
        File.WriteAllText(path, "toggle\r\n");
        Assert.Equal("toggle", RequestFile.Consume(path));
        Assert.False(File.Exists(path));
    }

    [Fact]
    public void Consume_missing_file_returns_null()
    {
        Assert.Null(RequestFile.Consume(Path.Combine(Path.GetTempPath(), $"nope-{Guid.NewGuid():N}.txt")));
    }
}
