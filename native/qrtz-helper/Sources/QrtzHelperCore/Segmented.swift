import Foundation

// Echo on Parakeet (Jared, 2026-10-18): a batch recogniser (Parakeet TDT v2: whole-utterance decoding, no partials)
// fed in segments cut at the speaker's pauses while the key is held. Finished segments decode in the background
// on one serial chain, so at the release only the last segment (plus a short overlap with the one before) is left
// to decode, and the words are stitched at word boundaries. The pad (end context) goes on the last segment only,
// after the cut points are fixed, so it never looks like a pause. Nano keeps Nemotron streaming.

/// A recogniser that decodes a whole stretch of 16 kHz mono audio at once.
public protocol BatchRecognizer: AnyObject, Sendable {
    /// The id reported as `asr=` (timing line, `dictation.stopped.asr`), e.g. "parakeet-tdt-v2".
    var name: String { get }
    func prepare() async throws
    func decode(_ samples16k: [Float]) async throws -> BatchResult
    func unload() async
}

/// What a batch decode heard. `words` (start times in seconds from the decoded audio's first sample) are optional:
/// with them segments are stitched by time, without them by matching the overlap's words in the text.
public struct BatchResult: Sendable, Equatable {
    public var text: String
    public var words: [Word]?
    public struct Word: Sendable, Equatable {
        public var text: String
        public var start: Double
        public init(_ text: String, start: Double) { self.text = text; self.start = start }
    }
    public init(text: String, words: [Word]? = nil) { self.text = text; self.words = words }
}

/// Where segments are cut. Every knob is configurable (Bozniak's Linux eval sets the final rule):
/// `QRTZ_ECHO_SEGMENT_PAUSE_MS`, `_CAP_MS`, `_OVERLAP_MS`, `_OVERLAP_MODE` (all|cap), `_MIN_MS` override them
/// (`fromEnvironment`), and the bench takes pause/cap/overlap per engine (`parakeet-seg-<pause>-<cap>[-<overlap>]`).
public struct SegmentRule: Sendable, Equatable {
    /// A run of silence at least this long after speech ends a segment (cut in the middle of the run).
    public var pauseMs: Int
    /// A segment plus its pad never gets longer than this (Parakeet's single model window is 15 s); it is cut at
    /// the quietest 100 ms of its last `capSearchMs`.
    public var capMs: Int
    public var capSearchMs: Int
    /// The next segment's decode starts this far before the cut; its words before the middle of the overlap belong
    /// to the previous segment. Off by default: Parakeet often drops a segment that starts inside a word (Linux
    /// eval: 480 ms of overlap on every cut took WER from 2.8 to 6.9 %; on cap cuts only, 3.3 %; none, 2.8 %, the
    /// whole clip's), and the cuts already land in silence (a pause, or the quietest 100 ms before the cap).
    public var overlapMs: Int
    /// The overlap on pause cuts too (false: cap cuts only; a pause cut falls in silence, so there's no word to split).
    public var overlapOnPause: Bool
    /// A pause only cuts a segment at least this long (fewer, longer segments: more context per decode). Linux
    /// eval (40 long dictations, with `vadMinRms` 0.0002): 2 s took WER from 2.74 to 2.55 % clean (whole clip
    /// 2.83 %) and from 3.26 to 3.04 % at a tenth of the level (whole 3.13 %); the tail decoded at release stays
    /// about 5 s (p50; whole clip 43 s).
    public var minSegmentMs: Int
    /// Silence after a background segment's audio (end context for its last word, like the release pad).
    public var segmentPadMs: Int
    /// A pause only cuts after this much speech in the segment (no segments of a cough).
    public var minSpeechMs: Int
    /// The VAD (20 ms frames): speech when the RMS is at least `vadMinRms`, the running noise floor + `vadMarginDb`,
    /// and `vadSpeechRelDb` below the running speech level (so a quiet mic still has speech, and a loud room's
    /// breaths between words are not taken for it). `vadMinRms` 0.0002 (not 0.001): at a tenth of the level the
    /// higher floor over-cut (12.9 segments a dictation, WER 3.91 %; 0.0002: 10.0 and 3.26 %).
    public var vadMinRms: Float
    public var vadMarginDb: Float
    public var vadSpeechRelDb: Float

    public static let wholeClip = SegmentRule(pauseMs: .max, capMs: .max)

