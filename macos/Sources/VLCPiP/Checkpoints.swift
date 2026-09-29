import CoreVideo
import Foundation

/// Copies of recent frames taken while VLC verifiably had a video loaded, so a stop can
/// fall back to a real picture. VLC swaps its window to the playlist the moment playback
/// stops, and those frames reach the PiP before the next status poll notices; the copy
/// taken one poll *earlier* predates the stop.
final class Checkpoints: @unchecked Sendable {
    private let lock = NSLock()
    private var wanted = false
    private var older: CVPixelBuffer?
    private var newer: CVPixelBuffer?

    /// Main actor, after each poll that saw a loaded item: copy the next frame.
    func request() {
        lock.lock(); defer { lock.unlock() }
        wanted = true
    }

    /// Capture queue, every frame.
    func offer(_ buffer: CVPixelBuffer) {
        lock.lock()
        let take = wanted
        wanted = false
        lock.unlock()
        guard take, let copy = Self.copy(buffer) else { return }
        lock.lock(); defer { lock.unlock() }
        older = newer
        newer = copy
    }

    /// A frame from before the most recent poll; nil until two polls have passed.
    func safeFrame() -> CVPixelBuffer? {
        lock.lock(); defer { lock.unlock() }
        return older
    }

    func clear() {
        lock.lock(); defer { lock.unlock() }
        wanted = false
        older = nil
        newer = nil
    }

    private static func copy(_ src: CVPixelBuffer) -> CVPixelBuffer? {
        let w = CVPixelBufferGetWidth(src), h = CVPixelBufferGetHeight(src)
        let attrs: [CFString: Any] = [kCVPixelBufferIOSurfacePropertiesKey: [String: Any]()]
        var out: CVPixelBuffer?
        guard CVPixelBufferCreate(nil, w, h, CVPixelBufferGetPixelFormatType(src), attrs as CFDictionary, &out) == kCVReturnSuccess,
              let dst = out else { return nil }
        CVPixelBufferLockBaseAddress(src, .readOnly)
        CVPixelBufferLockBaseAddress(dst, [])
        defer {
            CVPixelBufferUnlockBaseAddress(dst, [])
            CVPixelBufferUnlockBaseAddress(src, .readOnly)
        }
        guard let s = CVPixelBufferGetBaseAddress(src), let d = CVPixelBufferGetBaseAddress(dst) else { return nil }
        let sRow = CVPixelBufferGetBytesPerRow(src), dRow = CVPixelBufferGetBytesPerRow(dst)
        for y in 0..<h { memcpy(d + y * dRow, s + y * sRow, min(sRow, dRow)) }
        return dst
    }
}
