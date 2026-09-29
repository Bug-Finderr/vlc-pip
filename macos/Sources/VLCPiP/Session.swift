import AppKit
import os

/// One PiP session. Follows VLC's video window (windowed ↔ fullscreen, moves, display
/// changes), trims letterbox bars, mirrors play/pause and time, and holds the last frame
/// through stops, closed windows and capture interruptions instead of dropping the PiP.
@MainActor
final class Session {
    var onEnd: (() -> Void)?

    private enum Phase { case starting, running, ended }

    private let vlc: VLC
    private let capture = Capture()
    private let presenter = Presenter()
    /// True while VLC has nothing loaded: frames are dropped so the PiP keeps the last picture.
    private let holding = OSAllocatedUnfairLock(initialState: false)
    private let checkpoints = Checkpoints()
    private var phase = Phase.starting
    private var window: VLCWindow?
    private var letterbox = Letterbox()
    private var policy = CropPolicy()  // crop within the video area, normalized
    private var fileAspect: Double?    // the playing file's real shape, when it can be read
    private var aspectPath = ""
    private var probeTicks = 0
    private var cropGeneration = 0  // bumps on every reset, so an in-flight still can't leak into a new one
    private var settleUntil = Date.distantPast  // no stills while VLC is still animating
    private var status = VLC.Status()
    private var timers: [Timer] = []
    private var polling = false
    private var probing = false
    private var reconciling = false
    private var reconcileAgain = false
    private var appliedRect: CGRect?

    init(vlc: VLC) { self.vlc = vlc }

    func start(systemPiP: Bool) async throws {
        guard VLC.app != nil else { throw PiPError.vlcNotRunning }
        guard CGPreflightScreenCaptureAccess() else {
            CGRequestScreenCaptureAccess()
            throw PiPError.screenRecordingDenied
        }
        guard await vlc.ensurePermission() else { throw PiPError.automationDenied }
        switch await vlc.status() {
        case .success(let s): status = s
        case .failure(.notRunning): throw PiPError.vlcNotRunning
        case .failure(.notPermitted): throw PiPError.automationDenied
        case .failure(.failed): break
        }
        guard phase == .starting else { return }
        guard !status.name.isEmpty else { throw PiPError.nothingPlaying }
        guard let app = VLC.app,
              let found = WindowLocator.locate(pid: app.processIdentifier, title: status.name, fullscreen: status.fullscreen)
        else { throw PiPError.noWindow }
        aspectPath = status.path
        fileAspect = await MediaInfo.displayAspect(ofFileAt: status.path)
        guard phase == .starting else { return }
        window = found
        resetCrop()  // with the file's shape known, the very first frame is already trimmed

        wireCallbacks()
        let rect = sourceRect(in: found)
        try await capture.start(window: found, sourceRect: rect)
        appliedRect = rect
        guard phase == .starting else {
            await capture.stop()
            return
        }
        phase = .running

        presenter.show(systemPiP: systemPiP)
        presenter.update(playing: status.playing, time: status.time, duration: status.duration)
        schedule(0.5) { $0.pollStatus() }
        schedule(1.0) { $0.followWindow() }
        schedule(0.5) { $0.probeLetterbox() }
        probeLetterbox()
    }

    func stop() {
        guard phase != .ended else { return }
        phase = .ended
        timers.forEach { $0.invalidate() }
        timers = []
        presenter.teardown()
        let capture = self.capture
        Task { await capture.stop() }
        onEnd?()
    }

    private func wireCallbacks() {
        presenter.onSetPlaying = { [weak self] playing in self?.setPlaying(playing) }
        presenter.onSkip = { [weak self] seconds in self?.skip(seconds) }
        presenter.onRestore = { VLC.app?.activate() }
        presenter.onClosed = { [weak self] in self?.stop() }

        let presenter = self.presenter, holding = self.holding, checkpoints = self.checkpoints
        var lastSize = CGSize.zero  // touched only on the capture queue
        capture.onFrame = { [weak self] sb in
            if holding.withLock({ $0 }) { return }
            presenter.enqueue(sb)
            guard let buffer = sb.imageBuffer else { return }
            checkpoints.offer(buffer)
            let size = CGSize(width: CVPixelBufferGetWidth(buffer), height: CVPixelBufferGetHeight(buffer))
            guard size != lastSize else { return }
            lastSize = size
            DispatchQueue.main.async { self?.presenter.frameArrived(size: size) }
        }
        // Window closed, display sleep, etc.: forget the window so the next tick re-finds it.
        capture.onStop = { [weak self] in
            self?.window = nil
            self?.appliedRect = nil
        }
    }

