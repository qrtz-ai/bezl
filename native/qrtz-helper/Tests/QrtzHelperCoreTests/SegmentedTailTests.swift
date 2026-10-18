import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// ADR 0118: the segmented release budget, the same numbers as qrtz-talk's `HelperTail::segmented_budget_ms`
/// (crates/qrtz-talk/tests/segmented_tail.rs: talk waits budget + pad 640 + slack 1500 = 6926 ms for 60 s, 4554 for 5 s).
final class SegmentedTailTests: XCTestCase {
    let t = SegmentedTail()

    func testTheBudgetMatchesQrtzTalk() {
        XCTAssertEqual(t.budgetMs(audioMs: 60_000), 6_926 - 640 - 1_500)
        XCTAssertEqual(t.budgetMs(audioMs: 5_000), 4_554 - 640 - 1_500)
        // tail 50 + drain 300 + floor 1500 + (pad 640) × 100/1000.
        XCTAssertEqual(t.budgetMs(audioMs: 0), 50 + 300 + 1_500 + 64)
        XCTAssertEqual(t.finalizeLimitMs(audioMs: 60_000), t.budgetMs(audioMs: 60_000) - 50 - 300)
    }

    func testItStopsGrowingPastTwoSegments() {
        let span = 15_000 - 640
        XCTAssertEqual(t.budgetMs(audioMs: 2 * span), t.budgetMs(audioMs: 60_000))
        XCTAssertEqual(t.budgetMs(audioMs: 60_000), t.budgetMs(audioMs: 300_000))
        var last = 0
        for a in stride(from: 0, through: 120_000, by: 1_000) {
            let b = t.budgetMs(audioMs: a)
            XCTAssertGreaterThanOrEqual(b, last); last = b
        }
        XCTAssertLessThan(t.budgetMs(audioMs: 60_000), DictationTail().budgetMs(audioMs: 60_000), "below the streaming bound")
    }

    func testTheMaxWaitIsKept() {
        let small = SegmentedTail(tail: DictationTail(maxWaitMs: 2_000))
        // finalize at most max(floor, 2000 - 300) = 1700.
        XCTAssertEqual(small.finalizeLimitMs(audioMs: 60_000), 1_700)
        let slow = SegmentedTail(decodeMsPerAudioS: 10_000)
        XCTAssertEqual(slow.finalizeLimitMs(audioMs: 60_000), 20_000 - 300)
        XCTAssertEqual(SegmentedTail(tail: DictationTail(), rule: .wholeClip).finalizeLimitMs(audioMs: 600_000), 20_000 - 300)
        XCTAssertEqual(t.budgetMs(audioMs: -5), t.budgetMs(audioMs: 0))
    }

    func testTheRuleSetsCapAndOverlap() {
        let r = SegmentedTail(tail: DictationTail(), rule: SegmentRule(capMs: 8_000, overlapMs: 200))
        XCTAssertEqual(r.capMs, 8_000); XCTAssertEqual(r.overlapMs, 200)
        XCTAssertEqual(SegmentedTail(tail: DictationTail(), rule: SegmentRule()), t)
    }

    /// The engine uses it for a segmented recogniser's stop, the streaming limit for any other.
    func testTheEnginePicksTheSegmentedLimit() async {
        let seg = SegmentedTranscriber(BurstBatch(), rule: SegmentedTests.short)
        let e = DictationEngine(mic: FakeMic(), transcribers: [seg], inserter: nil, hotkeys: nil, send: { _ in }, now: { 0 })
        let a = await e.finalizeLimitMs(seg, audioMs: 60_000, queuedMs: 500)
        XCTAssertEqual(a, SegmentedTail().finalizeLimitMs(audioMs: 60_000))
        let b = await e.finalizeLimitMs(RecordingASR(), audioMs: 60_000, queuedMs: 500)
        XCTAssertEqual(b, DictationTail().finalizeLimitMs(audioMs: 60_000, queuedMs: 500))
    }
}
