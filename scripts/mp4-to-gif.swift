// Convert a screen-recording MP4 into a small, endlessly looping GIF for the
// README (GitHub does not autoplay or loop videos in Markdown).
//
//   swiftc -O scripts/mp4-to-gif.swift -o /tmp/mp4-to-gif
//   /tmp/mp4-to-gif INPUT.mp4 OUTPUT.gif [WIDTH=960] [FPS=10]
//
// Frames are decoded with AVFoundation and downscaled with Core Image. One
// global 255-colour palette is built from the popular colours of sampled
// frames (spread out so that rare accent colours survive). Each frame stores
// only the rectangle that changed since the previous one, with unchanged
// pixels transparent, which keeps UI recordings small. Pixels whose colour
// moved less than a small tolerance keep their previous index, so video
// compression noise does not count as change. LZW coding is implemented here
// (no third-party packages).

import AVFoundation
import CoreImage
import Foundation

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

let args = CommandLine.arguments
guard args.count >= 3 else { fail("usage: mp4-to-gif INPUT.mp4 OUTPUT.gif [WIDTH=960] [FPS=10]") }
let inputURL = URL(fileURLWithPath: args[1])
let outputPath = args[2]
let outWidth = args.count > 3 ? Int(args[3]) ?? 960 : 960
let fps = args.count > 4 ? Double(args[4]) ?? 10 : 10

// ---------------------------------------------------------------------------
// Decoding

let asset = AVURLAsset(url: inputURL)
func loadTrack() -> AVAssetTrack {
    let sem = DispatchSemaphore(value: 0)
    var track: AVAssetTrack?
    Task {
        track = try? await asset.loadTracks(withMediaType: .video).first
        sem.signal()
    }
    sem.wait()
    guard let t = track else { fail("no video track in \(inputURL.path)") }
    return t
}
let track = loadTrack()
let natural: CGSize = {
    let sem = DispatchSemaphore(value: 0)
    var size = CGSize.zero
    Task {
        size = (try? await track.load(.naturalSize)) ?? .zero
        sem.signal()
    }
    sem.wait()
    return size
}()
guard natural.width > 0 else { fail("cannot read the video size") }
let W = outWidth
let H = Int((Double(W) * Double(natural.height) / Double(natural.width)).rounded())
let ciContext = CIContext(options: [.workingColorSpace: NSNull(), .outputColorSpace: NSNull()])

/// Calls `body` with RGBA pixels (W×H) and the time of each sampled frame.
func forEachFrame(step: Double, _ body: (UnsafeMutablePointer<UInt8>, Double) -> Void) {
    guard let reader = try? AVAssetReader(asset: asset) else { fail("cannot read \(inputURL.path)") }
    let output = AVAssetReaderTrackOutput(
        track: track,
        outputSettings: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA])
    output.alwaysCopiesSampleData = false
    reader.add(output)
    guard reader.startReading() else { fail("cannot decode: \(String(describing: reader.error))") }
    let pixels = UnsafeMutablePointer<UInt8>.allocate(capacity: W * H * 4)
    defer { pixels.deallocate() }
    let scale = Double(W) / Double(natural.width)
    var next = 0.0
    while let sample = output.copyNextSampleBuffer() {
        let t = CMSampleBufferGetPresentationTimeStamp(sample).seconds
        guard t + 1e-6 >= next, let buffer = CMSampleBufferGetImageBuffer(sample) else { continue }
        let image = CIImage(cvPixelBuffer: buffer)
            .applyingFilter("CILanczosScaleTransform", parameters: [kCIInputScaleKey: scale, kCIInputAspectRatioKey: 1.0])
        ciContext.render(
            image, toBitmap: pixels, rowBytes: W * 4,
            bounds: CGRect(x: 0, y: 0, width: W, height: H), format: .RGBA8, colorSpace: nil)
        body(pixels, t)
        while next <= t + 1e-6 { next += step }
    }
}

// ---------------------------------------------------------------------------
// Palette: 6 bits per channel histogram over sampled frames.

let bins = 1 << 18
var counts = [Int](repeating: 0, count: bins)
var sums = [SIMD3<Int>](repeating: .zero, count: bins)
func bin(_ r: UInt8, _ g: UInt8, _ b: UInt8) -> Int {
    (Int(r >> 2) << 12) | (Int(g >> 2) << 6) | Int(b >> 2)
}
forEachFrame(step: 0.5) { px, _ in
    for i in stride(from: 0, to: W * H * 4, by: 4) {
        let k = bin(px[i], px[i + 1], px[i + 2])
        counts[k] += 1
        sums[k] &+= SIMD3(Int(px[i]), Int(px[i + 1]), Int(px[i + 2]))
    }
}
let order = (0..<bins).filter { counts[$0] > 0 }.sorted { counts[$0] > counts[$1] }
var palette: [SIMD3<Int>] = []
var threshold = 24 * 24
while palette.count < 255 && threshold >= 1 {
    for k in order where palette.count < 255 {
        let c = sums[k] / counts[k]
        if palette.allSatisfy({ d in
            let e = d &- c
            return e.x * e.x + e.y * e.y + e.z * e.z >= threshold
        }) {
            palette.append(c)
        }
    }
    threshold /= 2
}
while palette.count < 255 { palette.append(.zero) }
let transparent: UInt8 = 255

// Nearest palette index for every 6-bit colour.
var nearest = [UInt8](repeating: 0, count: bins)
for k in 0..<bins {
    let c = SIMD3((k >> 12) << 2 | 2, ((k >> 6) & 63) << 2 | 2, (k & 63) << 2 | 2)
    var best = 0
    var bestD = Int.max
    for (i, p) in palette.enumerated() {
        let e = p &- c
        let d = e.x * e.x + e.y * e.y + e.z * e.z
        if d < bestD {
            bestD = d
            best = i
        }
    }
    nearest[k] = UInt8(best)
}

