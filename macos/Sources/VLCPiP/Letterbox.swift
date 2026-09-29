import CoreGraphics

/// Finds the black bars around the picture (VLC's own when the view's shape differs from
/// the video's, or bars encoded in the file).
///
/// Accumulates stills taken since the last geometry change: bars stay black in every
/// frame, while real picture brightens at some point. A pixel counts as picture once it
/// was bright in two separate stills, so a dark scene can't pass for bars and one stray
/// still (say, mid-animation) can't pass for picture.
struct Letterbox {
    static let full = CGRect(x: 0, y: 0, width: 1, height: 1)
    static let sampleWidth = 320

    private static let darkLevel: UInt8 = 24
    /// A row/column is picture when this share of its pixels has ever been bright, which
    /// keeps a subtitle drawn inside a bar from counting as picture.
    private static let pictureShare = 0.75
    private static let samplesBeforeCropping = 3

    private var brightHits: [UInt8] = []  // per pixel: stills in which it was bright
    private var width = 0
    private var height = 0
    private(set) var samples = 0

    mutating func reset() {
        brightHits = []
        samples = 0
    }

    /// Adds a still; returns the picture's rect (normalized, top-left origin) once enough
    /// stills agree, or nil while undecided.
    mutating func add(_ image: CGImage) -> CGRect? {
        guard let (px, w, h) = Self.gray(image) else { return nil }
        if w != width || h != height || brightHits.count != px.count {
            brightHits = [UInt8](repeating: 0, count: px.count)
            width = w
            height = h
            samples = 0
        }
        for i in px.indices where px[i] > Self.darkLevel && brightHits[i] < 255 { brightHits[i] += 1 }
        samples += 1
        guard samples >= Self.samplesBeforeCropping else { return nil }
        return Self.pictureRect(brightHits, width, height)
    }

    static func pictureRect(_ hits: [UInt8], _ w: Int, _ h: Int) -> CGRect? {
        let minHits: UInt8 = 2
        func share(row y: Int, _ cols: Range<Int>) -> Double {
            var n = 0
            for x in cols where hits[y * w + x] >= minHits { n += 1 }
            return Double(n) / Double(max(1, cols.count))
        }
        func share(col x: Int, _ rows: Range<Int>) -> Double {
            var n = 0
            for y in rows where hits[y * w + x] >= minHits { n += 1 }
            return Double(n) / Double(max(1, rows.count))
        }
        func span(_ n: Int, _ isPicture: (Int) -> Bool) -> Range<Int>? {
            guard let a = (0..<n).first(where: isPicture), let b = (0..<n).last(where: isPicture) else { return nil }
            return a..<(b + 1)
        }

        // Rows and columns are judged within each other's span (a letterboxed column is only
        // partly picture), seeded leniently and then refined with the strict share.
        var rows = 0..<h, cols = 0..<w
        for threshold in [0.25, pictureShare, pictureShare] {
            guard let r = span(h, { share(row: $0, cols) >= threshold }),
                  let c = span(w, { share(col: $0, r) >= threshold }) else { return nil }
            rows = r
            cols = c
        }
        let firstRow = rows.lowerBound, lastRow = rows.upperBound - 1
        let firstCol = cols.lowerBound, lastCol = cols.upperBound - 1

        // VLC centers the picture, so bars are symmetric: trim the thinner side on both.
        let v = trim(min(firstRow, h - 1 - lastRow), of: h)
        let hz = trim(min(firstCol, w - 1 - lastCol), of: w)
        let cw = w - 2 * hz, ch = h - 2 * v
        // Real bars never leave less than ~45% (2.76:1 on 16:10 keeps 58%); anything
        // smaller is a dark scene still being learned.
        guard cw * 100 >= w * 45, ch * 100 >= h * 45 else { return nil }
        return CGRect(x: Double(hz) / Double(w), y: Double(v) / Double(h),
                      width: Double(cw) / Double(w), height: Double(ch) / Double(h))
    }

    /// Hairline edges are noise; a real bar is trimmed one extra line for the blended boundary.
    private static func trim(_ bar: Int, of n: Int) -> Int {
        bar * 100 < n ? 0 : bar + 1
    }

    private static func gray(_ image: CGImage) -> ([UInt8], Int, Int)? {
        let w = sampleWidth
        let h = max(16, Int((Double(w) * Double(image.height) / Double(max(1, image.width))).rounded()))
        var px = [UInt8](repeating: 0, count: w * h)
        let drawn = px.withUnsafeMutableBytes { buf -> Bool in
            guard let ctx = CGContext(data: buf.baseAddress, width: w, height: h, bitsPerComponent: 8,
                                      bytesPerRow: w, space: CGColorSpaceCreateDeviceGray(),
                                      bitmapInfo: CGImageAlphaInfo.none.rawValue) else { return false }
            ctx.interpolationQuality = .medium
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
            return true
        }
        return drawn ? (px, w, h) : nil
    }

