@preconcurrency import AVKit
import AppKit

/// Thread-safe playback snapshot: AVKit polls the playback delegate synchronously.
final class PlaybackSnapshot: @unchecked Sendable {
    private let lock = NSLock()
    private var playing = false
    private var duration = 0.0

    func set(playing: Bool, duration: Double) {
        lock.lock(); defer { lock.unlock() }
        self.playing = playing
        self.duration = duration
    }

    func read() -> (playing: Bool, duration: Double) {
        lock.lock(); defer { lock.unlock() }
        return (playing, duration)
    }
}

/// Video view of the fallback panel: drag to move, double-click to go back to VLC.
private final class VideoView: NSView {
    var onDoubleClick: (() -> Void)?
    override var mouseDownCanMoveWindow: Bool { true }
    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 { onDoubleClick?() } else { super.mouseDown(with: event) }
    }
}

/// Shows captured frames in system Picture in Picture, which floats over every Space
/// including other apps' fullscreen ones. The source panel (itself pinned above
/// fullscreen apps) is the fallback when system PiP is off or can't start.
@MainActor
final class Presenter: NSObject, NSWindowDelegate {
    var onSetPlaying: ((Bool) -> Void)?
    var onSkip: ((Double) -> Void)?
    var onRestore: (() -> Void)?
    var onClosed: (() -> Void)?

    // The renderer accepts buffers from any thread; the capture queue feeds it directly.
    nonisolated(unsafe) let renderer: AVSampleBufferVideoRenderer
    private let displayLayer = AVSampleBufferDisplayLayer()
    private let panel: NSPanel
    private let snapshot = PlaybackSnapshot()
    private var pip: AVPictureInPictureController?
    private var possibleObservation: NSKeyValueObservation?
    private var resizeObserver: NSObjectProtocol?
    private var timebase: CMTimebase?
    private var lastPlaying: Bool?
    private var lastDuration = 0.0
    private var aspect = CGSize(width: 16, height: 9)
    private var active = false
    private var pipActive = false
    private var wantsSystemPiP = false
    private var hasFrame = false

    private static let panelWidth: CGFloat = 480
    private static let margin: CGFloat = 16

    override init() {
        renderer = displayLayer.sampleBufferRenderer
        panel = NSPanel(contentRect: CGRect(x: 0, y: 0, width: 480, height: 270),
                        styleMask: [.titled, .closable, .resizable, .fullSizeContentView, .nonactivatingPanel],
                        backing: .buffered, defer: false)
        super.init()

        panel.titlebarAppearsTransparent = true
        panel.titleVisibility = .hidden
        panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
        panel.standardWindowButton(.zoomButton)?.isHidden = true
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isMovableByWindowBackground = true
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.backgroundColor = .black
        panel.minSize = CGSize(width: 200, height: 112)
        panel.delegate = self

        let view = VideoView(frame: panel.contentLayoutRect)
        view.wantsLayer = true
        view.layer?.backgroundColor = NSColor.black.cgColor
        displayLayer.frame = view.bounds
        displayLayer.autoresizingMask = [.layerWidthSizable, .layerHeightSizable]
        displayLayer.videoGravity = .resizeAspect
        view.layer?.addSublayer(displayLayer)
        view.onDoubleClick = { [weak self] in
            self?.onRestore?()
            self?.onClosed?()
        }
        panel.contentView = view

        var tb: CMTimebase?
        CMTimebaseCreateWithSourceClock(allocator: kCFAllocatorDefault, sourceClock: CMClockGetHostTimeClock(), timebaseOut: &tb)
        if let tb {
            CMTimebaseSetRate(tb, rate: 0)
            displayLayer.controlTimebase = tb
            timebase = tb
        }
    }

