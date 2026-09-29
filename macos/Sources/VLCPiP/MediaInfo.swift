import AVFoundation

/// The display aspect ratio (width / height, as shown on screen) of a local video file.
/// VLC's scripting interface doesn't expose the video's size, so it is read from the file:
/// Matroska/WebM headers are parsed directly, everything else goes through AVFoundation.
enum MediaInfo {
    private static let cache = Cache()

    static func displayAspect(ofFileAt path: String) async -> Double? {
        guard path.hasPrefix("/") else { return nil }  // network streams have no file
        if let hit = await cache.get(path) { return hit.value }
        let ext = (path as NSString).pathExtension.lowercased()
        var aspect: Double?
        if ["mkv", "webm", "mka", "mk3d"].contains(ext) {
            aspect = Matroska.displayAspect(ofFileAt: path)
        }
        if aspect == nil {
            aspect = await avFoundationAspect(URL(fileURLWithPath: path))
        }
        if aspect == nil, !["mkv", "webm", "mka", "mk3d"].contains(ext) {
            aspect = Matroska.displayAspect(ofFileAt: path)  // mislabeled Matroska
        }
        let valid = aspect.flatMap { $0.isFinite && $0 > 0.2 && $0 < 5 ? $0 : nil }
        await cache.set(path, valid)
        return valid
    }

    private static func avFoundationAspect(_ url: URL) async -> Double? {
        let asset = AVURLAsset(url: url)
        guard let track = try? await asset.loadTracks(withMediaType: .video).first,
              let size = try? await track.load(.naturalSize), size.width > 0, size.height > 0 else { return nil }
        var width = Double(size.width), height = Double(size.height)
        if let formats = try? await track.load(.formatDescriptions), let format = formats.first,
           let par = CMFormatDescriptionGetExtension(format, extensionKey: kCMFormatDescriptionExtension_PixelAspectRatio)
               as? [String: Any],
           let h = (par[kCMFormatDescriptionKey_PixelAspectRatioHorizontalSpacing as String] as? NSNumber)?.doubleValue,
           let v = (par[kCMFormatDescriptionKey_PixelAspectRatioVerticalSpacing as String] as? NSNumber)?.doubleValue,
           h > 0, v > 0 {
            width *= h / v
        }
        if let t = try? await track.load(.preferredTransform), abs(t.b) == 1, abs(t.c) == 1 {
            swap(&width, &height)  // rotated 90°
        }
        return width / height
    }

    private actor Cache {
        struct Entry { let value: Double? }
        private var entries: [String: Entry] = [:]
        func get(_ path: String) -> Entry? { entries[path] }
        func set(_ path: String, _ value: Double?) { entries[path] = Entry(value: value) }
    }
}

/// Minimal EBML reader for the first video track's display size.
enum Matroska {
    private static let segment: UInt32 = 0x1853_8067
    private static let tracks: UInt32 = 0x1654_AE6B
    private static let trackEntry: UInt32 = 0xAE
    private static let trackType: UInt32 = 0x83
    private static let video: UInt32 = 0xE0
    private static let cluster: UInt32 = 0x1F43_B675
    private static let pixelWidth: UInt32 = 0xB0
    private static let pixelHeight: UInt32 = 0xBA
    private static let displayWidth: UInt32 = 0x54B0
    private static let displayHeight: UInt32 = 0x54BA
    private static let cropBottom: UInt32 = 0x54AA
    private static let cropTop: UInt32 = 0x54BB
    private static let cropLeft: UInt32 = 0x54CC
    private static let cropRight: UInt32 = 0x54DD

    /// Tracks sit near the start of the file; 16 MB covers even large SeekHeads/attachments.
    private static let readLimit = 16 << 20

    static func displayAspect(ofFileAt path: String) -> Double? {
        guard let handle = FileHandle(forReadingAtPath: path) else { return nil }
        defer { try? handle.close() }
        guard let data = try? handle.read(upToCount: readLimit), data.count > 8 else { return nil }
        let bytes = [UInt8](data)
        guard bytes.starts(with: [0x1A, 0x45, 0xDF, 0xA3]) else { return nil }  // EBML magic
        var aspect: Double?
        walk(bytes, 0..<bytes.count) { id, segmentBody in
            guard id == segment else { return true }
            walk(bytes, segmentBody) { id, body in
                if id == cluster { return false }  // media data: tracks come before it
                if id == tracks {
                    walk(bytes, body) { id, entry in
                        guard id == trackEntry, let a = videoAspect(bytes, entry) else { return true }
                        aspect = a
                        return false
                    }
                }
                return aspect == nil
            }
            return false
        }
        return aspect
    }

    private static func videoAspect(_ bytes: [UInt8], _ entry: Range<Int>) -> Double? {
        var isVideo = false
        var fields: [UInt32: UInt64] = [:]
        walk(bytes, entry) { id, body in
            if id == trackType { isVideo = uint(bytes, body) == 1 }
            if id == video {
                walk(bytes, body) { id, field in
                    fields[id] = uint(bytes, field)
                    return true
                }
            }
            return true
        }
        guard isVideo || !fields.isEmpty else { return nil }
        if let dw = fields[displayWidth], let dh = fields[displayHeight], dw > 0, dh > 0 {
            return Double(dw) / Double(dh)  // pixels or a display aspect ratio: the shape either way
        }
        guard let pw = fields[pixelWidth], let ph = fields[pixelHeight] else { return nil }
        let w = Double(pw) - Double((fields[cropLeft] ?? 0) + (fields[cropRight] ?? 0))
        let h = Double(ph) - Double((fields[cropTop] ?? 0) + (fields[cropBottom] ?? 0))
        return w > 0 && h > 0 ? w / h : nil
    }

    /// Visits each element in `range`; the visitor returns false to stop.
    private static func walk(_ bytes: [UInt8], _ range: Range<Int>, visit: (UInt32, Range<Int>) -> Bool) {
        var i = range.lowerBound
        while i < range.upperBound {
            guard let (id, idLen) = vint(bytes, i, keepMarker: true), idLen <= 4,
                  let (size, sizeLen) = vint(bytes, i + idLen, keepMarker: false) else { return }
            let start = i + idLen + sizeLen
            let unknown = size == (UInt64(1) << (7 * UInt64(sizeLen))) - 1
            let end = unknown ? range.upperBound : min(range.upperBound, start + Int(clamping: size))
            guard start <= end else { return }
            if !visit(UInt32(truncatingIfNeeded: id), start..<end) { return }
            i = end
        }
    }

    private static func vint(_ bytes: [UInt8], _ at: Int, keepMarker: Bool) -> (UInt64, Int)? {
        guard at < bytes.count, bytes[at] != 0 else { return nil }
        let len = bytes[at].leadingZeroBitCount + 1
        guard len <= 8, at + len <= bytes.count else { return nil }
        var value = UInt64(keepMarker ? bytes[at] : bytes[at] & (0xFF >> len))
        for k in 1..<len { value = value << 8 | UInt64(bytes[at + k]) }
        return (value, len)
    }

    private static func uint(_ bytes: [UInt8], _ range: Range<Int>) -> UInt64 {
        guard range.count <= 8 else { return 0 }
        return range.reduce(0) { $0 << 8 | UInt64(bytes[$1]) }
    }
}
