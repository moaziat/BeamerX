// Encode a list of frames into an H.264 MP4 with AVFoundation (no ffmpeg).
//
// usage: swift scripts/frames-to-mp4.swift MANIFEST OUTPUT.mp4 [WIDTH]
//
// MANIFEST has one line per frame: `path<TAB>seconds[<TAB>caption]`; paths
// are relative to the manifest's directory. `file.pdf#N` is page N of a PDF
// and `file.pdf#*` expands to every page (each shown for `seconds`; `%d` in
// the caption becomes the page number). Frames are scaled to fit WIDTH
// (default 1920) on a dark background; the height follows the aspect ratio
// of the raster frames (the middle one).

import AVFoundation
import AppKit
import CoreVideo
import PDFKit

struct Entry {
    let path: String
    let seconds: Double
    let caption: String?
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

let args = CommandLine.arguments
guard args.count >= 3 else { fail("usage: frames-to-mp4.swift MANIFEST OUTPUT.mp4 [WIDTH]") }
let manifestURL = URL(fileURLWithPath: args[1])
let outputURL = URL(fileURLWithPath: args[2])
let width = args.count > 3 ? Int(args[3]) ?? 1920 : 1920
let baseDir = manifestURL.deletingLastPathComponent()

guard let manifest = try? String(contentsOf: manifestURL, encoding: .utf8) else {
    fail("cannot read \(manifestURL.path)")
}
var entries: [Entry] = []
for line in manifest.split(separator: "\n") {
    let cols = line.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
    guard cols.count >= 2, let secs = Double(cols[1]) else { continue }
    entries.append(Entry(path: cols[0], seconds: secs, caption: cols.count > 2 ? cols[2] : nil))
}
guard !entries.isEmpty else { fail("empty manifest") }

/// A source image: a raster file or one PDF page.
enum Source {
    case image(CGImage)
    case pdf(PDFPage)
}

func loadImage(_ url: URL) -> CGImage? {
    guard let src = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
    return CGImageSourceCreateImageAtIndex(src, 0, nil)
}

// Expand entries into concrete sources.
var frames: [(Source, Double, String?)] = []
for e in entries {
    if let hash = e.path.range(of: ".pdf#", options: .backwards) {
        let file = String(e.path[..<hash.lowerBound]) + ".pdf"
        let which = String(e.path[hash.upperBound...])
        let url = file.hasPrefix("/") ? URL(fileURLWithPath: file) : baseDir.appendingPathComponent(file)
        guard let doc = PDFDocument(url: url) else { fail("cannot open \(url.path)") }
        let pages = which == "*" ? Array(0..<doc.pageCount) : [max((Int(which) ?? 1) - 1, 0)]
        for i in pages {
            guard let page = doc.page(at: i) else { continue }
            let caption = e.caption?.replacingOccurrences(of: "%d", with: String(i + 1))
            frames.append((.pdf(page), e.seconds, caption))
        }
    } else {
        let url = e.path.hasPrefix("/") ? URL(fileURLWithPath: e.path) : baseDir.appendingPathComponent(e.path)
        guard let img = loadImage(url) else {
            FileHandle.standardError.write("skipping unreadable \(url.path)\n".data(using: .utf8)!)
            continue
        }
        frames.append((.image(img), e.seconds, e.caption))
    }
}

// Output size from the raster frames.
var aspect = 16.0 / 10.0
let rasters = frames.compactMap { f -> CGImage? in
    if case let .image(img) = f.0 { return img }
    return nil
}
if !rasters.isEmpty {
    let img = rasters[rasters.count / 2]
    aspect = Double(img.width) / Double(img.height)
}
let height = Int((Double(width) / aspect / 2.0).rounded()) * 2

// Write next to the target and rename when complete: an MP4 is only
// playable once its index is written at the very end.
let partialURL = outputURL.deletingLastPathComponent()
    .appendingPathComponent("." + outputURL.deletingPathExtension().lastPathComponent + ".partial.mp4")
try? FileManager.default.removeItem(at: partialURL)
guard let writer = try? AVAssetWriter(outputURL: partialURL, fileType: .mp4) else {
    fail("cannot create \(outputURL.path)")
}
let input = AVAssetWriterInput(
    mediaType: .video,
    outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.h264,
        AVVideoWidthKey: width,
        AVVideoHeightKey: height,
        AVVideoCompressionPropertiesKey: [
            AVVideoAverageBitRateKey: 6_000_000,
            AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
            AVVideoMaxKeyFrameIntervalKey: 60,
        ],
    ])