    /// Frames go straight to the renderer from the capture queue.
    nonisolated func enqueue(_ sb: CMSampleBuffer) {
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            let dict = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(dict,
                                 Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        if renderer.status == .failed || renderer.requiresFlushToResumeDecoding { renderer.flush() }
        renderer.enqueue(sb)
    }

    /// Shows a single still (the hold frame when VLC stops).
    nonisolated func enqueueStill(_ buffer: CVPixelBuffer) {
        var format: CMVideoFormatDescription?
        CMVideoFormatDescriptionCreateForImageBuffer(allocator: nil, imageBuffer: buffer, formatDescriptionOut: &format)
        guard let format else { return }
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
                                        decodeTimeStamp: .invalid)
        var sb: CMSampleBuffer?
        CMSampleBufferCreateReadyWithImageBuffer(allocator: nil, imageBuffer: buffer, formatDescription: format,
                                                 sampleTiming: &timing, sampleBufferOut: &sb)
        if let sb { enqueue(sb) }
    }

    func show(systemPiP: Bool) {
        active = true
        wantsSystemPiP = systemPiP && AVPictureInPictureController.isPictureInPictureSupported()
        placePanel()
        panel.orderFrontRegardless()
        if hasFrame { startSystemPiP() }
    }

    /// A captured frame of `size` pixels reached the renderer.
    func frameArrived(size: CGSize) {
        setAspect(size)
        guard !hasFrame else { return }
        hasFrame = true
        if active { startSystemPiP() }
    }

    /// Only once a frame is displayed: AVKit sizes its PiP host view from the video's
    /// dimensions and traps on the NaN geometry an empty layer produces.
    private func startSystemPiP() {
        guard wantsSystemPiP, pip == nil else { return }

        // Live-resizing the PiP window posts these continuously.
        resizeObserver = NotificationCenter.default.addObserver(forName: NSWindow.didResizeNotification, object: nil,
                                                                queue: .main) { [weak self] note in
            MainActor.assumeIsolated {
                guard let self, let w = note.object as? NSWindow, self.isPiPWindow(w) else { return }
                self.followPiPWindow(w)
            }
        }

        let source = AVPictureInPictureController.ContentSource(sampleBufferDisplayLayer: displayLayer, playbackDelegate: self)
        let controller = AVPictureInPictureController(contentSource: source)
        controller.delegate = self
        pip = controller
        possibleObservation = controller.observe(\.isPictureInPicturePossible, options: [.initial, .new]) { [weak self] c, _ in
            guard c.isPictureInPicturePossible else { return }
            DispatchQueue.main.async {
                guard let self, self.active, self.pip === c, !c.isPictureInPictureActive else { return }
                c.startPictureInPicture()
            }
        }
    }

    func teardown() {
        guard active else { return }
        active = false
        possibleObservation = nil
        if let resizeObserver { NotificationCenter.default.removeObserver(resizeObserver) }
        resizeObserver = nil
        pip?.stopPictureInPicture()
        pip = nil
        pipActive = false
        panel.orderOut(nil)
        renderer.flush(removingDisplayedImage: true, completionHandler: nil)
        lastPlaying = nil
    }

    /// The captured picture changed shape (crop, new video, window resize).
    private func setAspect(_ size: CGSize) {
        guard size.width > 0, size.height > 0, size != aspect else { return }
        aspect = size
        panel.contentAspectRatio = size
        guard !pipActive else { return }  // system PiP adopts the new shape itself
        var frame = panel.frame
        let height = (frame.width * size.height / size.width).rounded()
        frame.origin.y += frame.height - height
        frame.size.height = height
        panel.setFrame(frame, display: true)
    }

    func update(playing: Bool, time: Double, duration: Double) {
        snapshot.set(playing: playing, duration: duration)
        if let timebase {
            let current = CMTimebaseGetTime(timebase).seconds
            if playing != lastPlaying || !current.isFinite || abs(current - time) > 1.5 {
                CMTimebaseSetTime(timebase, time: CMTime(seconds: time, preferredTimescale: 600))
            }
            CMTimebaseSetRate(timebase, rate: playing ? 1 : 0)
        }
        if playing != lastPlaying || duration != lastDuration { pip?.invalidatePlaybackState() }
        lastPlaying = playing
        lastDuration = duration
    }

