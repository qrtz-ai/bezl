import Foundation
import XCTest
@testable import QrtzHelperCore

/// Records the audio of every decode (peak and length), hears nothing.
final class PeakBatch: BatchRecognizer, @unchecked Sendable {
    let name = "peak"
    let lock = NSLock()
    var peaks: [Float] = []
    var maxSample: [Float] = []
    func prepare() async throws {}
    func unload() async {}
    func decode(_ s: [Float]) async throws -> BatchResult {
        lock.withLock { peaks.append(s.map(abs).max() ?? 0) }
        return BatchResult(text: "", words: [])
    }
}

/// ADR 0127: the gated per-segment peak gain (decode path only, off by default).
final class SegmentPeakTests: XCTestCase {
    func testTheGain() {
        XCTAssertEqual(SegmentPeak.gain(peak: 0.5), 1, "normal level: untouched")
        XCTAssertEqual(SegmentPeak.gain(peak: 0.1), 1, "at the threshold: untouched")
        XCTAssertEqual(SegmentPeak.gain(peak: 0.09), 10, accuracy: 1e-5)
        XCTAssertEqual(SegmentPeak.gain(peak: 0.0001), 10, "capped at 10x")
        XCTAssertEqual(SegmentPeak.gain(peak: 0), 1, "silence")
        let (q, g) = SegmentPeak.apply([0.05, -0.02, 0.01])
        XCTAssertTrue(g); XCTAssertEqual(q[0], 0.5, accuracy: 1e-6, "0.9 / 0.05 = 18: capped at 10")
        let (q2, g2) = SegmentPeak.apply([0.095, -0.03])
        XCTAssertTrue(g2); XCTAssertEqual(q2[0], 0.9, accuracy: 1e-5, "peak to 0.9"); XCTAssertEqual(q2[1], -0.9 * 0.03 / 0.095, accuracy: 1e-5)
        let normal: [Float] = [0.3, -0.2]
        let (n, gn) = SegmentPeak.apply(normal)
        XCTAssertFalse(gn); XCTAssertEqual(n, normal)
    }

    func testTheKnobIsOffByDefaultAndNeverImplied() {
        XCTAssertFalse(SegmentRule().segPeak)
        XCTAssertFalse(SegmentRule.fromEnvironment([:]).segPeak)
        XCTAssertTrue(SegmentRule.fromEnvironment(["QRTZ_ECHO_SEG_PEAK": "on"]).segPeak)
        XCTAssertFalse(SegmentRule.fromEnvironment(["QRTZ_ECHO_SEG_PEAK": "off"]).segPeak)
        XCTAssertFalse(SegmentRule.fromEnvironment(["QRTZ_ECHO_SEG_PEAK": "always"]).segPeak, "unknown: off")
    }

    func run(_ level: Float, gate: Bool, windows: Bool = false) async -> (PeakBatch, SegmentStats) {
        var r = SegmentRule(minSegmentMs: 0); r.segPeak = gate; r.cutWindow = windows; r.windowAfterMs = 1_000
        let b = PeakBatch()
        let t = SegmentedTranscriber(b, rule: r)
        await t.reset()
        // Three phrases at `level` (pauses after words 3 and 6), fed like the mic.
        let a = burstSpeech(9, pauses: [2: 500, 5: 600]).map { $0 * level / 0.09 } + [Float](repeating: 0, count: 16 * 1_100)
        var i = 0
        while i < a.count { await t.append(Array(a[i..<min(a.count, i + 1360)])); i += 1360 }
        try? await Task.sleep(nanoseconds: 200_000_000)
        _ = await t.finish(padSamples: 16 * 640)
        return (b, await t.segmentStats())
    }

    func testGateOffIsANoOp() async {
        let (b, st) = await run(0.02, gate: false)
        XCTAssertFalse(b.peaks.isEmpty)
        for p in b.peaks { XCTAssertLessThanOrEqual(p, 0.02 + 1e-6) }
        XCTAssertFalse(st.peakGate); XCTAssertEqual(st.peakGained, 0)
        var t = DictationTiming(); t.segments = st
        XCTAssertFalse(t.logLine.contains("peak_gained"))
        XCTAssertNil(t.json["peakGained"])
    }

    func testAQuietSegmentIsScaledCappedAt10x() async {
        // Peaks 0.0067…0.02: 0.9/peak would be 45-135x, so the cap (10x) applies.
        let (b, st) = await run(0.02, gate: true)
        XCTAssertEqual(b.peaks.count, 3)
        for p in b.peaks { XCTAssertGreaterThan(p, 0.05); XCTAssertLessThanOrEqual(p, 0.2 + 1e-5) }
        XCTAssertTrue(st.peakGate); XCTAssertEqual(st.peakGained, 3, "two background segments and the tail")
        var t = DictationTiming(); t.segments = st
        XCTAssertTrue(t.logLine.hasSuffix("peak_gained=3"), t.logLine)
        XCTAssertEqual(t.json["peakGained"] as? Int, 3)
        // Slightly quiet: scaled to exactly 0.9.
        let (b2, _) = await run(0.09, gate: true)
        XCTAssertEqual(b2.peaks.last ?? 0, 0.9, accuracy: 1e-4)
    }

    func testANormalSegmentIsUntouched() async {
        let (b, st) = await run(0.3, gate: true)
        XCTAssertEqual(b.peaks.count, 3)
        XCTAssertEqual(b.peaks.max() ?? 0, 0.3, accuracy: 1e-5)
        XCTAssertEqual(st.peakGained, 0)
    }

    /// Cut windows go through the same decode path (gained, not counted as segments); the VAD sees the raw level.
    func testCutWindowsAreGainedAndTheVadIsNot() async {
        let (b, st) = await run(0.02, gate: true, windows: true)
        let (b0, st0) = await run(0.02, gate: false, windows: true)
        XCTAssertEqual(st.capCuts, st0.capCuts); XCTAssertEqual(st.segments, st0.segments, "same cuts: the VAD is untouched")
        XCTAssertEqual(b.peaks.count, b0.peaks.count)
        XCTAssertGreaterThan(st.cutWindows, 0)
        for p in b.peaks { XCTAssertGreaterThan(p, 0.05) }
        XCTAssertEqual(st.peakGained, st.segments)
    }
}