    private func schedule(_ interval: TimeInterval, _ body: @escaping (Session) -> Void) {
        let timer = Timer(timeInterval: interval, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.phase == .running else { return }
                body(self)
            }
        }
        RunLoop.main.add(timer, forMode: .common)  // keep ticking during menus and live resize
        timers.append(timer)
    }

    // MARK: VLC state

    private func pollStatus() {
        guard !polling else { return }
        polling = true
        Task {
            let result = await vlc.status()
            polling = false
            guard phase == .running else { return }
            switch result {
            case .failure(.notRunning): stop()
            case .failure: break  // transient (VLC busy) or permission revoked: keep the last state
            case .success(let s):
                let itemChanged = s.name != status.name || s.path != status.path
                let fullscreenChanged = s.fullscreen != status.fullscreen
                status = s
                let hold = s.name.isEmpty
                let startsHolding = holding.withLock { was -> Bool in
                    defer { was = hold }
                    return hold && !was
                }
                if startsHolding, let still = checkpoints.safeFrame() {
                    presenter.enqueueStill(still)  // cover the playlist frames VLC showed on stop
                }
                if hold { checkpoints.clear() } else { checkpoints.request() }
                presenter.update(playing: s.playing, time: s.time, duration: s.duration)
                if itemChanged, !s.path.isEmpty { loadFileAspect(for: s.path) }  // stopped: keep the last shape
                if itemChanged || fullscreenChanged { followWindow() }
            }
        }
    }

    private func setPlaying(_ playing: Bool) {
        guard phase == .running, status.playing != playing else { return }
        vlc.togglePlay()
        status.playing = playing
        presenter.update(playing: playing, time: status.time, duration: status.duration)
    }

    private func skip(_ seconds: Double) {
        Task {
            guard case .success(var s) = await vlc.status(), phase == .running else { return }
            var target = max(0, s.time + seconds)
            if s.duration > 0 { target = min(target, max(0, s.duration - 1)) }
            vlc.seek(to: target)
            s.time = target
            status = s
            presenter.update(playing: s.playing, time: s.time, duration: s.duration)
        }
    }

    // MARK: Window tracking

    private func followWindow() {
        guard phase == .running else { return }
        guard let app = VLC.app else { return stop() }
        // Nothing loaded, or the window is gone for now: hold the last frame and keep looking.
        if !status.name.isEmpty,
           let found = WindowLocator.locate(pid: app.processIdentifier, title: status.name, fullscreen: status.fullscreen),
           found != window {
            window = found
            resetCrop()
        }
        reconcile()
    }

    /// Brings the capture in line with the wanted window and crop. Serialized, re-runs when
    /// the target moved while a change was in flight, and runs every tick, so a transition
    /// seen half-way (VLC animating to or from fullscreen) always converges.
    private func reconcile() {
        guard phase == .running else { return }
        if reconciling {
            reconcileAgain = true
            return
        }
        guard let target = window else { return }
        let rect = sourceRect(in: target)
        let restart = !capture.isRunning || capture.windowID != target.id || capture.windowScale != target.scale
        guard restart || rect != appliedRect else { return }
        reconciling = true
        Task {
            if restart {
                do {
                    try await capture.start(window: target, sourceRect: rect)
                    appliedRect = rect
                } catch PiPError.screenRecordingDenied {
                    stop()
                } catch {
                    NSLog("vlc-pip: could not capture VLC's window yet: %@", error.localizedDescription)
                    appliedRect = nil
                    if window == target { window = nil }  // re-locate next tick
                }
            } else {
                appliedRect = await capture.update(sourceRect: rect) ? rect : nil
            }
            if phase != .running { await capture.stop() }
            reconciling = false
            if reconcileAgain {
                reconcileAgain = false
                reconcile()
            }
        }
    }

    // MARK: Letterbox

    private func loadFileAspect(for path: String) {
        aspectPath = path
        Task {
            let aspect = await MediaInfo.displayAspect(ofFileAt: path)
            guard phase == .running, aspectPath == path else { return }
            fileAspect = aspect
            resetCrop()
            applyCrop()
        }
    }

    /// Bars depend on the video and the window geometry: start over when either changes.
    private func resetCrop() {
        letterbox.reset()
        policy.reset(view: window?.videoArea.size ?? .zero, fileAspect: fileAspect)
        probeTicks = 0
        cropGeneration += 1
        settleUntil = Date().addingTimeInterval(1.5)
    }

    /// Every 0.5s while the bars are being learned, then every 5s once the running maximum
    /// has settled (it can only reveal more picture from then on).
    private func probeLetterbox() {
        probeTicks += 1
        guard letterbox.samples < 40 || probeTicks % 10 == 0 else { return }
        guard !probing, Date() >= settleUntil, let target = window, capture.isRunning, !holding.withLock({ $0 }) else { return }
        probing = true
        let generation = cropGeneration
        Task {
            defer { probing = false }
            guard let image = await capture.snapshot(of: target.videoArea, inWindowOf: target.size, width: Letterbox.sampleWidth),
                  phase == .running, window == target, cropGeneration == generation,
                  let detected = letterbox.add(image),
                  policy.feed(detected, samples: letterbox.samples) != nil else { return }
            applyCrop()
        }
    }

    private func applyCrop() { reconcile() }

    /// The current crop as window-relative points.
    private func sourceRect(in window: VLCWindow) -> CGRect {
        let area = window.videoArea, crop = policy.crop
        let rect = CGRect(x: area.minX + crop.minX * area.width, y: area.minY + crop.minY * area.height,
                          width: crop.width * area.width, height: crop.height * area.height).integral.intersection(area)
        return rect.width >= 16 && rect.height >= 16 ? rect : area
    }
}
