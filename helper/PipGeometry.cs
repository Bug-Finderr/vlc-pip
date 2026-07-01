namespace PipHelper;

public static class PipGeometry
{
    public static (int X, int Y) ComputeCorner(
        int workLeft, int workTop, int workRight, int workBottom,
        int w, int h, string corner, int margin)
    {
        int left = workLeft + margin;
        int top = workTop + margin;
        int right = workRight - w - margin;
        int bottom = workBottom - h - margin;
        return corner switch
        {
            "tl" => (left, top),
            "tr" => (right, top),
            "bl" => (left, bottom),
            _ => (right, bottom), // "br" and fallback
        };
    }
}
