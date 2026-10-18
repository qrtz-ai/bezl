import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// A mic whose resampler holds back `held` samples until a flush (ADR 0115).
final class FlushingMic: MicSource, @unchecked Sendable {
    let lock = NSLock()
    var cb: (@Sendable ([Float]) -> Void)?
    var flushes = 0
    let held: [Float]
    init(held: [Float]) { self.held = held }
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws { lock.withLock { cb = onSamples } }
    func stop() { lock.withLock { cb = nil } }
    func send(_ s: [Float]) { lock.withLock { cb }?(s) }
    func flush() {
        let c = lock.withLock { flushes += 1; return cb }
        c?(held)
    }
}

/// Logs each append: "s<n>" for audio, "z<n>" for silence (the pad).
final class OrderASR: StreamingTranscriber, @unchecked Sendable {
    let name = "order"
    let lock = NSLock()
    var log: [String] = []
    func prepare() async throws {}
    func append(_ s: [Float]) async { lock.withLock { log.append((s.allSatisfy { $0 == 0 } ? "z" : "s") + "\(s.count)") } }
    func transcript() async -> String { "" }
    func finish() async -> String { "hello" }
    func reset() async { lock.withLock { log.removeAll() } }
}

final class ResamplerFlushTests: XCTestCase {
    func testAStopFlushesTheResamplerBeforeThePad() async {
        let mic = FlushingMic(held: [Float](repeating: 0.3, count: 77))
        let asr = OrderASR()
        let e = DictationEngine(mic: mic, transcribers: [asr], inserter: nil, hotkeys: nil, send: { _ in },
                                now: { 0 }, tail: DictationTail(tailMs: 0, padMs: 640))
        await e.start(id: "d1")
        mic.send([Float](repeating: 0.2, count: 1_600))
        await e.stop(id: "d1")
        XCTAssertEqual(mic.flushes, 1)
        let log = asr.lock.withLock { asr.log }
        guard let held = log.firstIndex(of: "s77") else { return XCTFail("the flushed tail never reached the recogniser: \(log)") }
        XCTAssertLessThan(log.firstIndex(of: "s1600") ?? .max, held, "after the audio")
        XCTAssertLessThan(held, log.firstIndex(where: { $0.hasPrefix("z") }) ?? .max, "before the pad")
    }

    func testTheHubFlushesOnlyARunningDevice() throws {
        let mic = FlushingMic(held: [1])
        let hub = MicHub(source: mic)
        let got = Box<[Float]>([])
        let v = hub.view("dictation")
        v.flush()
        XCTAssertEqual(mic.flushes, 0)
        try v.start(onSamples: { s in got.mutate { $0 += s } })
        v.flush()
        XCTAssertEqual(mic.flushes, 1); XCTAssertEqual(got.value, [1])
    }
}
