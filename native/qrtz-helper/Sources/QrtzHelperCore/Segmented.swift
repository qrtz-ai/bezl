import Foundation

// Echo on Parakeet (Jared, 2026-10-18): a batch recogniser (Parakeet TDT v2: whole-utterance decoding, no partials)
// fed in segments cut at the speaker's pauses while the key is held. Finished segments decode in the background
// on one serial chain, so at the release only the last segment (plus a short overlap with the one before) is left
// to decode, and the words are stitched at word boundaries. Every segment is decoded with 640 ms of silence after
// it (end context), added after the cut points are fixed, so it never looks like a pause. Nano keeps Nemotron
// streaming. ADR 0120 (off by default): a window decoded across each cut (pause or cap) fixes the cut's punctuation.

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
    /// The recogniser's own confidence in the result (Parakeet: FluidAudio's `ASRResult.confidence`, the mean TDT
    /// token confidence), when it reports one. ADR 0131 uses it to drop a short, unsure release tail.
    public var confidence: Float?
    public init(text: String, words: [Word]? = nil, confidence: Float? = nil) {
        self.text = text; self.words = words; self.confidence = confidence
    }
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
    /// ADR 0120 / review §12.35 (N167): re-punctuate cuts from a window (pause cuts, and cap cuts since ADR 0121). Each segment is decoded alone and
    /// Parakeet ends almost every one with a sentence end, so a pause mid-sentence came out as "store. And then".
    /// With this on, once a cut has `windowAfterMs` of audio after it, a background decode of
    /// `[max(previous cut, cut − windowBeforeMs), cut + windowAfterMs]` (+ the segment pad) runs on the same serial
    /// chain, and its words decide only the cut word's trailing punctuation and the next word's first-letter case
    /// (words and times stay). A window not decoded by the release is skipped, never waited for. Linux
    /// (`linux-crosscheck/seg/cutwin.py`, 3 s / 2 s): false sentence ends against the whole clip 99 → 24, formatted
    /// diff 7.46 → 6.10 %. Off until the Mac gate (tuning §12): `QRTZ_ECHO_CUT_WINDOW=on`, sizes
    /// `QRTZ_ECHO_CUT_WINDOW_BEFORE_MS` / `_AFTER_MS`, bench engine `parakeet-segmented-win`.
    public var cutWindow = false
    /// ADR 0127 / review §12.37: a gated per-segment peak gain on the decode path only (`SegmentPeak`): a segment,
    /// cut window or tail whose peak is below 0.1 is scaled so its peak is 0.9 (gain at most 10×) before Parakeet;
    /// louder audio is untouched. The VAD, nano and the shared mic stream never see it. Off by default
    /// (`QRTZ_ECHO_SEG_PEAK=on`); never always-on: an ungated peak to 0.9 cost normal speech +11 % errors (Linux,
    /// 2.57 → 2.85 %), while the gate leaves normal alone and recovers ×0.03 (4.15 → 2.94 %).
    public var segPeak = false
    /// ADR 0131 / review §12.38 (N169): a release tail of at most `tailMaxWords` words whose decode confidence is
    /// below this floor isn't stitched (Parakeet hears "Yeah." in a second of room tone before the release); the
    /// earlier segments' text stays. nil (the default): off. `QRTZ_ECHO_TAIL_CONF=0.6` (0 < floor ≤ 1). Linux: 0.6
    /// drops 76 of 122 phantom tails and 0 of 161 real ones; set the Mac floor from real `tail_conf=` values.
    public var tailConfFloor: Float?
    public static let tailMaxWords = 4
    public var windowBeforeMs = 3_000
    public var windowAfterMs = 2_000

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
        switch env["QRTZ_ECHO_SEG_PEAK"]?.trimmingCharacters(in: .whitespaces).lowercased() {
        case "on", "1", "true", "yes": r.segPeak = true
        case "off", "0", "false", "no": r.segPeak = false
        default: break
        }
        if let v = env["QRTZ_ECHO_TAIL_CONF"]?.trimmingCharacters(in: .whitespaces).lowercased(), !v.isEmpty {
            if v == "off" || v == "0" { r.tailConfFloor = nil }
            else if let f = Float(v), f > 0, f <= 1 { r.tailConfFloor = f }
        }
        switch env["QRTZ_ECHO_CUT_WINDOW"]?.trimmingCharacters(in: .whitespaces).lowercased() {
        case "on", "1", "true", "yes": r.cutWindow = true
        case "off", "0", "false", "no": r.cutWindow = false
        default: break
        }
        func win(_ k: String) -> Int? {
            guard let v = env[k].flatMap({ Int($0.trimmingCharacters(in: .whitespaces)) }), (200...10_000).contains(v) else { return nil }
            return v
        }
        if let b = win("QRTZ_ECHO_CUT_WINDOW_BEFORE_MS") { r.windowBeforeMs = b }
        if let a = win("QRTZ_ECHO_CUT_WINDOW_AFTER_MS") { r.windowAfterMs = a }
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
    /// ADR 0120 cut windows: decoded, the cuts whose punctuation or case they changed, and the cuts left
    /// as stitched because their window wasn't decoded (or applicable) by the release.
    public var cutWindows = 0
    public var windowFixes = 0
    public var windowsSkipped = 0
    /// ADR 0127: the gated peak gain was on for this dictation, and how many segments (the tail included) it scaled.
    public var peakGate = false
    public var peakGained = 0
    /// ADR 0131: the release tail decode's confidence (when the recogniser reports one), and whether the tail was
    /// dropped under `tailConfFloor`.
    public var tailConf: Float?
    public var tailDropped = false
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