// ---------------------------------------------------------------------------
// GIF writing

var out = Data()
func u16(_ v: Int) { out.append(UInt8(v & 0xff)); out.append(UInt8((v >> 8) & 0xff)) }
out.append(contentsOf: Array("GIF89a".utf8))
u16(W)
u16(H)
out.append(contentsOf: [0xF7, 0, 0])  // global table of 256 colours
for i in 0..<256 {
    let c = i < palette.count ? palette[i] : .zero
    out.append(contentsOf: [UInt8(clamping: c.x), UInt8(clamping: c.y), UInt8(clamping: c.z)])
}
out.append(contentsOf: [0x21, 0xFF, 0x0B])
out.append(contentsOf: Array("NETSCAPE2.0".utf8))
out.append(contentsOf: [0x03, 0x01, 0x00, 0x00, 0x00])  // loop forever

/// GIF-flavoured LZW (variable code size, LSB-first), as in the classic
/// compress/GIFEncoder scheme.
func lzw(_ data: [UInt8]) -> [UInt8] {
    let minCode = 8
    let clear = 1 << minCode
    let eoi = clear + 1
    var codeSize = minCode + 1
    var next = eoi + 1
    var dict = [Int: Int](minimumCapacity: 8192)
    var bytes: [UInt8] = []
    bytes.reserveCapacity(data.count / 2)
    var acc = 0
    var nbits = 0
    func emit(_ code: Int) {
        acc |= code << nbits
        nbits += codeSize
        while nbits >= 8 {
            bytes.append(UInt8(acc & 0xff))
            acc >>= 8
            nbits -= 8
        }
    }
    emit(clear)
    var prefix = Int(data[0])
    for i in 1..<data.count {
        let b = Int(data[i])
        let key = (prefix << 8) | b
        if let c = dict[key] {
            prefix = c
            continue
        }
        emit(prefix)
        if next >= (1 << codeSize) && codeSize < 12 { codeSize += 1 }
        if next < 4096 {
            dict[key] = next
            next += 1
        } else {
            emit(clear)
            dict.removeAll(keepingCapacity: true)
            next = eoi + 1
            codeSize = minCode + 1
        }
        prefix = b
    }
    emit(prefix)
    if next >= (1 << codeSize) && codeSize < 12 { codeSize += 1 }
    emit(eoi)
    if nbits > 0 { bytes.append(UInt8(acc & 0xff)) }
    return bytes
}

/// A frame waiting for its delay (known when the next frame arrives).
struct Pending {
    var x: Int, y: Int, w: Int, h: Int
    var indices: [UInt8]
    var time: Double
}

func write(_ f: Pending, until: Double) {
    let delay = max(2, Int(((until - f.time) * 100).rounded()))
    out.append(contentsOf: [0x21, 0xF9, 0x04, 0x05])  // do not dispose, transparency on
    u16(delay)
    out.append(contentsOf: [transparent, 0x00])
    out.append(0x2C)
    u16(f.x)
    u16(f.y)
    u16(f.w)
    u16(f.h)
    out.append(0x00)
    out.append(8)
    let packed = lzw(f.indices)
    var i = 0
    while i < packed.count {
        let n = min(255, packed.count - i)
        out.append(UInt8(n))
        out.append(contentsOf: packed[i..<(i + n)])
        i += n
    }
    out.append(0x00)
}

var shown = [UInt8](repeating: 0, count: W * H)  // indices on screen
var first = true
var pending: Pending?
var frames = 0
var lastTime = 0.0
let tolerance = 14 * 14
forEachFrame(step: 1.0 / fps) { px, t in
    lastTime = t
    var current = shown
    var minX = W, minY = H, maxX = -1, maxY = -1
    for y in 0..<H {
        for x in 0..<W {
            let p = y * W + x
            let i = p * 4
            let r = Int(px[i]), g = Int(px[i + 1]), b = Int(px[i + 2])
            if !first {
                let s = palette[Int(shown[p])]
                let dr = s.x - r, dg = s.y - g, db = s.z - b
                if dr * dr + dg * dg + db * db <= tolerance { continue }
            }
            let k = nearest[bin(UInt8(r), UInt8(g), UInt8(b))]
            if first || k != shown[p] {
                current[p] = k
                if x < minX { minX = x }
                if x > maxX { maxX = x }
                if y < minY { minY = y }
                if y > maxY { maxY = y }
            }
        }
    }
    if maxX < 0 { return }  // nothing changed: the previous frame lasts longer
    let w = maxX - minX + 1, h = maxY - minY + 1
    var indices = [UInt8](repeating: transparent, count: w * h)
    for y in 0..<h {
        for x in 0..<w {
            let p = (minY + y) * W + minX + x
            if first || current[p] != shown[p] { indices[y * w + x] = current[p] }
        }
    }
    if let prev = pending { write(prev, until: t) }
    pending = Pending(x: minX, y: minY, w: w, h: h, indices: indices, time: t)
    shown = current
    first = false
    frames += 1
}
if let prev = pending { write(prev, until: max(lastTime + 1.0 / fps, prev.time + 2.0)) }
out.append(0x3B)
do {
    try out.write(to: URL(fileURLWithPath: outputPath), options: .atomic)
} catch {
    fail("cannot write \(outputPath): \(error)")
}
print(String(format: "wrote %@: %dx%d, %d frames, %.1f MB", outputPath, W, H, frames, Double(out.count) / 1_048_576))
