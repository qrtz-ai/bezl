import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// A batch recogniser over synthetic audio: a word is a burst of constant level `0.01 * k` (word "w<k>"); it is
/// heard only when silence (or the pad) follows it inside the decoded audio, the way a recogniser needs end context.
final class BurstBatch: BatchRecognizer, @unchecked Sendable {
    let name: String
    let withTimes: Bool
    let delayMs: UInt64
    let lock = NSLock()
    var decodes: [Int] = []  // samples per decode
    var active = 0, maxActive = 0
    init(name: String = "burst", withTimes: Bool = true, delayMs: UInt64 = 0) {
        self.name = name; self.withTimes = withTimes; self.delayMs = delayMs
    }
    func prepare() async throws {}
    func unload() async {}
    func decode(_ s: [Float]) async throws -> BatchResult {
        lock.withLock { decodes.append(s.count); active += 1; maxActive = max(maxActive, active) }
        if delayMs > 0 { try? await Task.sleep(nanoseconds: delayMs * 1_000_000) }
        defer { lock.withLock { active -= 1 } }
        return Self.hear(s, withTimes: withTimes)
    }
    static func hear(_ s: [Float], withTimes: Bool) -> BatchResult {
        var words: [BatchResult.Word] = []
        var i = 0
        while i < s.count {
            guard s[i] > 0.001 else { i += 1; continue }
            let v = s[i], start = i
            while i < s.count, abs(s[i] - v) < 1e-6 { i += 1 }
            // A burst cut by the start of the audio is still heard (its start is mid-word: the overlap covers it).
            if i < s.count, s[i] <= 0.001 { words.append(.init("w\(Int((v * 100).rounded()))", start: Double(start) / 16_000)) }
        }
        return BatchResult(text: words.map(\.text).joined(separator: " "), words: withTimes ? words : nil)
    }
}

/// Builds speech: words of `wordMs` at levels 1, 2, 3…, `gapMs` between words, and `pauses` (word index → pause ms
/// after it).
func burstSpeech(_ n: Int, wordMs: Int = 200, gapMs: Int = 100, pauses: [Int: Int] = [:], first: Int = 1) -> [Float] {
    var a: [Float] = []
    for k in 0..<n {
        a += [Float](repeating: 0.01 * Float(first + k), count: 16 * wordMs)
        a += [Float](repeating: 0, count: 16 * (pauses[k] ?? gapMs))
    }
    return a
}

func words(_ n: Int, first: Int = 1) -> String { (first..<(first + n)).map { "w\($0)" }.joined(separator: " ") }

final class SegmentedTests: XCTestCase {
    /// The test phrases are a second or two; the default 2 s minimum segment would hold their pause cuts.
    static let short = SegmentRule(minSegmentMs: 0)

    func testTheDefaultRuleIsTheLinuxEvalsPick() {
        let r = SegmentRule()
        XCTAssertEqual(r.pauseMs, 300); XCTAssertEqual(r.capMs, 15_000); XCTAssertEqual(r.overlapMs, 0)
        XCTAssertEqual(r.minSegmentMs, 2_000); XCTAssertEqual(r.vadMinRms, 0.000_2)
    }

    func feed(_ t: SegmentedTranscriber, _ a: [Float], block: Int = 1360) async {
        var i = 0
        while i < a.count { await t.append(Array(a[i..<min(a.count, i + block)])); i += block }
    }

    func testAPauseOf300msCutsInItsMiddleAndAShorterOneDoesnt() {
        var s = PauseSegmenter(rule: Self.short)
        XCTAssertEqual(s.feed(burstSpeech(3, gapMs: 200)), [], "200 ms gaps are not pauses")
        var s2 = PauseSegmenter(rule: Self.short)
        let a = [Float](repeating: 0.05, count: 16 * 1000) + [Float](repeating: 0, count: 16 * 400) + [Float](repeating: 0.05, count: 16 * 500)
        let cuts = s2.feed(a)
        XCTAssertEqual(cuts.count, 1)
        // The cut fires at 300 ms of silence, half of it kept: 1000 + 150 ms.
        XCTAssertEqual(cuts.first, .init(at: 16 * 1160, byCap: false, hadSpeech: true))
    }

    func testTheCapCutsAtTheQuietestFrameOfItsSearchWindow() {
        var s = PauseSegmenter(rule: SegmentRule(capMs: 5_000, minSegmentMs: 0))
        var a = [Float](repeating: 0.05, count: 16 * 6_000)
        // A 100 ms gap between words, and a single quiet frame (a stop consonant) later: the gap is the cut.
        for i in (16 * 4_200)..<(16 * 4_300) { a[i] = 0.02 }
        for i in (16 * 4_600)..<(16 * 4_620) { a[i] = 0.01 }
        let cuts = s.feed(a)
        XCTAssertEqual(cuts.first, .init(at: 16 * 4_240, byCap: true, hadSpeech: true), "the middle of the gap")
    }