    public init(pauseMs: Int = 300, capMs: Int = 15_000, capSearchMs: Int = 1_500, overlapMs: Int = 0,
                segmentPadMs: Int = 640, minSpeechMs: Int = 200, vadMinRms: Float = 0.000_2, vadMarginDb: Float = 8, vadSpeechRelDb: Float = -24,
                overlapOnPause: Bool = true, minSegmentMs: Int = 2_000) {
        self.pauseMs = pauseMs; self.capMs = capMs; self.capSearchMs = capSearchMs; self.overlapMs = overlapMs
        self.overlapOnPause = overlapOnPause; self.minSegmentMs = minSegmentMs
        self.segmentPadMs = segmentPadMs; self.minSpeechMs = minSpeechMs; self.vadMinRms = vadMinRms; self.vadMarginDb = vadMarginDb
        self.vadSpeechRelDb = vadSpeechRelDb
    }

    /// The default rule with the `QRTZ_ECHO_SEGMENT_*` overrides applied (pause/cap: 0 or "off" turns that cut off).
    /// Invalid values are ignored.
    public static func fromEnvironment(_ env: [String: String] = ProcessInfo.processInfo.environment) -> SegmentRule {
        var r = SegmentRule()
        func ms(_ k: String) -> Int?? {
            guard let v = env[k]?.trimmingCharacters(in: .whitespaces), !v.isEmpty else { return nil }
            if v == "off" || v == "0" { return .some(nil) }
            guard let n = Int(v), n > 0, n <= 600_000 else { return nil }
            return .some(n)
        }
        if let p = ms("QRTZ_ECHO_SEGMENT_PAUSE_MS") { r.pauseMs = p ?? .max }
        if let c = ms("QRTZ_ECHO_SEGMENT_CAP_MS") { r.capMs = max(2_000, c ?? .max) }
        if let o = ms("QRTZ_ECHO_SEGMENT_OVERLAP_MS") { r.overlapMs = min(3_000, o ?? 0) }
        if let m = ms("QRTZ_ECHO_SEGMENT_MIN_MS") { r.minSegmentMs = m ?? 0 }
        switch env["QRTZ_ECHO_SEGMENT_OVERLAP_MODE"] {
        case "cap": r.overlapOnPause = false
        case "all": r.overlapOnPause = true
        default: break
        }
        return r
    }
}

/// Per-dictation counts for the timing line and `dictation.stopped` (content-free).
public struct SegmentStats: Sendable, Equatable {
    /// Segments decoded, the tail included (0: nothing was decoded).
    public var segments = 0
    /// Of those, decoded in the background while the key was held.
    public var backgroundSegments = 0
    /// Cuts made by the cap (no pause in time).
    public var capCuts = 0
    /// The release's decode of the tail segment (ms; -1: no tail decode ran, e.g. the tail had no speech).
    public var tailDecodeMs = -1.0
    /// Release → the background chain caught up (ms waited for segments still decoding).
    public var backlogWaitMs = 0.0
    /// Background or tail decodes that threw (their words are missing).
    public var failures = 0
    public init() {}
}

/// Implemented by a recogniser that takes the release pad as an argument instead of as appended audio, so the pad
/// is end context for the last segment only (never a pause, never a segment of its own).
/// ADR 0118: the release deadline of a segmented recogniser (N130: `dictation.stopped` within the budget qrtz-talk
/// waits for, `HelperTail::segmented_budget_ms`, which mirrors this). The release decodes the last segment (at most
/// `capMs - padMs` of audio) with its overlap and the pad, and waits for at most one more still decoding in the
/// background: `decodeFloorMs` plus `decodeMsPerAudioS` per second of that audio, within what `maxWaitMs` leaves
/// after the drain. Unlike `DictationTail.budgetMs` it stops growing past two segments of audio.
public struct SegmentedTail: Sendable, Equatable {
    public var tail: DictationTail
    public var capMs: Int
    public var overlapMs: Int
    public var decodeFloorMs: Int
    public var decodeMsPerAudioS: Int
    public init(tail: DictationTail = DictationTail(), capMs: Int = 15_000, overlapMs: Int = 0, decodeFloorMs: Int = 1_500,
                decodeMsPerAudioS: Int = 100) {
        self.tail = tail; self.capMs = capMs; self.overlapMs = overlapMs; self.decodeFloorMs = decodeFloorMs
        self.decodeMsPerAudioS = decodeMsPerAudioS
    }
    public init(tail: DictationTail, rule: SegmentRule) {
        self.init(tail: tail, capMs: rule.capMs, overlapMs: rule.overlapMs)
    }