input.expectsMediaDataInRealTime = false
let adaptor = AVAssetWriterInputPixelBufferAdaptor(
    assetWriterInput: input,
    sourcePixelBufferAttributes: [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        kCVPixelBufferWidthKey as String: width,
        kCVPixelBufferHeightKey as String: height,
    ])
writer.add(input)
guard writer.startWriting() else { fail("writer: \(String(describing: writer.error))") }
writer.startSession(atSourceTime: .zero)

let background = CGColor(red: 0.11, green: 0.11, blue: 0.13, alpha: 1)

func render(_ src: Source, caption: String?, into ctx: CGContext) {
    let W = CGFloat(width), H = CGFloat(height)
    ctx.setFillColor(background)
    ctx.fill(CGRect(x: 0, y: 0, width: W, height: H))
    ctx.interpolationQuality = .high
    switch src {
    case let .image(img):
        let s = min(W / CGFloat(img.width), H / CGFloat(img.height))
        let w = CGFloat(img.width) * s, h = CGFloat(img.height) * s
        ctx.draw(img, in: CGRect(x: (W - w) / 2, y: (H - h) / 2, width: w, height: h))
    case let .pdf(page):
        let box = page.bounds(for: .mediaBox)
        let s = min(W * 0.86 / box.width, H * 0.80 / box.height)
        let w = box.width * s, h = box.height * s
        let rect = CGRect(x: (W - w) / 2, y: (H - h) / 2 + H * 0.04, width: w, height: h)
        ctx.setShadow(offset: CGSize(width: 0, height: -6), blur: 24, color: CGColor(gray: 0, alpha: 0.6))
        ctx.setFillColor(CGColor(gray: 1, alpha: 1))
        ctx.fill(rect)
        ctx.setShadow(offset: .zero, blur: 0, color: nil)
        ctx.saveGState()
        ctx.translateBy(x: rect.minX, y: rect.minY)
        ctx.scaleBy(x: s, y: s)
        ctx.translateBy(x: -box.minX, y: -box.minY)
        page.draw(with: .mediaBox, to: ctx)
        ctx.restoreGState()
    }
    if let caption, !caption.isEmpty {
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: H * 0.026, weight: .medium),
            .foregroundColor: NSColor.white,
        ]
        let text = NSAttributedString(string: caption, attributes: attrs)
        let size = text.size()
        let pill = CGRect(
            x: (W - size.width) / 2 - 22, y: H * 0.035, width: size.width + 44, height: size.height + 18)
        NSColor(white: 0.08, alpha: 0.9).setFill()
        NSBezierPath(roundedRect: pill, xRadius: 14, yRadius: 14).fill()
        text.draw(at: CGPoint(x: pill.minX + 22, y: pill.minY + 9))
        NSGraphicsContext.restoreGraphicsState()
    }
}

var t = 0.0
for (i, (src, secs, caption)) in frames.enumerated() {
    guard let pool = adaptor.pixelBufferPool else { fail("no pixel buffer pool") }
    var pb: CVPixelBuffer?
    CVPixelBufferPoolCreatePixelBuffer(nil, pool, &pb)
    guard let buffer = pb else { fail("no pixel buffer") }
    CVPixelBufferLockBaseAddress(buffer, [])
    guard
        let ctx = CGContext(
            data: CVPixelBufferGetBaseAddress(buffer), width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: CVPixelBufferGetBytesPerRow(buffer), space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
    else { fail("no bitmap context") }
    render(src, caption: caption, into: ctx)
    CVPixelBufferUnlockBaseAddress(buffer, [])
    while !input.isReadyForMoreMediaData { usleep(2000) }
    if !adaptor.append(buffer, withPresentationTime: CMTime(seconds: t, preferredTimescale: 600)) {
        fail("append failed at frame \(i): \(String(describing: writer.error))")
    }
    t += secs
}
input.markAsFinished()
writer.endSession(atSourceTime: CMTime(seconds: t, preferredTimescale: 600))
let done = DispatchSemaphore(value: 0)
writer.finishWriting { done.signal() }
done.wait()
guard writer.status == .completed else { fail("writer: \(String(describing: writer.error))") }
try? FileManager.default.removeItem(at: outputURL)
do {
    try FileManager.default.moveItem(at: partialURL, to: outputURL)
} catch {
    fail("cannot move \(partialURL.path) to \(outputURL.path): \(error)")
}
print("wrote \(outputURL.path): \(frames.count) frames, \(String(format: "%.1f", t)) s, \(width)x\(height)")
