import CoreMedia
import ScreenCaptureKit
import os

enum PiPError: LocalizedError {
    case vlcNotRunning
    case nothingPlaying
    case noWindow
    case screenRecordingDenied
    case automationDenied
    case captureFailed(String)

    var errorDescription: String? {
        switch self {
        case .vlcNotRunning: "VLC isn't running."
        case .nothingPlaying: "Nothing is loaded in VLC. Open a video first."
        case .noWindow: "Couldn't find VLC's video window."
        case .screenRecordingDenied:
            "VLC PiP needs Screen Recording permission to show VLC's video. Turn it on in System Settings, then reopen VLC PiP."
        case .automationDenied:
            "VLC PiP needs permission to control VLC (for play/pause and seeking). Turn on VLC under VLC PiP in System Settings → Privacy & Security → Automation."
        case .captureFailed(let reason): "Couldn't capture VLC's window: \(reason)"
        }
    }

    var settingsURL: URL? {
        switch self {
        case .screenRecordingDenied:
            URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        case .automationDenied:
            URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation")
        default: nil
        }
    }
}

/// Streams one region of a VLC window with ScreenCaptureKit. Desktop-independent window
/// capture reads the window's own backing store, so it keeps working while the window is
/// covered or on another Space. All state lives on the main actor; frames arrive on a
/// private queue through `Output`, which drops anything from a stream no longer current.
@MainActor
final class Capture {
    /// Called on the capture queue with each complete frame.
    var onFrame: ((CMSampleBuffer) -> Void)? {
        get { output.onFrame }
        set { output.onFrame = newValue }
    }
    /// Called when the stream dies on its own (window closed, sleep, permission revoked).
    var onStop: (() -> Void)?

    private let output = Output()
    private let queue = DispatchQueue(label: "vlc-pip.capture", qos: .userInteractive)
    private var stream: SCStream?
    private var filter: SCContentFilter?
    private var scale: CGFloat = 2
    private(set) var windowID: CGWindowID = 0
    private(set) var windowScale: CGFloat = 0

    private static let maxWidth: CGFloat = 1920

    var isRunning: Bool { stream != nil }

    init() {
        output.onStop = { [weak self] dead in
            DispatchQueue.main.async {
                guard let self, self.stream === dead else { return }
                self.clear()
                self.onStop?()
            }
        }
    }

    func start(window: VLCWindow, sourceRect: CGRect) async throws {
        await stop()
        let content: SCShareableContent
        do {
            content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        } catch {
            throw CGPreflightScreenCaptureAccess() ? PiPError.captureFailed(error.localizedDescription) : .screenRecordingDenied
        }
        guard let scWindow = content.windows.first(where: { $0.windowID == window.id }) else { throw PiPError.noWindow }
        let filter = SCContentFilter(desktopIndependentWindow: scWindow)
        scale = window.scale
        let stream = SCStream(filter: filter, configuration: configuration(for: sourceRect), delegate: output)
        // Current before starting, so the first frames aren't dropped as strays.
        self.stream = stream
        self.filter = filter
        windowID = window.id
        windowScale = window.scale
        output.current.withLock { $0 = ObjectIdentifier(stream) }
        do {
            try stream.addStreamOutput(output, type: .screen, sampleHandlerQueue: queue)
            try await stream.startCapture()
        } catch {
            if self.stream === stream { clear() }
            throw CGPreflightScreenCaptureAccess() ? PiPError.captureFailed(error.localizedDescription) : .screenRecordingDenied
        }
    }

    /// False when there is no stream or ScreenCaptureKit refused the new configuration.
    func update(sourceRect: CGRect) async -> Bool {
        guard let stream else { return false }
        do {
            try await stream.updateConfiguration(configuration(for: sourceRect))
            return self.stream === stream
        } catch {
            NSLog("vlc-pip: capture reconfigure failed: %@", error.localizedDescription)
            return false
        }
    }

    func stop() async {
        guard let stream else { return }
        clear()
        try? await stream.stopCapture()
    }

    /// A small still of `rect` (window-relative points) for letterbox analysis, outside the
    /// live stream. The screenshot API ignores `sourceRect` for window filters, so the whole
    /// window is captured and cropped here.
    func snapshot(of rect: CGRect, inWindowOf windowSize: CGSize, width: Int) async -> CGImage? {
        guard let filter, rect.width >= 16, rect.height >= 16, windowSize.width >= rect.width else { return nil }
        let cfg = SCStreamConfiguration()
        cfg.width = max(16, Int((CGFloat(width) * windowSize.width / rect.width).rounded()))
        cfg.height = max(16, Int((CGFloat(cfg.width) * windowSize.height / windowSize.width).rounded()))
        cfg.showsCursor = false
        cfg.ignoreShadowsSingleWindow = true
        guard let image = try? await SCScreenshotManager.captureImage(contentFilter: filter, configuration: cfg) else { return nil }
        let sx = CGFloat(image.width) / windowSize.width, sy = CGFloat(image.height) / windowSize.height
        // Round inward and skip one line per edge: a pixel straddling the title or control
        // bar would otherwise read as bright picture and hide the black bars next to it.
        let x0 = ceil(rect.minX * sx) + (rect.minX > 0 ? 1 : 0), x1 = floor(rect.maxX * sx) - (rect.maxX < windowSize.width ? 1 : 0)
        let y0 = ceil(rect.minY * sy) + (rect.minY > 0 ? 1 : 0), y1 = floor(rect.maxY * sy) - (rect.maxY < windowSize.height ? 1 : 0)
        guard x1 - x0 >= 16, y1 - y0 >= 16 else { return nil }
        return image.cropping(to: CGRect(x: x0, y: y0, width: x1 - x0, height: y1 - y0))
    }

    private func clear() {
        output.current.withLock { $0 = nil }
        stream = nil
        filter = nil
        windowID = 0
        windowScale = 0
    }

    private func configuration(for rect: CGRect) -> SCStreamConfiguration {
        let fit = min(1, Self.maxWidth / max(1, rect.width * scale))
        let cfg = SCStreamConfiguration()
        cfg.sourceRect = rect
        cfg.width = max(2, Int((rect.width * scale * fit).rounded()))
        cfg.height = max(2, Int((rect.height * scale * fit).rounded()))
        cfg.pixelFormat = kCVPixelFormatType_32BGRA
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: 60)
        cfg.queueDepth = 5
        cfg.showsCursor = false
        cfg.ignoreShadowsSingleWindow = true
        return cfg
    }
}

/// ScreenCaptureKit's callback object, off the main actor.
private final class Output: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    let current = OSAllocatedUnfairLock<ObjectIdentifier?>(initialState: nil)
    var onFrame: ((CMSampleBuffer) -> Void)?
    var onStop: ((SCStream) -> Void)?

    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sb.isValid, current.withLock({ $0 }) == ObjectIdentifier(stream),
              let attachments = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let raw = attachments.first?[.status] as? Int,
              SCFrameStatus(rawValue: raw) == .complete else { return }
        onFrame?(sb)
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        NSLog("vlc-pip: capture stopped: %@", error.localizedDescription)
        onStop?(stream)
    }
}