    /// The pad and finish part (after the drain): what `budgetMs` allows past `tailMs + drainLimitMs(0)`.
    public func finalizeLimitMs(audioMs: Int) -> Int {
        let a = max(0, audioMs)
        let span = max(1, capMs == .max ? .max : capMs - tail.padMs)
        let last = min(a, span), queued = min(a - last, span)
        let work = Self.sat(Self.sat(Self.sat(last, overlapMs), tail.padMs), queued)
        let want = Self.sat(decodeFloorMs, Int(min(Double(Int.max), Double(work) * Double(decodeMsPerAudioS) / 1000)))
        return min(want, max(decodeFloorMs, tail.maxWaitMs - tail.drainLimitMs(queuedMs: 0)))
    }

    /// Release to `dictation.stopped` at most: tail + drain(0) + finalize.
    public func budgetMs(audioMs: Int) -> Int {
        tail.tailMs + tail.drainLimitMs(queuedMs: 0) + finalizeLimitMs(audioMs: audioMs)
    }

    static func sat(_ a: Int, _ b: Int) -> Int { let (r, o) = a.addingReportingOverflow(b); return o ? .max : r }
}

public protocol PaddedFinishing: StreamingTranscriber {
    func finish(padSamples: Int) async -> String
    /// The counts of the dictation since the last `reset()`.
    func segmentStats() async -> SegmentStats
}

/// Cuts a stream of 16 kHz samples into segments at pauses or the cap. Pure (no decoding), for the tests.
public struct PauseSegmenter: Sendable {
    public static let frame = 320  // 20 ms
    public let rule: SegmentRule
    /// Global sample index where the current (open) segment starts.
    public private(set) var segmentStart = 0
    /// Samples seen.
    public private(set) var position = 0
    /// Speech frames in the open segment, the current silence run (frames), the noise floor and speech level.
    public private(set) var speechFrames = 0
    var silenceRun = 0
    var floor: Float = 0.001
    var speechLevel: Float = 0.02
    var partial: [Float] = []
    /// Frame RMS of the open segment (for the cap's quietest-frame search).
    var frameRms: [Float] = []
    /// Speech flags per frame of the open segment.
    var frameSpeech: [Bool] = []

    public init(rule: SegmentRule) { self.rule = rule }

    public struct Cut: Sendable, Equatable {
        /// Global sample index of the cut (end of one segment, start of the next).
        public var at: Int
        public var byCap: Bool
        /// The segment that ended had speech frames.
        public var hadSpeech: Bool
    }

    /// Feeds samples; returns the cuts they caused (in order).
    public mutating func feed(_ s: [Float]) -> [Cut] {
        var cuts: [Cut] = []
        var i = 0
        while i < s.count {
            let take = min(Self.frame - partial.count, s.count - i)
            partial.append(contentsOf: s[i..<(i + take)])
            i += take
            if partial.count == Self.frame {
                if let c = frameDone(partial) { cuts.append(c) }
                partial.removeAll(keepingCapacity: true)
            }
        }
        return cuts
    }

    /// The open segment has speech frames (a tail with none needn't be decoded).
    public var openHasSpeech: Bool { speechFrames > 0 || partialIsSpeech }
    var partialIsSpeech: Bool { !partial.isEmpty && isSpeech(Self.rms(partial)) }

    static func rms(_ f: [Float]) -> Float {
        var e: Float = 0
        for x in f { e += x * x }
        return (e / Float(max(1, f.count))).squareRoot()
    }

    func isSpeech(_ r: Float) -> Bool {
        let margin = Float(pow(10, Double(rule.vadMarginDb) / 20)), rel = Float(pow(10, Double(rule.vadSpeechRelDb) / 20))
        return r >= max(rule.vadMinRms, floor * margin, speechLevel * rel)
    }