/// ADR 0127: the gated per-segment peak gain (decode path only). Below `threshold` peak abs the audio is scaled so
/// its peak becomes `target`, with the gain capped at `maxGain`; anything louder (and pure silence) is returned
/// unchanged.
public enum SegmentPeak {
    public static let threshold: Float = 0.1
    public static let target: Float = 0.9
    public static let maxGain: Float = 10

    /// The gain for audio with this peak: 1 at or above the threshold (or for silence).
    public static func gain(peak: Float) -> Float {
        guard peak > 0, peak < threshold else { return 1 }
        return min(maxGain, target / peak)
    }

    /// The audio to decode, and whether it was scaled.
    public static func apply(_ x: [Float]) -> (audio: [Float], gained: Bool) {
        var peak: Float = 0
        for v in x { peak = max(peak, abs(v)) }
        let g = gain(peak: peak)
        guard g != 1 else { return (x, false) }
        return (x.map { $0 * g }, true)
    }
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

    public enum WindowFix: Equatable { case changed, unchanged, noMatch, notReady }

    static func key(_ w: String) -> String { w.lowercased().filter { $0.isLetter || $0.isNumber || $0 == "'" } }
    static func trailingPunct(_ w: String) -> Substring {
        w[w.index(w.endIndex, offsetBy: -w.reversed().prefix(while: { ".?!,;:".contains($0) }).count)...]
    }