    func testRuleFromTheEnvironment() {
        XCTAssertEqual(SegmentRule.fromEnvironment([:]), SegmentRule())
        let r = SegmentRule.fromEnvironment(["QRTZ_ECHO_SEGMENT_PAUSE_MS": "450", "QRTZ_ECHO_SEGMENT_CAP_MS": "off"])
        XCTAssertEqual(r.pauseMs, 450); XCTAssertEqual(r.capMs, .max)
        XCTAssertEqual(SegmentRule.fromEnvironment(["QRTZ_ECHO_SEGMENT_PAUSE_MS": "x", "QRTZ_ECHO_SEGMENT_CAP_MS": "500"]).capMs, 2_000)
        let o = SegmentRule.fromEnvironment(["QRTZ_ECHO_SEGMENT_OVERLAP_MS": "0", "QRTZ_ECHO_SEGMENT_OVERLAP_MODE": "cap", "QRTZ_ECHO_SEGMENT_MIN_MS": "2000"])
        XCTAssertEqual(o.overlapMs, 0); XCTAssertFalse(o.overlapOnPause); XCTAssertEqual(o.minSegmentMs, 2_000)
    }

    func testAMinimumSegmentLengthDefersPauseCuts() {
        var s = PauseSegmenter(rule: SegmentRule(minSegmentMs: 2_000))
        // Pauses after 1.2 s (too early) and after 2.9 s (cuts).
        let cuts = s.feed(burstSpeech(8, pauses: [3: 400, 7: 400]) + burstSpeech(2, first: 9))
        XCTAssertEqual(cuts.count, 1)
        XCTAssertGreaterThanOrEqual(cuts[0].at, 16 * 2_000)
    }

    /// Every overlap setting stitches pause-cut phrases the same.
    func testOverlapSettingsAllStitchPausedPhrases() async {
        for rule in [SegmentRule(overlapMs: 0, minSegmentMs: 0), SegmentRule(overlapOnPause: false, minSegmentMs: 0), SegmentRule(overlapMs: 1_000, minSegmentMs: 0)] {
            let t = SegmentedTranscriber(BurstBatch(), rule: rule)
            await t.reset()
            await feed(t, burstSpeech(9, pauses: [2: 500, 5: 600]))
            let text = await t.finish(padSamples: 16 * 640)
            XCTAssertEqual(text, words(9), "\(rule)")
        }
    }

    /// Phrases between pauses decode in the background; the release decodes only the last one.
    func testPausedPhrasesDecodeInTheBackgroundAndOnlyTheTailAtTheRelease() async {
        let b = BurstBatch()
        let t = SegmentedTranscriber(b, rule: Self.short)
        await t.reset()
        await feed(t, burstSpeech(9, pauses: [2: 500, 5: 600]))
        try? await Task.sleep(nanoseconds: 100_000_000)
        let r1 = await t.transcript(); XCTAssertEqual(r1, words(6), "two phrases decoded while held")
        let text = await t.finish(padSamples: 16 * 640)
        XCTAssertEqual(text, words(9))
        let st = await t.segmentStats()
        XCTAssertEqual(st.segments, 3); XCTAssertEqual(st.backgroundSegments, 2); XCTAssertEqual(st.capCuts, 0)
        XCTAssertGreaterThanOrEqual(st.tailDecodeMs, 0)
        // The tail decode is the last phrase plus its overlap and the pad, not the whole clip.
        // (the rest of the 600 ms pause after its cut + three words + the overlap + the pad)
        XCTAssertLessThan(b.decodes.last!, 16 * (460 + 3 * 300 + 480 + 640 + 40))
    }

    /// Long speech with no pause: cap cuts land inside words; the overlap and the stitch keep each word exactly once.
    func testCapCutsStitchEveryWordOnce() async {
        for times in [true, false] { for overlap in [0, 480] {
            let t = SegmentedTranscriber(BurstBatch(withTimes: times), rule: SegmentRule(capMs: 3_000, overlapMs: overlap, minSegmentMs: 0))
            await t.reset()
            await feed(t, burstSpeech(60, wordMs: 230, gapMs: 90))
            let text = await t.finish(padSamples: 16 * 640)
            XCTAssertEqual(text, words(60), "word times: \(times), overlap \(overlap)")
            let st = await t.segmentStats()
            XCTAssertGreaterThanOrEqual(st.capCuts, 5)
        } }
    }

    func testAWordSplitByTheCutIsKeptWholeOnce() {
        var st = WordStitcher()
        st.add(BatchResult(text: "the most affect", words: [.init("the", start: 0), .init("most", start: 0.3), .init("affect", start: 0.6)]), origin: 0, boundary: nil)
        // The next segment starts 480 ms before the cut at 1.0 s; its "affectionate" starts a little later than the
        // piece did (a word's start moves when its head is cut).
        st.add(BatchResult(text: "most affectionate reception", words: [.init("most", start: 0.1), .init("affectionate", start: 0.3), .init("reception", start: 0.9)]),
               origin: 0.52, boundary: 0.76)
        XCTAssertEqual(st.text, "the most affectionate reception")
        // A short word is not taken for a piece of the next one.
        var s2 = WordStitcher()
        s2.add(BatchResult(text: "in a", words: [.init("in", start: 0), .init("a", start: 0.6)]), origin: 0, boundary: nil)
        s2.add(BatchResult(text: "about", words: [.init("about", start: 0.3)]), origin: 0.52, boundary: 0.76)
        XCTAssertEqual(s2.text, "in a about")
    }