    mutating func frameDone(_ f: [Float]) -> Cut? {
        let r = Self.rms(f)
        // The noise floor follows quiet frames down fast and creeps up slowly (a room getting louder); the speech
        // level follows speech frames.
        floor = r < floor ? 0.9 * floor + 0.1 * r : min(floor * 1.002, speechLevel * 0.1)
        let speech = isSpeech(r)
        position += f.count
        frameRms.append(r); frameSpeech.append(speech)
        if speech { silenceRun = 0; speechFrames += 1; speechLevel = 0.95 * speechLevel + 0.05 * r } else { silenceRun += 1 }
        let ms = 20
        if rule.pauseMs != .max, speechFrames * ms >= rule.minSpeechMs, silenceRun * ms >= rule.pauseMs,
           (position - (silenceRun / 2) * Self.frame - segmentStart) / 16 >= rule.minSegmentMs {
            // Mid-pause: half the silence ends this segment, half starts the next.
            return cut(at: position - (silenceRun / 2) * Self.frame, byCap: false)
        }
        // The cap counts the pad the decode adds, so a segment and its end context fit one 15 s model window.
        if rule.capMs != .max, (position - segmentStart) / 16 >= max(1_000, rule.capMs - rule.segmentPadMs) {
            let n = frameRms.count
            let look = max(1, min(n - 1, rule.capSearchMs / ms))
            // Quietest over 100 ms (5 frames centred on k), so the cut lands in a gap between words rather than
            // in a stop consonant's closure; the latest of equally quiet ones (the longest segment the cap allows).
            func smooth(_ k: Int) -> Float {
                let lo = max(0, k - 2), hi = min(n - 1, k + 2)
                return frameRms[lo...hi].reduce(0, +) / Float(hi - lo + 1)
            }
            var best = n - 1, bestE = smooth(n - 1)
            for k in stride(from: n - 1, through: n - look, by: -1) { let e = smooth(k); if e < bestE { best = k; bestE = e } }
            return cut(at: segmentStart + best * Self.frame, byCap: true)
        }
        return nil
    }

    /// Ends the open segment at `at` (≤ position, on a frame boundary); the frames after it open the next.
    mutating func cut(at: Int, byCap: Bool) -> Cut {
        let keep = (at - segmentStart) / Self.frame
        let had = frameSpeech.prefix(keep).contains(true)
        frameRms.removeFirst(keep); frameSpeech.removeFirst(keep)
        speechFrames = frameSpeech.filter { $0 }.count
        // Only silence follows a pause cut; after a cap cut the run restarts from the frames carried over.
        silenceRun = frameSpeech.reversed().prefix(while: { !$0 }).count
        segmentStart = at
        return Cut(at: at, byCap: byCap, hadSpeech: had)
    }
}

/// Stitches segment results into one word list, by time when the recogniser gives word starts.
public struct WordStitcher: Sendable {
    /// (word, global start seconds or nil)
    public private(set) var words: [(String, Double?)] = []
    public init() {}

    public var text: String { words.map(\.0).joined(separator: " ") }

    static func norm(_ w: String) -> String { w.lowercased().filter { $0.isLetter || $0.isNumber } }

    /// Adds one segment's result. `origin`: global seconds of the decoded audio's first sample; `boundary`: global
    /// seconds from which words belong to this segment (nil: from the start, the first segment).
    public mutating func add(_ r: BatchResult, origin: Double, boundary: Double?) {
        if let ws = r.words, !ws.isEmpty {
            // Times on a 1 ms grid, so a word heard by two segments compares equal against the boundary in both.
            func ms(_ t: Double) -> Double { (t * 1000).rounded() / 1000 }
            let b = boundary.map(ms)
            let mine = ws.map { ($0.text.trimmingCharacters(in: .whitespaces), ms(origin + $0.start)) }.filter { !$0.0.isEmpty }
            guard let b else { words += mine.map { ($0.0, Optional($0.1)) }; return }
            // A word the previous segment started after the boundary is this one's (it heard it with its end).
            while let last = words.last, let t = last.1, t >= b { words.removeLast() }
            let add = mine.filter { $0.1 >= b }
            // The word cut by the cut point, heard by both: the previous segment's piece ("affect") just before the
            // boundary and this one's whole word ("affectionate") just after. Keep this one's.
            if let last = words.last, let t = last.1, let first = add.first, first.1 - t < 0.6 {
                let a = Self.norm(last.0), f = Self.norm(first.0)
                let piece = min(a.count, f.count) >= 3 && (f.hasPrefix(a) || a.hasPrefix(f))
                if !a.isEmpty, a == f || piece { words.removeLast() }
            }
            words += add.map { ($0.0, Optional($0.1)) }
            return
        }
        let mine = r.text.split(whereSeparator: { $0.isWhitespace }).map(String.init)
        guard boundary != nil, !words.isEmpty else { words += mine.map { ($0, nil) }; return }
        // No times: drop the longest prefix of this segment that repeats the end of the text so far (the overlap).
        let norm = Self.norm
        let maxK = min(4, mine.count, words.count)
        var drop = 0
        if maxK > 0 {
            for k in stride(from: maxK, through: 1, by: -1) {
                let tail = words.suffix(k).map { norm($0.0) }, head = mine.prefix(k).map(norm)
                if tail == head, !head.allSatisfy(\.isEmpty) { drop = k; break }
            }
        }
        words += mine.dropFirst(drop).map { ($0, nil) }
    }
}

