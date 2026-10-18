import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// BurstBatch's words, punctuated the way Parakeet does a segment decoded alone: the last word of the decoded audio
/// ends in "." and the first is capitalized; `ends` are true sentence ends ("w3." then "W4") wherever they fall.
final class PunctBatch: BatchRecognizer, @unchecked Sendable {
    let name = "punct"
    let ends: Set<String>
    let delayMs: UInt64
    let lock = NSLock()
    var decodes: [Int] = []
    init(ends: Set<String> = [], delayMs: UInt64 = 0) { self.ends = ends; self.delayMs = delayMs }
    func prepare() async throws {}
    func unload() async {}
    func decode(_ s: [Float]) async throws -> BatchResult {
        lock.withLock { decodes.append(s.count) }
        if delayMs > 0 { try? await Task.sleep(nanoseconds: delayMs * 1_000_000) }
        let ws = BurstBatch.hear(s, withTimes: true).words ?? []
        var out: [BatchResult.Word] = []
        for (i, w) in ws.enumerated() {
            var t = w.text
            if i == 0 || ends.contains(ws[i - 1].text) { t = t.prefix(1).uppercased() + t.dropFirst() }
            if i == ws.count - 1 || ends.contains(w.text) { t += "." }
            out.append(.init(t, start: w.start))
        }
        return BatchResult(text: out.map(\.text).joined(separator: " "), words: out)
    }
}

final class CutWindowTests: XCTestCase {
    /// The test phrases are short: no minimum segment, and a 1 s "after" part.
    static func rule(_ on: Bool = true) -> SegmentRule {
        var r = SegmentRule(minSegmentMs: 0); r.cutWindow = on; r.windowAfterMs = 1_000; return r
    }
    /// Three phrases, pauses after w3 and w6 (cuts at about 0.95 s and 2.25 s).
    let speech = burstSpeech(9, pauses: [2: 500, 5: 600]) + [Float](repeating: 0, count: 16 * 1_000)

    func feed(_ t: SegmentedTranscriber, _ a: [Float], block: Int = 1360) async {
        var i = 0
        while i < a.count { await t.append(Array(a[i..<min(a.count, i + block)])); i += block }
    }

    func testOffByDefaultTheCutsKeepTheSegmentsPunctuation() async {
        XCTAssertFalse(SegmentRule().cutWindow)
        let b = PunctBatch()
        let t = SegmentedTranscriber(b, rule: Self.rule(false))
        await t.reset(); await feed(t, speech)
        let text = await t.finish(padSamples: 16 * 640)
        XCTAssertEqual(text, "W1 w2 w3. W4 w5 w6. W7 w8 w9.")
        let st = await t.segmentStats()
        XCTAssertEqual(b.decodes.count, 3, "no window decodes")
        XCTAssertEqual(st.cutWindows + st.windowFixes + st.windowsSkipped, 0)
    }

    func testAWindowAcrossEachPauseCutTakesItsPunctuationAndCase() async {
        let b = PunctBatch()
        let t = SegmentedTranscriber(b, rule: Self.rule())
        await t.reset(); await feed(t, speech)
        try? await Task.sleep(nanoseconds: 200_000_000)  // both windows decoded while held
        let text = await t.finish(padSamples: 16 * 640)
        XCTAssertEqual(text, "W1 w2 w3 w4 w5 w6 w7 w8 w9.")
        let st = await t.segmentStats()
        XCTAssertEqual(st.cutWindows, 2); XCTAssertEqual(st.windowFixes, 2)
        XCTAssertEqual(st.windowsSkipped, 1, "the pause after w9 is cut too, 0.7 s before the release: no window")
        XCTAssertEqual(st.segments, 3, "windows aren't segments")
        // Each window is at most before + after + the pad.
        XCTAssertEqual(b.decodes.count, 5)
        XCTAssertLessThanOrEqual(b.decodes.max() ?? 0, 16 * (3_000 + 1_000 + 640))
    }