    /// ADR 0120: the words on each side of the cut at `cut` (global seconds) take the window's punctuation and case.
    /// `window`: the window decode's words with global start times. The window's last word before the cut and its
    /// first at or after `cut − 0.1 s` must match (normalized) the stitched words on each side of the cut and be
    /// adjacent; then only the earlier word's trailing `[.?!,;:]+` and the later word's first-letter case change.
    public mutating func repunctuate(cut: Double, window: [(String, Double)]) -> WindowFix {
        let c = (cut * 1000).rounded() / 1000
        guard let j = words.firstIndex(where: { ($0.1 ?? -1) >= c }) else { return .notReady }
        guard j > 0, words[j - 1].1 != nil else { return .noMatch }
        let before = words[j - 1].0, after = words[j].0
        let kb = Self.key(before), ka = Self.key(after)
        guard !kb.isEmpty, !ka.isEmpty,
              let bi = window.lastIndex(where: { $0.1 < c && Self.key($0.0) == kb }),
              let ai = window.firstIndex(where: { $0.1 >= c - 0.1 && Self.key($0.0) == ka }), ai == bi + 1 else { return .noMatch }
        let wb = window[bi].0, wa = window[ai].0
        let nb = String(before.dropLast(Self.trailingPunct(before).count)) + Self.trailingPunct(wb)
        var na = after
        if let f = after.first, let g = wa.first {
            na = (g.isUppercase ? String(f).uppercased() : String(f).lowercased()) + after.dropFirst()
        }
        guard nb != before || na != after else { return .unchanged }
        words[j - 1].0 = nb; words[j].0 = na
        return .changed
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
    /// ADR 0120 cut windows: recent audio (global index `histStart`), the cuts waiting for theirs, the last
    /// cut, samples seen, and the release (windows not started by then are skipped).
    var hist: [Float] = []
    var histStart = 0
    var total = 0
    var lastCut = 0
    var pending: [PendingCut] = []
    var releasing = false
    struct PendingCut { var at: Int; var lo: Int; var queued = false; var words: [(String, Double)]? }

    public init(_ batch: BatchRecognizer, rule: SegmentRule = SegmentRule(), name: String? = nil) {
        self.batch = batch; self.rule = rule; self.name = name ?? batch.name
        self.seg = PauseSegmenter(rule: rule)
        self.stats.peakGate = rule.segPeak
    }

    public func prepare() async throws { try await batch.prepare() }
    public func unload() async { await batch.unload() }

    public func reset() async {
        generation += 1
        seg = PauseSegmenter(rule: rule)
        buf.removeAll(); bufStart = 0
        stitched = WordStitcher(); stats = SegmentStats(); stats.peakGate = rule.segPeak
        chain = nil; boundaryOfOpen = nil
        hist.removeAll(); histStart = 0; total = 0; lastCut = 0; pending.removeAll(); releasing = false
    }

    public func append(_ s: [Float]) async {
        buf.append(contentsOf: s)
        if rule.cutWindow { hist.append(contentsOf: s) }
        total += s.count
        for c in seg.feed(s) { cut(c) }
        if rule.cutWindow { queueWindows() }
    }

    /// Queues the window of every cut that has `windowAfterMs` of audio after it, and trims the history.
    func queueWindows() {
        let after = 16 * rule.windowAfterMs
        for i in pending.indices where !pending[i].queued && total >= pending[i].at + after {
            pending[i].queued = true
            let lo = max(pending[i].lo, histStart), hi = min(total, pending[i].at + after)
            guard hi > lo else { continue }
            enqueueWindow(Array(hist[(lo - histStart)..<(hi - histStart)]), origin: Double(lo) / 16_000, at: pending[i].at)
        }
        let keep = 16 * (rule.windowBeforeMs + rule.windowAfterMs + 1_000)
        if hist.count > keep + 16 * 1_000 { let d = hist.count - keep; hist.removeFirst(d); histStart += d }
    }

    /// ADR 0127: the decode-path copy of a segment, gained when the gate is on and it is quiet (`count`: a segment
    /// or the tail, not a cut window).
    func forDecode(_ audio: [Float], count: Bool) -> [Float] {
        guard rule.segPeak else { return audio }
        let (a, gained) = SegmentPeak.apply(audio)
        if gained, count { stats.peakGained += 1 }
        return a
    }

    func enqueueWindow(_ audio: [Float], origin: Double, at: Int) {
        let audio = forDecode(audio, count: false)
        let prev = chain, gen = generation, batch = self.batch, pad = rule.segmentPadMs
        chain = Task {
            await prev?.value
            // At the release a window not started is skipped: the tail decode never waits for one.
            guard await self.generation == gen, await !self.releasing else { return }
            let r = try? await batch.decode(audio + [Float](repeating: 0, count: 16 * max(0, pad)))
            await self.windowDecoded(r, gen: gen, origin: origin, at: at)
        }
    }

    func windowDecoded(_ r: BatchResult?, gen: Int, origin: Double, at: Int) {
        guard gen == generation, let i = pending.firstIndex(where: { $0.at == at }) else { return }
        guard let ws = r?.words else { pending.remove(at: i); return }  // a failed decode or no word times
        stats.cutWindows += 1
        pending[i].words = ws.map { ($0.text, ((origin + $0.start) * 1000).rounded() / 1000) }
        applyWindows(final: false)
    }

    /// Applies every decoded window whose cut has words stitched on both sides. `final`: the release; what is left
    /// is skipped.
    func applyWindows(final: Bool) {
        var left: [PendingCut] = []
        for p in pending {
            guard let w = p.words else { left.append(p); continue }
            switch stitched.repunctuate(cut: Double(p.at) / 16_000, window: w) {
            case .changed: stats.windowFixes += 1
            case .notReady: left.append(p)
            case .unchanged, .noMatch: break
            }
        }
        pending = left
        if final { stats.windowsSkipped += pending.count; pending.removeAll() }
    }

    func cut(_ c: PauseSegmenter.Cut) {
        if rule.cutWindow {
            if c.hadSpeech { pending.append(PendingCut(at: c.at, lo: max(lastCut, c.at - 16 * rule.windowBeforeMs))) }
            lastCut = c.at
        }
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
        let audio = forDecode(audio, count: true)
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
        if !pending.isEmpty { applyWindows(final: false) }
    }

    public func transcript() async -> String { stitched.text }

    public func finish() async -> String { await finish(padSamples: 0) }

    public func finish(padSamples: Int) async -> String {
        let gen = generation
        let t0 = ContinuousClock.now
        releasing = true  // windows still queued are skipped (one already decoding finishes first: serial decoder)
        await chain?.value
        guard gen == generation else { return stitched.text }
        stats.backlogWaitMs = t0.msToNow()
        // The tail: the open segment (with its overlap). A tail with no speech (the speaker paused before letting
        // go) needs no decode: its words, if any, were the previous segment's.
        if seg.openHasSpeech, !buf.isEmpty {
            let audio = forDecode(buf, count: true) + [Float](repeating: 0, count: max(0, padSamples))
            let t1 = ContinuousClock.now
            let r = try? await batch.decode(audio)
            guard gen == generation else { return stitched.text }
            stats.tailDecodeMs = t1.msToNow()
            stats.segments += 1
            if let r {
                stats.tailConf = r.confidence
                if Self.dropsTail(r, floor: rule.tailConfFloor) { stats.tailDropped = true }
                else { stitched.add(r, origin: Double(bufStart) / 16_000, boundary: boundaryOfOpen) }
            } else { stats.failures += 1 }
        }
        if rule.cutWindow { applyWindows(final: true) }
        buf.removeAll()
        seg = PauseSegmenter(rule: rule)
        return stitched.text
    }

    public func segmentStats() async -> SegmentStats { stats }

    /// ADR 0131: a tail of at most `tailMaxWords` words with a confidence below the floor (set) is dropped. No floor,
    /// no confidence, or a longer tail: kept.
    static func dropsTail(_ r: BatchResult, floor: Float?) -> Bool {
        guard let floor, let c = r.confidence, c < floor else { return false }
        let n = r.text.split(whereSeparator: { $0.isWhitespace }).count
        return n > 0 && n <= SegmentRule.tailMaxWords
    }
}
