import Foundation

/// 16-bit PCM mono WAV writer. The header is rewritten with real sizes on `finish()`, so a crash leaves a file whose
/// header says 0 bytes but whose samples are intact (the host repairs it on recovery).
public final class WavWriter {
    public let url: URL
    public let sampleRate: Int
    private let handle: FileHandle
    public private(set) var frames: Int = 0
    private var finished = false

    public init(url: URL, sampleRate: Int = 16_000) throws {
        self.url = url
        self.sampleRate = sampleRate
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        guard FileManager.default.createFile(atPath: url.path, contents: WavWriter.header(sampleRate: sampleRate, dataBytes: 0)) else {
            throw ProtocolError("cannot create \(url.path)")
        }
        handle = try FileHandle(forWritingTo: url)
        try handle.seekToEnd()
    }

    public static func header(sampleRate: Int, dataBytes: Int) -> Data {
        var d = Data()
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
        func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
        d.append(contentsOf: Array("RIFF".utf8)); u32(UInt32(36 + dataBytes))
        d.append(contentsOf: Array("WAVE".utf8))
        d.append(contentsOf: Array("fmt ".utf8)); u32(16); u16(1); u16(1)
        u32(UInt32(sampleRate)); u32(UInt32(sampleRate * 2)); u16(2); u16(16)
        d.append(contentsOf: Array("data".utf8)); u32(UInt32(dataBytes))
        return d
    }

    public func append(_ samples: [Float]) throws {
        guard !finished, !samples.isEmpty else { return }
        var pcm = Data(capacity: samples.count * 2)
        for s in samples {
            let v = Int16(max(-1, min(1, s)) * Float(Int16.max))
            withUnsafeBytes(of: v.littleEndian) { pcm.append(contentsOf: $0) }
        }
        try handle.write(contentsOf: pcm)
        frames += samples.count
    }

    public var durationMs: Double { Double(frames) * 1000 / Double(sampleRate) }

    public func finish() throws {
        guard !finished else { return }
        finished = true
        try handle.seek(toOffset: 0)
        try handle.write(contentsOf: WavWriter.header(sampleRate: sampleRate, dataBytes: frames * 2))
        try handle.close()
    }
}

public enum DSP {
    public static func rms(_ x: [Float]) -> Double {
        guard !x.isEmpty else { return 0 }
        var acc: Double = 0
        for v in x { acc += Double(v) * Double(v) }
        return min(1, (acc / Double(x.count)).squareRoot())
    }

    /// Linear-interpolation resampler, stateful across calls so chunk boundaries don't click.
    public final class Resampler {
        public let from: Double, to: Double
        private var pos: Double = 0
        private var last: Float = 0
        public init(from: Double, to: Double) { self.from = from; self.to = to }
        public func process(_ x: [Float]) -> [Float] {
            guard from != to else { return x }
            guard !x.isEmpty else { return [] }
            let step = from / to
            var out: [Float] = []
            out.reserveCapacity(Int(Double(x.count) / step) + 1)
            // pos is relative to x[0]; index -1 is the previous chunk's last sample.
            while pos < Double(x.count - 1) {
                let i = Int(pos.rounded(.down))
                let frac = Float(pos - Double(i))
                let a = i < 0 ? last : x[i]
                let b = x[i + 1]
                out.append(a + (b - a) * frac)
                pos += step
            }
            pos -= Double(x.count)
            last = x[x.count - 1]
            return out
        }
    }

    /// Aligns two 16 kHz mono streams (system + mic) arriving in separate callbacks into CaptureBlocks.
    /// If the mic stalls for more than `maxSkewFrames` (or there is no mic), system audio passes with mic = nil;
    /// if system stalls, the mic passes with silent system audio.
    public final class Aligner: @unchecked Sendable {
        private var a: [Float] = [], b: [Float] = []
        public let maxSkewFrames: Int
        public init(maxSkewFrames: Int = 16_000 / 2) { self.maxSkewFrames = maxSkewFrames }
        public func pushSystem(_ x: [Float]) -> [CaptureBlock] { a += x; return drain() }
        public func pushMic(_ x: [Float]) -> [CaptureBlock] { b += x; return drain() }
        private func drain() -> [CaptureBlock] {
            var out: [CaptureBlock] = []
            let n = min(a.count, b.count)
            if n > 0 {
                out.append(CaptureBlock(system: Array(a[0..<n]), mic: Array(b[0..<n])))
                a.removeFirst(n); b.removeFirst(n)
            }
            if a.count > maxSkewFrames { out.append(CaptureBlock(system: a, mic: nil)); a.removeAll() }
            if b.count > maxSkewFrames { out.append(CaptureBlock(system: [Float](repeating: 0, count: b.count), mic: b)); b.removeAll() }
            return out
        }
    }