    /// ADR 0121: cap cuts get a window too (the cut lands in the quietest gap of the cap's search window).
    func testACapCutIsRepunctuatedToo() async {
        var r = Self.rule(); r.capMs = 3_000
        let b = PunctBatch()
        let t = SegmentedTranscriber(b, rule: r)
        await t.reset(); await feed(t, burstSpeech(20) + [Float](repeating: 0, count: 16 * 1_000))
        try? await Task.sleep(nanoseconds: 200_000_000)
        let text = await t.finish(padSamples: 16 * 640)
        let st = await t.segmentStats()
        XCTAssertGreaterThanOrEqual(st.capCuts, 2)
        XCTAssertEqual(st.windowFixes, st.capCuts, "every cap cut fixed")
        XCTAssertEqual(text, "W1 " + words(19, first: 2) + ".")
    }

    func testATrueSentenceEndStays() async {
        let t = SegmentedTranscriber(PunctBatch(ends: ["w3"]), rule: Self.rule())
        await t.reset(); await feed(t, speech)
        try? await Task.sleep(nanoseconds: 200_000_000)
        let text = await t.finish(padSamples: 16 * 640)
        XCTAssertEqual(text, "W1 w2 w3. W4 w5 w6 w7 w8 w9.")
        let st = await t.segmentStats()
        XCTAssertEqual(st.cutWindows, 2); XCTAssertEqual(st.windowFixes, 1, "the true end needed no change")
    }

    /// The release never waits for a window: one not started is skipped, and so is a cut with less than the
    /// "after" part captured.
    func testTheReleaseSkipsWindowsNotDecoded() async {
        let b = PunctBatch(delayMs: 60)
        let t = SegmentedTranscriber(b, rule: Self.rule())
        await t.reset(); await feed(t, burstSpeech(9, pauses: [2: 500, 5: 600]))  // 0.9 s after the second cut
        let text = await t.finish(padSamples: 16 * 640)
        let st = await t.segmentStats()
        XCTAssertEqual(st.windowsSkipped, 2 - st.cutWindows)
        XCTAssertGreaterThanOrEqual(st.windowsSkipped, 1)
        XCTAssertTrue(text.hasSuffix("w6. W7 w8 w9."), text)
        // Decodes: three segments plus at most the first cut's window (started before the release).
        XCTAssertLessThanOrEqual(b.decodes.count, 4)
        XCTAssertEqual(text.split(separator: " ").count, 9, "words unchanged")
    }

    func testAResetDropsPendingWindows() async {
        let t = SegmentedTranscriber(PunctBatch(delayMs: 30), rule: Self.rule())
        await t.reset(); await feed(t, speech)
        await t.reset(); await feed(t, burstSpeech(2, first: 7))
        let text = await t.finish(padSamples: 16 * 640)
        XCTAssertEqual(text, "W7 w8.")
        let st = await t.segmentStats()
        XCTAssertEqual(st.cutWindows + st.windowFixes + st.windowsSkipped, 0)
    }