/// Echo's recogniser: a `BatchRecognizer` behind the streaming interface the engine drives, segmented at pauses.
/// `transcript()` is the text of the segments decoded so far (no partials within a segment).
public actor SegmentedTranscriber: PaddedFinishing {
    public nonisolated let name: String
    public nonisolated var loadsInstantly: Bool { false }
    public nonisolated var latencyMs: Double { 0 }
    let batch: BatchRecognizer
    let rule: SegmentRule
    var seg: PauseSegmenter
    /// Audio of the open segment, plus the overlap before it (global index `bufStart`).
    var buf: [Float] = []
    var bufStart = 0
    var stitched = WordStitcher()
    var stats = SegmentStats()
    var chain: Task<Void, Never>?
    var generation = 0
    var boundaryOfOpen: Double?

    public init(_ batch: BatchRecognizer, rule: SegmentRule = SegmentRule(), name: String? = nil) {
        self.batch = batch; self.rule = rule; self.name = name ?? batch.name
        self.seg = PauseSegmenter(rule: rule)
    }

    public func prepare() async throws { try await batch.prepare() }
    public func unload() async { await batch.unload() }

    public func reset() async {
        generation += 1
        seg = PauseSegmenter(rule: rule)
        buf.removeAll(); bufStart = 0
        stitched = WordStitcher(); stats = SegmentStats()
        chain = nil; boundaryOfOpen = nil
    }

    public func append(_ s: [Float]) async {
        buf.append(contentsOf: s)
        for c in seg.feed(s) { cut(c) }
    }

    func cut(_ c: PauseSegmenter.Cut) {
        if c.byCap { stats.capCuts += 1 }
        let n = c.at - bufStart
        let audio = Array(buf.prefix(n))
        let origin = Double(bufStart) / 16_000, boundary = boundaryOfOpen
        let ov = c.byCap || rule.overlapOnPause ? min(16 * rule.overlapMs, n) : 0
        // Next segment: starts `overlap` before the cut; words before the overlap's middle stay with this one.
        buf.removeFirst(n - ov); bufStart = c.at - ov
        boundaryOfOpen = Double(c.at - ov / 2) / 16_000
        guard c.hadSpeech else { return }
        stats.backgroundSegments += 1
        enqueue(audio, origin: origin, boundary: boundary, padMs: rule.segmentPadMs)
    }

    func enqueue(_ audio: [Float], origin: Double, boundary: Double?, padMs: Int) {
        let prev = chain, gen = generation, batch = self.batch
        chain = Task {
            await prev?.value
            guard await self.generation == gen else { return }
            let r = try? await batch.decode(audio + [Float](repeating: 0, count: 16 * max(0, padMs)))
            await self.decoded(r, gen: gen, origin: origin, boundary: boundary)
        }
    }

    func decoded(_ r: BatchResult?, gen: Int, origin: Double, boundary: Double?) {
        guard gen == generation else { return }
        stats.segments += 1
        guard let r else { stats.failures += 1; return }
        stitched.add(r, origin: origin, boundary: boundary)
    }

    public func transcript() async -> String { stitched.text }

    public func finish() async -> String { await finish(padSamples: 0) }

    public func finish(padSamples: Int) async -> String {
        let gen = generation
        let t0 = ContinuousClock.now
        await chain?.value
        guard gen == generation else { return stitched.text }
        stats.backlogWaitMs = t0.msToNow()
        // The tail: the open segment (with its overlap). A tail with no speech (the speaker paused before letting
        // go) needs no decode: its words, if any, were the previous segment's.
        if seg.openHasSpeech, !buf.isEmpty {
            let audio = buf + [Float](repeating: 0, count: max(0, padSamples))
            let t1 = ContinuousClock.now
            let r = try? await batch.decode(audio)
            guard gen == generation else { return stitched.text }
            stats.tailDecodeMs = t1.msToNow()
            stats.segments += 1
            if let r { stitched.add(r, origin: Double(bufStart) / 16_000, boundary: boundaryOfOpen) } else { stats.failures += 1 }
        }
        buf.removeAll()
        seg = PauseSegmenter(rule: rule)
        return stitched.text
    }

    public func segmentStats() async -> SegmentStats { stats }
}
