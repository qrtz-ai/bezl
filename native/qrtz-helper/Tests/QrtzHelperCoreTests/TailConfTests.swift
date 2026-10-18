import Foundation
import XCTest
@testable import QrtzHelperCore

/// BurstBatch's words, with a confidence per decode: `tail` for any decode that contains the word "w9" or later
/// (the release tail in these tests), 0.95 otherwise; nil when `reports` is false.
final class ConfBatch: BatchRecognizer, @unchecked Sendable {
    let name = "conf"
    let tail: Float, reports: Bool
    init(tail: Float, reports: Bool = true) { self.tail = tail; self.reports = reports }
    func prepare() async throws {}
    func unload() async {}
    func decode(_ s: [Float]) async throws -> BatchResult {
        var r = BurstBatch.hear(s, withTimes: true)
        let isTail = (r.words ?? []).contains { Int($0.text.dropFirst()) ?? 0 >= 9 }
        r.confidence = reports ? (isTail ? tail : 0.95) : nil
        return r
    }
}

/// ADR 0131 (N169): the release tail's confidence floor (off by default).
final class TailConfTests: XCTestCase {
    /// Two phrases (w1-w3, pause, w4-w6), then a tail of `tailWords` words from w9 on.
    func run(_ b: BatchRecognizer, floor: Float?, tailWords: Int = 1) async -> (String, SegmentStats) {
        var r = SegmentRule(minSegmentMs: 0); r.tailConfFloor = floor
        let t = SegmentedTranscriber(b, rule: r)
        await t.reset()
        let a = burstSpeech(6, pauses: [2: 500, 5: 600]) + burstSpeech(tailWords, first: 9)
        var i = 0
        while i < a.count { await t.append(Array(a[i..<min(a.count, i + 1360)])); i += 1360 }
        let text = await t.finish(padSamples: 16 * 640)
        return (text, await t.segmentStats())
    }

    func testOffIsANoOp() async {
        XCTAssertNil(SegmentRule().tailConfFloor)
        XCTAssertNil(SegmentRule.fromEnvironment([:]).tailConfFloor)
        let (text, st) = await run(ConfBatch(tail: 0.2), floor: nil)
        XCTAssertEqual(text, "w1 w2 w3 w4 w5 w6 w9", "an unsure tail is kept with the floor off")
        XCTAssertFalse(st.tailDropped)
        XCTAssertEqual(st.tailConf ?? -1, 0.2, accuracy: 1e-6, "the confidence is still reported")
        var t = DictationTiming(); t.segments = st
        XCTAssertTrue(t.logLine.contains(" tail_conf=0.200"), t.logLine)
        XCTAssertFalse(t.logLine.contains("tail_dropped"))
        XCTAssertEqual(t.json["tailDropped"] as? Bool, false)
    }

    func testALowConfidenceShortTailIsDropped() async {
        let (text, st) = await run(ConfBatch(tail: 0.4), floor: 0.6, tailWords: 2)
        XCTAssertEqual(text, "w1 w2 w3 w4 w5 w6", "the earlier segments' text stays")
        XCTAssertTrue(st.tailDropped)
        var t = DictationTiming(); t.segments = st
        XCTAssertTrue(t.logLine.hasSuffix("tail_conf=0.400 tail_dropped=1"), t.logLine)
        XCTAssertEqual(t.json["tailDropped"] as? Bool, true)
        XCTAssertEqual(t.json["tailConf"] as? Double ?? -1, 0.4, accuracy: 1e-6)
    }

    func testALongOrConfidentTailIsKept() async {
        let (long, st1) = await run(ConfBatch(tail: 0.4), floor: 0.6, tailWords: 5)
        XCTAssertEqual(long, "w1 w2 w3 w4 w5 w6 w9 w10 w11 w12 w13", "5 words: kept")
        XCTAssertFalse(st1.tailDropped)
        let (sure, st2) = await run(ConfBatch(tail: 0.8), floor: 0.6, tailWords: 1)
        XCTAssertEqual(sure, "w1 w2 w3 w4 w5 w6 w9")
        XCTAssertFalse(st2.tailDropped)
        let (four, st3) = await run(ConfBatch(tail: 0.59), floor: 0.6, tailWords: 4)
        XCTAssertEqual(four, "w1 w2 w3 w4 w5 w6", "4 words is still short")
        XCTAssertTrue(st3.tailDropped)
    }

    func testANilConfidenceIsKept() async {
        let (text, st) = await run(ConfBatch(tail: 0.1, reports: false), floor: 0.6)
        XCTAssertEqual(text, "w1 w2 w3 w4 w5 w6 w9")
        XCTAssertFalse(st.tailDropped); XCTAssertNil(st.tailConf)
        var t = DictationTiming(); t.segments = st
        XCTAssertFalse(t.logLine.contains("tail_conf"))
        XCTAssertNil(t.json["tailConf"])
    }

    func testTheFloorFromTheEnvironment() {
        XCTAssertEqual(SegmentRule.fromEnvironment(["QRTZ_ECHO_TAIL_CONF": "0.6"]).tailConfFloor, 0.6)
        XCTAssertEqual(SegmentRule.fromEnvironment(["QRTZ_ECHO_TAIL_CONF": "1"]).tailConfFloor, 1)
        for bad in ["off", "0", "-0.2", "1.5", "x", "nan"] {
            XCTAssertNil(SegmentRule.fromEnvironment(["QRTZ_ECHO_TAIL_CONF": bad]).tailConfFloor, bad)
        }
    }
}
