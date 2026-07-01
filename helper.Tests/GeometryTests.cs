using PipHelper;

namespace PipHelper.Tests;

public class GeometryTests
{
    // work area 0,0..1920x1040 (taskbar excluded), 480x270, margin 16
    [Theory]
    [InlineData("br", 1424, 754)]
    [InlineData("bl", 16, 754)]
    [InlineData("tr", 1424, 16)]
    [InlineData("tl", 16, 16)]
    public void ComputeCorner_places_window_inside_work_area(string corner, int ex, int ey)
    {
        var (x, y) = PipGeometry.ComputeCorner(0, 0, 1920, 1040, 480, 270, corner, 16);
        Assert.Equal(ex, x);
        Assert.Equal(ey, y);
    }

    [Fact]
    public void ComputeCorner_unknown_corner_falls_back_to_br()
    {
        var (x, y) = PipGeometry.ComputeCorner(0, 0, 1920, 1040, 480, 270, "zz", 16);
        Assert.Equal(1424, x);
        Assert.Equal(754, y);
    }
}
