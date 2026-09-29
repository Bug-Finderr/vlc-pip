import AppKit

/// The VLC window that currently shows video, and the part of it that is video.
struct VLCWindow: Equatable {
    var id: CGWindowID
    var size: CGSize       // points
    var videoArea: CGRect  // window-relative points, top-left origin
    var scale: CGFloat     // backing scale of the display the window is on
}

/// VLC 3 on macOS (measured on 3.0.23):
/// - Windowed, the video view lives in an untitled borderless window glued over the
///   player window's content, between a 28pt title bar and a 36pt control bar.
/// - Non-native fullscreen moves that untitled window to cover the screen, while the
///   titled player window stays behind with nothing in it.
/// - Native fullscreen (a VLC preference) makes the titled window itself fullscreen,
///   below the notch on notched displays.
enum WindowLocator {
    private static let ignoredTitles: Set<String> = ["Fullscreen Controls"]
    private static let titleBar: CGFloat = 28
    private static let controlBar: CGFloat = 36

    private struct Info {
        let id: CGWindowID
        let frame: CGRect  // global points, top-left origin
        let name: String
        var area: CGFloat { frame.width * frame.height }
    }

    static func locate(pid: pid_t, title: String, fullscreen: Bool) -> VLCWindow? {
        guard let list = CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] else { return nil }

        let windows: [Info] = list.compactMap { d in
            guard (d[kCGWindowOwnerPID as String] as? pid_t) == pid,
                  let id = d[kCGWindowNumber as String] as? CGWindowID,
                  let bounds = d[kCGWindowBounds as String] as? NSDictionary,
                  let frame = CGRect(dictionaryRepresentation: bounds) else { return nil }
            return Info(id: id, frame: frame, name: d[kCGWindowName as String] as? String ?? "")
        }

        if fullscreen, let fs = windows.filter({ $0.name.isEmpty && coversScreen($0.frame.size) }).max(by: { $0.area < $1.area }) {
            return make(fs, videoArea: CGRect(origin: .zero, size: fs.frame.size))
        }

        // The player window is titled with the playing item; fall back to the largest titled window.
        let titled = windows.filter {
            !$0.name.isEmpty && !ignoredTitles.contains($0.name) && $0.frame.width >= 160 && $0.frame.height >= 90
        }
        guard let main = titled.first(where: { $0.name == title }) ?? titled.max(by: { $0.area < $1.area })
        else { return nil }
        return make(main, videoArea: videoArea(of: main, among: windows))
    }

    private static func make(_ w: Info, videoArea: CGRect) -> VLCWindow {
        VLCWindow(id: w.id, size: w.frame.size, videoArea: videoArea.integral, scale: scale(for: w.frame))
    }

    private static func videoArea(of main: Info, among windows: [Info]) -> CGRect {
        let full = CGRect(origin: .zero, size: main.frame.size)
        if coversScreen(main.frame.size) { return full }

        if let view = windows.first(where: { w in
            w.name.isEmpty && w.id != main.id
                && abs(w.frame.minX - main.frame.minX) < 2 && abs(w.frame.width - main.frame.width) < 2
                && w.frame.minY >= main.frame.minY - 1 && w.frame.maxY <= main.frame.maxY + 1
                && w.frame.height >= main.frame.height * 0.5 && w.frame.height < main.frame.height
        }) {
            return view.frame.offsetBy(dx: -main.frame.minX, dy: -main.frame.minY).intersection(full)
        }

        let area = CGRect(x: 0, y: titleBar, width: full.width, height: full.height - titleBar - controlBar)
        return area.height > 0 ? area : full
    }

    /// Full screen size, or the area below the notch on notched displays.
    private static func coversScreen(_ size: CGSize) -> Bool {
        NSScreen.screens.contains { s in
            let f = s.frame.size
            return abs(size.width - f.width) < 2
                && (abs(size.height - f.height) < 2 || abs(size.height - (f.height - s.safeAreaInsets.top)) < 2)
        }
    }

    private static func scale(for cgFrame: CGRect) -> CGFloat {
        guard let primary = NSScreen.screens.first?.frame else { return 2 }
        // CG global coordinates are top-left based on the primary display; AppKit's are bottom-left.
        let frame = CGRect(x: cgFrame.minX, y: primary.maxY - cgFrame.maxY, width: cgFrame.width, height: cgFrame.height)
        let best = NSScreen.screens.max { a, b in
            let ia = a.frame.intersection(frame), ib = b.frame.intersection(frame)
            return ia.width * ia.height < ib.width * ib.height
        }
        return best?.backingScaleFactor ?? 2
    }
}