    /// System PiP mirrors the display layer at the layer's own size, and the layer stays in
    /// the hidden source panel. So the source panel follows the PiP window's size, or the
    /// video would sit small in a corner of a larger PiP.
    private func followPiPWindow(_ window: NSWindow? = nil) {
        guard pipActive, let pipWindow = window ?? NSApp.windows.first(where: isPiPWindow),
              let size = pipWindow.contentView?.bounds.size, size.width > 0, size.height > 0,
              panel.contentView?.bounds.size != size else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        panel.setContentSize(size)
        displayLayer.frame = panel.contentView?.bounds ?? CGRect(origin: .zero, size: size)
        CATransaction.commit()
    }

    /// AVKit's PiP window (a private `PIPPanel`) lives in this process.
    private func isPiPWindow(_ w: NSWindow) -> Bool {
        w !== panel && w.isVisible && w.className.contains("PIP")
    }

    private func placePanel() {
        guard let screen = (NSScreen.main ?? NSScreen.screens.first)?.visibleFrame else { return }
        let w = Self.panelWidth, h = (w * aspect.height / aspect.width).rounded()
        panel.contentAspectRatio = aspect
        panel.setFrame(CGRect(x: screen.maxX - w - Self.margin, y: screen.minY + Self.margin, width: w, height: h), display: true)
    }

    // MARK: NSWindowDelegate (fallback panel's close button)

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        onClosed?()
        return false
    }
}

extension Presenter: AVPictureInPictureControllerDelegate {
    nonisolated func pictureInPictureControllerDidStartPictureInPicture(_ c: AVPictureInPictureController) {
        DispatchQueue.main.async {
            guard self.active, self.pip === c else { return }
            self.pipActive = true
            self.panel.orderOut(nil)
            self.followPiPWindow()
        }
    }

    nonisolated func pictureInPictureController(_ c: AVPictureInPictureController, failedToStartPictureInPictureWithError error: Error) {
        NSLog("vlc-pip: system PiP failed, keeping the floating panel: %@", error.localizedDescription)
    }

    nonisolated func pictureInPictureController(_ c: AVPictureInPictureController,
                                                restoreUserInterfaceForPictureInPictureStopWithCompletionHandler done: @escaping (Bool) -> Void) {
        DispatchQueue.main.async { self.onRestore?() }
        done(true)
    }

    nonisolated func pictureInPictureControllerDidStopPictureInPicture(_ c: AVPictureInPictureController) {
        DispatchQueue.main.async {
            guard self.active, self.pip === c else { return }
            self.onClosed?()
        }
    }
}

extension Presenter: AVPictureInPictureSampleBufferPlaybackDelegate {
    nonisolated func pictureInPictureController(_ c: AVPictureInPictureController, setPlaying playing: Bool) {
        DispatchQueue.main.async { self.onSetPlaying?(playing) }
    }

    nonisolated func pictureInPictureControllerTimeRangeForPlayback(_ c: AVPictureInPictureController) -> CMTimeRange {
        let d = snapshot.read().duration
        guard d > 0 else { return CMTimeRange(start: .negativeInfinity, duration: .positiveInfinity) }
        return CMTimeRange(start: .zero, duration: CMTime(seconds: d, preferredTimescale: 600))
    }

    nonisolated func pictureInPictureControllerIsPlaybackPaused(_ c: AVPictureInPictureController) -> Bool {
        !snapshot.read().playing
    }

    nonisolated func pictureInPictureController(_ c: AVPictureInPictureController, didTransitionToRenderSize size: CMVideoDimensions) {
        DispatchQueue.main.async { self.followPiPWindow() }
    }

    nonisolated func pictureInPictureController(_ c: AVPictureInPictureController, skipByInterval interval: CMTime,
                                                completion: @escaping () -> Void) {
        let seconds = interval.seconds
        DispatchQueue.main.async {
            self.onSkip?(seconds)
            completion()
        }
    }
}