    func testTheReleasePadIsEndContextForTheLastWord() async {
        let a = burstSpeech(4, pauses: [3: 0])  // the last word runs to the release
        let t = SegmentedTranscriber(BurstBatch(), rule: Self.short)
        await t.reset(); await feed(t, a)
        let r2 = await t.finish(padSamples: 0); XCTAssertEqual(r2, words(3))
        await t.reset(); await feed(t, a)
        let r3 = await t.finish(padSamples: 16 * 640); XCTAssertEqual(r3, words(4))
    }

    func testATailWithNoSpeechIsNotDecoded() async {
        let b = BurstBatch()
        let t = SegmentedTranscriber(b, rule: Self.short)
        await t.reset()
        await feed(t, burstSpeech(3) + [Float](repeating: 0, count: 16 * 1_000))
        let r4 = await t.finish(padSamples: 16 * 640); XCTAssertEqual(r4, words(3))
        let st = await t.segmentStats()
        XCTAssertEqual(st.tailDecodeMs, -1); XCTAssertEqual(st.segments, 1); XCTAssertEqual(b.decodes.count, 1)
    }

    func testWholeClipDecodesOnceAtTheRelease() async {
        let b = BurstBatch()
        let t = SegmentedTranscriber(b, rule: .wholeClip, name: "burst-whole")
        await t.reset()
        await feed(t, burstSpeech(9, pauses: [2: 800, 5: 800]))
        let r5 = await t.transcript(); XCTAssertEqual(r5, "")
        let r6 = await t.finish(padSamples: 16 * 640); XCTAssertEqual(r6, words(9))
        XCTAssertEqual(b.decodes.count, 1)
        XCTAssertEqual(t.name, "burst-whole")
    }

    /// One serial chain: background decodes never overlap, and the release waits for the backlog before the tail.
    func testBackgroundDecodesRunOneAtATimeAndTheReleaseWaitsForThem() async {
        let b = BurstBatch(delayMs: 60)
        let t = SegmentedTranscriber(b, rule: Self.short)
        await t.reset()
        await feed(t, burstSpeech(8, pauses: [0: 400, 1: 400, 2: 400, 3: 400, 4: 400, 5: 400, 6: 400]), block: 16_000)
        let r7 = await t.finish(padSamples: 16 * 640); XCTAssertEqual(r7, words(8))
        XCTAssertEqual(b.maxActive, 1)
        let st = await t.segmentStats()
        XCTAssertEqual(st.segments, 8)
        XCTAssertGreaterThan(st.backlogWaitMs, 0)
    }

    func testAResetDropsDecodesStillRunning() async {
        let b = BurstBatch(delayMs: 80)
        let t = SegmentedTranscriber(b, rule: Self.short)
        await t.reset()
        await feed(t, burstSpeech(3, pauses: [0: 400, 1: 400]))
        await t.reset()
        await feed(t, burstSpeech(2, first: 7))
        let r8 = await t.finish(padSamples: 16 * 640); XCTAssertEqual(r8, "w7 w8")
    }

    /// Through the engine: the pad goes to finish(padSamples:), and dictation.stopped and the timing line carry
    /// the recogniser and its segment counts.
    func testTheEngineReportsTheRecogniserAndSegments() async {
        let b = BurstBatch(name: "parakeet-tdt-v2")
        let t = SegmentedTranscriber(b, rule: Self.short)
        let out = Box<[HelperMessage]>([])
        let deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux",
                              features: [], dictationMic: FakeMic(), dictationTranscribers: [t], insertion: FakeInsertion())
        let c = HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        await c.handle(.dictationStart(id: "d1"))
        await c.dictation.onSamples(burstSpeech(6, pauses: [2: 500, 5: 0]))
        await c.handle(.dictationStop(id: "d1"))
        let stopped = out.value.compactMap { m -> (String, DictationAsrInfo?)? in
            if case .dictationStopped("d1", let text, _, _, let info) = m { (text, info) } else { nil }
        }.first
        XCTAssertEqual(stopped?.0, words(6), "the last word, which ran to the release, is heard: the pad reached it")
        XCTAssertEqual(stopped?.1?.asr, "parakeet-tdt-v2")
        XCTAssertEqual(stopped?.1?.segments, 2)
        XCTAssertNotNil(stopped?.1?.tailDecodeMs)
        let line = out.value.compactMap { if case .log(_, let m) = $0, m.hasPrefix("echo timing:") { m } else { nil } }.first ?? ""
        XCTAssertTrue(line.contains("asr=parakeet-tdt-v2"), line)
        XCTAssertTrue(line.contains(" segments=2 bg_segments=1 cap_cuts=0 tail_decode_ms="), line)
        // The pad was never fed as audio: no decode is longer than the speech plus one pad and an overlap.
        XCTAssertLessThan(b.decodes.max() ?? 0, 16 * (3 * 300 + 500 + 480 + 640 + 50))
    }
}