    /// Where VLC draws a picture of `aspect` (width/height) inside a view: fitted and centered.
    static func crop(forPictureAspect aspect: Double, inView view: CGSize) -> CGRect {
        guard aspect > 0, view.width > 0, view.height > 0 else { return full }
        let viewAspect = Double(view.width / view.height)
        if aspect >= viewAspect {
            let h = viewAspect / aspect
            return CGRect(x: 0, y: (1 - h) / 2, width: 1, height: h)
        }
        let w = aspect / viewAspect
        return CGRect(x: (1 - w) / 2, y: 0, width: w, height: 1)
    }

    /// Film, TV and phone shapes; a detected picture must be close to one to be trusted.
    private static let commonAspects: [Double] = [
        9.0 / 16, 3.0 / 4, 4.0 / 5, 1, 5.0 / 4, 4.0 / 3, 1.375, 1.43, 1.5, 1.6, 5.0 / 3, 16.0 / 9,
        1.85, 1.9, 2, 2.2, 2.35, 2.39, 2.4, 2.55, 2.76,
    ]

    static func isStandard(_ aspect: Double) -> Bool {
        commonAspects.contains { abs(aspect - $0) / $0 < 0.025 }
    }

    static func close(_ a: CGRect, _ b: CGRect, tolerance: Double = 0.005) -> Bool {
        abs(a.minX - b.minX) < tolerance && abs(a.minY - b.minY) < tolerance
            && abs(a.width - b.width) < tolerance && abs(a.height - b.height) < tolerance
    }
}

/// Decides the crop. With the file's real aspect ratio known, the bars are exact from the
/// first frame; the detector then only corrects it when the screen disagrees: picture
/// where bars should be (an aspect ratio forced in VLC) wins at once, while extra bars
/// (letterboxing encoded in the file) must hold a standard shape for a few seconds.
/// Without a file (streams), the detector alone decides, under the same rules.
struct CropPolicy {
    private(set) var crop = Letterbox.full
    private var view = CGSize(width: 16, height: 9)
    private var knownAspect = false
    private var candidate: CGRect?
    private var votes = 0

    /// Detector rows are ~5pt apart on a large window; differences below this are noise.
    private static let tolerance = 0.02

    mutating func reset(view: CGSize, fileAspect: Double?) {
        self.view = view
        knownAspect = fileAspect != nil
        crop = fileAspect.map { Letterbox.crop(forPictureAspect: $0, inView: view) } ?? Letterbox.full
        candidate = nil
        votes = 0
    }

    /// Feeds one detection made from `samples` accumulated stills; returns the new crop
    /// when it changed.
    mutating func feed(_ detected: CGRect, samples: Int) -> CGRect? {
        let t = Self.tolerance
        if Letterbox.close(detected, crop, tolerance: t) {
            candidate = nil
            votes = 0
            return nil
        }
        let showsMore = detected.minX < crop.minX - t || detected.minY < crop.minY - t
            || detected.maxX > crop.maxX + t || detected.maxY > crop.maxY + t
        if showsMore {
            crop = union(crop, detected)
            candidate = nil
            votes = 0
            return crop
        }
        // Extra bars: trusted only once the running maximum has seen enough footage for false
        // bars from dark scenes to fill in (10s when the file's shape is already known), and
        // only for a standard shape that holds across several stills.
        guard samples >= (knownAspect ? 20 : 6) else { return nil }
        let aspect = Double(detected.width * view.width / max(1, detected.height * view.height))
        guard Letterbox.isStandard(aspect) else { return nil }
        let target = Letterbox.crop(forPictureAspect: aspect, inView: view)
        if let c = candidate, Letterbox.close(c, target, tolerance: 0.01) {
            votes += 1
        } else {
            candidate = target
            votes = 1
        }
        guard votes >= (knownAspect ? 6 : 4) else { return nil }
        crop = target
        candidate = nil
        votes = 0
        return crop
    }

    private func union(_ a: CGRect, _ b: CGRect) -> CGRect {
        // Stay centered: VLC always centers the picture.
        let halfW = max(0.5 - a.minX, a.maxX - 0.5, 0.5 - b.minX, b.maxX - 0.5)
        let halfH = max(0.5 - a.minY, a.maxY - 0.5, 0.5 - b.minY, b.maxY - 0.5)
        let w = min(1, 2 * halfW), h = min(1, 2 * halfH)
        return CGRect(x: (1 - w) / 2, y: (1 - h) / 2, width: w, height: h)
    }
}
