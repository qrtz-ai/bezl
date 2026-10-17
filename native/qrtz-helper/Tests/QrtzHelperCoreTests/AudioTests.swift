import Foundation
import XCTest
@testable import QrtzHelperCore

final class AudioTests: XCTestCase {
    func testWavHeaderRewrittenOnFinish() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-\(UUID()).wav")
        let w = try WavWriter(url: url)
        try w.append([Float](repeating: 0.5, count: 16_000))
        try w.finish()
        let d = try Data(contentsOf: url)
        XCTAssertEqual(d.count, 44 + 32_000)
        XCTAssertEqual(d.subdata(in: 40..<44).withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) }.littleEndian, 32_000)
        XCTAssertEqual(w.durationMs, 1000)
    }

    func testResamplerRatio() {
        let r = DSP.Resampler(from: 48_000, to: 16_000)
        var total = 0
        for _ in 0..<10 { total += r.process([Float](repeating: 0.1, count: 480)).count }
        XCTAssertEqual(Double(total), 1600, accuracy: 2)
    }

    func testMixerPassesThroughStalledSide() {
        let m = DSP.Mixer(maxSkewFrames: 4)
        XCTAssertEqual(m.pushSystem([0.1, 0.1]), [])
        XCTAssertEqual(m.pushMic([0.1]).count, 1)
        XCTAssertEqual(m.pushSystem([0.2, 0.2, 0.2, 0.2]).count, 5)
    }

    func testSegmenter() {
        let s = UtteranceSegmenter()
        XCTAssertEqual(s.update(cumulative: "Hello", nowMs: 0), [.partial(utteranceId: "u1", startMs: 0, text: "Hello")])
        XCTAssertEqual(s.update(cumulative: "Hello world.", nowMs: 300), [.partial(utteranceId: "u1", startMs: 0, text: "Hello world.")])
        XCTAssertEqual(s.update(cumulative: "Hello world.", nowMs: 1100), [.final(utteranceId: "u1", startMs: 0, endMs: 300, text: "Hello world.", confidence: nil)])
        XCTAssertEqual(s.update(cumulative: "Hello world. Next", nowMs: 1200), [.partial(utteranceId: "u2", startMs: 1200, text: "Next")])
        XCTAssertEqual(s.flush(cumulative: "Hello world. Next one", nowMs: 1500).last, .final(utteranceId: "u2", startMs: 1200, endMs: 1500, text: "Next one", confidence: nil))
    }
}