    func testRepunctuateChangesOnlyPunctuationAndCaseOfAnAdjacentMatch() {
        var s = WordStitcher()
        s.add(BatchResult(text: "I went to the store.", words: [.init("I", start: 0), .init("went", start: 0.2), .init("to", start: 0.4),
                                                               .init("the", start: 0.5), .init("store.", start: 0.7)]), origin: 0, boundary: nil)
        XCTAssertEqual(s.repunctuate(cut: 1.0, window: [("store,", 0.7)]), .notReady, "nothing after the cut yet")
        s.add(BatchResult(text: "And then", words: [.init("And", start: 0), .init("then", start: 0.3)]), origin: 1.0, boundary: 1.0)
        let times = s.words.map(\.1)
        // Not adjacent in the window (a word between): no change.
        XCTAssertEqual(s.repunctuate(cut: 1.0, window: [("store", 0.7), ("uh", 0.95), ("and", 1.02)]), .noMatch)
        // Another word: no change.
        XCTAssertEqual(s.repunctuate(cut: 1.0, window: [("shop,", 0.7), ("and", 1.02)]), .noMatch)
        // The later word may start up to 0.1 s before the cut in the window.
        XCTAssertEqual(s.repunctuate(cut: 1.0, window: [("the", 0.5), ("store,", 0.7), ("and", 0.95), ("then", 1.3)]), .changed)
        XCTAssertEqual(s.text, "I went to the store, and then")
        XCTAssertEqual(s.words.map(\.1), times, "times unchanged")
        XCTAssertEqual(s.repunctuate(cut: 1.0, window: [("store,", 0.7), ("and", 1.0)]), .unchanged)
        // No punctuation in the window: none kept; a capital in the window: the stitched word gets it.
        XCTAssertEqual(s.repunctuate(cut: 1.0, window: [("Store", 0.7), ("And", 1.0)]), .changed)
        XCTAssertEqual(s.text, "I went to the store And then")
        XCTAssertEqual(s.repunctuate(cut: 1.0, window: [("store?!", 0.7), ("and", 1.0)]), .changed)
        XCTAssertEqual(s.text, "I went to the store?! and then")
    }

    func testTheKnobsFromTheEnvironment() {
        XCTAssertFalse(SegmentRule.fromEnvironment([:]).cutWindow)
        let r = SegmentRule.fromEnvironment(["QRTZ_ECHO_CUT_WINDOW": "on", "QRTZ_ECHO_CUT_WINDOW_BEFORE_MS": "4000",
                                             "QRTZ_ECHO_CUT_WINDOW_AFTER_MS": "3000"])
        XCTAssertTrue(r.cutWindow); XCTAssertEqual(r.windowBeforeMs, 4_000); XCTAssertEqual(r.windowAfterMs, 3_000)
        let d = SegmentRule.fromEnvironment(["QRTZ_ECHO_CUT_WINDOW": "1", "QRTZ_ECHO_CUT_WINDOW_AFTER_MS": "50"])
        XCTAssertTrue(d.cutWindow); XCTAssertEqual(d.windowBeforeMs, 3_000); XCTAssertEqual(d.windowAfterMs, 2_000, "out of range: ignored")
        XCTAssertFalse(SegmentRule.fromEnvironment(["QRTZ_ECHO_CUT_WINDOW": "off"]).cutWindow)
    }

    func testTheTimingLineCarriesTheWindowCountsOnlyWhenTheyRan() {
        var t = DictationTiming(); var g = SegmentStats(); g.segments = 3
        t.segments = g
        XCTAssertFalse(t.logLine.contains("cut_windows"))
        g.cutWindows = 2; g.windowFixes = 1; g.windowsSkipped = 1; t.segments = g
        XCTAssertTrue(t.logLine.hasSuffix("cut_windows=2 window_fixes=1 windows_skipped=1"), t.logLine)
        XCTAssertEqual(t.json["windowFixes"] as? Int, 1)
    }

    func testSentenceEndsAgainstTheWholeClip() {
        let whole = "I went to the store, and then I came back. It was late."
        let seg = "I went to the store. And then I came back. It was late"
        XCTAssertEqual(EchoWER.sentenceEnds(hyp: seg, ref: whole), .init(agreeing: 1, spurious: 1, lost: 1, spuriousWholeContinues: 1))
        // The whole clip's next word is capitalized: spurious, but not the strict count.
        let s2 = EchoWER.sentenceEnds(hyp: "I went home. Then I slept", ref: "I went home Then I slept")
        XCTAssertEqual(s2, .init(spurious: 1))
        XCTAssertEqual(EchoWER.SentenceEnds(spurious: 2, spuriousWholeContinues: 1).json["spuriousWholeContinues"] as? Int, 1)
        XCTAssertEqual(EchoWER.sentenceEnds(hyp: whole, ref: whole), .init(agreeing: 2))
        XCTAssertEqual(EchoWER.sentenceEnds(hyp: "", ref: whole), .init())
    }
}