    /// Mixes two 16 kHz mono streams (system + mic) that arrive in separate callbacks. Whatever is buffered on both
    /// sides is mixed; if one side stalls for more than `maxSkewFrames`, the other is passed through alone.
    public final class Mixer {
        private var a: [Float] = [], b: [Float] = []
        public let maxSkewFrames: Int
        /// Optional echo suppression applied to the mic side before mixing.
        public var echo: EchoGate?
        public init(maxSkewFrames: Int = 16_000 / 2, echo: EchoGate? = nil) { self.maxSkewFrames = maxSkewFrames; self.echo = echo }
        public func pushSystem(_ x: [Float]) -> [Float] { a += x; return drain() }
        public func pushMic(_ x: [Float]) -> [Float] { b += x; return drain() }
        private func drain() -> [Float] {
            let n = min(a.count, b.count)
            var out = [Float](repeating: 0, count: n)
            if n > 0 {
                let sys = Array(a[0..<n])
                var mic = Array(b[0..<n])
                echo?.process(system: sys, mic: &mic)
                for i in 0..<n { out[i] = max(-1, min(1, sys[i] + mic[i])) }
            }
            a.removeFirst(n); b.removeFirst(n)
            if a.count > maxSkewFrames { out += a; a.removeAll() }
            if b.count > maxSkewFrames { out += b; b.removeAll() }
            return out
        }
    }
}

/// Turns a streaming recogniser's cumulative transcript into protocol `asr` events.
/// Engines like FluidAudio's StreamingUnifiedAsrManager expose one growing transcript; we cut it into utterances
/// at sentence punctuation after a pause, at a long pause regardless, or when an utterance gets too long.
public final class UtteranceSegmenter {
    public struct Config: Sendable {
        public var pauseMs: Double = 700
        public var hardPauseMs: Double = 1600
        public var maxUtteranceMs: Double = 20_000
        public init() {}
    }
    public let config: Config
    private var committedChars = 0  // prefix of the cumulative transcript already finalized
    private var current: (id: String, startMs: Double, text: String, lastChangeMs: Double)?
    private var counter = 0
    private let idPrefix: String

    public init(config: Config = Config(), idPrefix: String = "u") { self.config = config; self.idPrefix = idPrefix }

    /// Call whenever the engine's cumulative transcript may have changed (and periodically, so pauses are noticed).
    public func update(cumulative: String, nowMs: Double) -> [AsrEvent] {
        var out: [AsrEvent] = []
        let chars = Array(cumulative)
        if committedChars > chars.count { committedChars = chars.count }  // engine revised history; accept it
        let tail = String(chars[committedChars...]).trimmingCharacters(in: .whitespacesAndNewlines)
        if !tail.isEmpty {
            if var c = current {
                if c.text != tail {
                    c.text = tail; c.lastChangeMs = nowMs; current = c
                    out.append(.partial(utteranceId: c.id, startMs: c.startMs, text: tail))
                }
            } else {
                counter += 1
                let c = (id: "\(idPrefix)\(counter)", startMs: nowMs, text: tail, lastChangeMs: nowMs)
                current = c
                out.append(.partial(utteranceId: c.id, startMs: c.startMs, text: tail))
            }
        }
        if let c = current {
            let quiet = nowMs - c.lastChangeMs
            let sentenceEnd = c.text.last.map { ".?!".contains($0) } ?? false
            if (sentenceEnd && quiet >= config.pauseMs) || quiet >= config.hardPauseMs || nowMs - c.startMs >= config.maxUtteranceMs {
                out.append(finalize(c, chars: chars.count, endMs: c.lastChangeMs))
            }
        }
        return out
    }

    /// Flush whatever is in progress (end of recording).
    public func flush(cumulative: String, nowMs: Double) -> [AsrEvent] {
        var out = update(cumulative: cumulative, nowMs: nowMs)
        if let c = current { out.append(finalize(c, chars: Array(cumulative).count, endMs: nowMs)) }
        return out
    }

    private func finalize(_ c: (id: String, startMs: Double, text: String, lastChangeMs: Double), chars: Int, endMs: Double) -> AsrEvent {
        committedChars = chars
        current = nil
        return .final(utteranceId: c.id, startMs: c.startMs, endMs: max(endMs, c.startMs), text: c.text, confidence: nil)
    }
}
