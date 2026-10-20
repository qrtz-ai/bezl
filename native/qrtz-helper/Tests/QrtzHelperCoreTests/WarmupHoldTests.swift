import Foundation
import XCTest
@testable import QrtzEcho
@testable import QrtzShared

/// QRT-270: a press before echo is ready keeps every sample from press-down, however long the load outlasts the
/// arm's ring, and all of it reaches the recogniser once the load is done.
final class WarmupHoldTests: XCTestCase {
    func engine(_ asr: StreamingTranscriber) -> (DictationEngine, Box<[HelperMessage]>) {
        let out = Box<[HelperMessage]>([])
        let e = DictationEngine(mic: FakeMic(), transcribers: [asr], inserter: nil, hotkeys: nil,
                                send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        return (e, out)
    }

    func testAPressHeldPastTheArmRingKeepsEverySampleFromPressDown() async {
        let asr = SlowLoadASR(loadMs: 600)
        let (e, out) = engine(asr)
        await e.arm(maxMs: 100)  // the ring alone would keep 1600 samples
        let start = Task { await e.start(id: "w1") }
        try? await Task.sleep(nanoseconds: 50_000_000)
        var fed = 0
        for _ in 0..<20 {  // 2 s of audio while the start waits for the load
            await e.onSamples([Float](repeating: 0.2, count: 1_600)); fed += 1_600
        }
        await start.value
        XCTAssertEqual(asr.lock.withLock { asr.received }, fed)
        let pre = out.value.compactMap { if case .dictationStarted("w1", let ms) = $0 { ms } else { nil } }.first
        XCTAssertEqual(pre, Double(fed) * 1000 / 16_000)
        XCTAssertFalse(out.value.contains { if case .log(_, let m) = $0 { m.contains("dropped") } else { false } })
        let cap = await e.preRoll.capacity
        XCTAssertEqual(cap, 16_000 * 300 / 1000)  // back to the configured pre-roll after the start
    }

    func testAPressOnAReadyRecogniserStillUsesTheArmRing() async {
        let asr = SlowLoadASR(loadMs: 1)
        let (e, _) = engine(asr)
        await e.preload(log: false).value
        await e.arm(maxMs: 100)
        for _ in 0..<5 { await e.onSamples([Float](repeating: 0.2, count: 1_600)) }
        let n = await e.preRollSamples
        XCTAssertEqual(n, 16 * 300)  // max(maxMs, preRollMs): nothing held beyond it without a load to wait for
    }
}
