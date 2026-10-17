import Foundation
import XCTest
@testable import QrtzHelperCore

/// A recogniser that "loads" slowly (like Nemotron), so a recording starts before it is ready.
final class SlowASR: StreamingTranscriber, @unchecked Sendable {
    let name: String
    let loadsInstantly = false
    let latencyMs: Double = 0
    let words: String
    let box = Box<(heard: Int, unloaded: Int)>((0, 0))
    let delayNs: UInt64
    init(_ name: String, words: String, delayNs: UInt64 = 50_000_000) { self.name = name; self.words = words; self.delayNs = delayNs }
    func prepare() async throws { try? await Task.sleep(nanoseconds: delayNs) }
    func append(_ s: [Float]) async { let loud = s.contains { $0 != 0 }; if loud { box.mutate { $0.heard += s.count } } }
    func transcript() async -> String { box.value.heard > 0 ? words : "" }
    func finish() async -> String { await transcript() }
    func reset() async { box.mutate { $0.heard = 0 } }
    func unload() async { box.mutate { $0.unloaded += 1 } }
}

final class FakeDiarizer: ChannelDiarizer, @unchecked Sendable {
    let name = "fake-diar"
    let speaker: Int
    let samples = Box<Int>(0)
    init(speaker: Int) { self.speaker = speaker }
    func prepare() async throws {}
    func reset() async { samples.mutate { $0 = 0 } }
    func append(_ s: [Float]) async -> [DiarizedTurn] { samples.mutate { $0 += s.count }; return [] }
    func finish() async -> [DiarizedTurn] {
        let n = samples.value
        return n > 0 ? [DiarizedTurn(speaker: speaker, startMs: 0, endMs: Double(n) / 16)] : []
    }
    func unload() async {}
}

final class TurnTrackerTests: XCTestCase {
    func testTurnsSpanChunksBridgeShortGapsAndDropBlips() {
        var t = TurnTracker(threshold: 0.5, minFrames: 20, bridgeFrames: 30)
        // 2 speakers, 10 ms frames. Speaker 0 talks frames 0..<50, pauses 10 frames, talks 60..<100 (one turn).
        // Speaker 1 blips for 5 frames (dropped: more than a bridge before its next speech), then talks 150..<200.
        func chunk(_ range: Range<Int>) -> [Float] {
            var p: [Float] = []
            for f in range {
                p.append((f < 50 || (60..<100).contains(f)) ? 0.9 : 0.1)
                p.append(((110..<115).contains(f) || (150..<200).contains(f)) ? 0.8 : 0.0)
            }
            return p
        }
        var out: [DiarizedTurn] = []
        out += t.feed(chunk(0..<70), frames: 70, speakers: 2)
        out += t.feed(chunk(70..<160), frames: 90, speakers: 2)
        out += t.feed(chunk(160..<256), frames: 96, speakers: 2)
        out += t.flush()
        XCTAssertEqual(out, [DiarizedTurn(speaker: 0, startMs: 0, endMs: 1000), DiarizedTurn(speaker: 1, startMs: 1500, endMs: 2000)])
        XCTAssertEqual(t.framesSeen, 256)
    }
}

final class PerChannelRecordingTests: XCTestCase {
    func testMicAndSystemAreTranscribedAndDiarizedSeparately() async throws {
        let out = Box<[HelperMessage]>([])
        let mic = SlowASR("mic", words: "hello from the room."), sys = SlowASR("sys", words: "hi from the call.")
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [FakeAudio("sck")], transcribers: [mic, FakeASR()],
                              helperVersion: "t", os: "linux", features: [])
        deps.systemTranscribers = [sys]
        deps.diarizers = [.mic: FakeDiarizer(speaker: 0), .system: FakeDiarizer(speaker: 1)]
        let c = HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-pc-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        XCTAssertTrue(out.value.contains(.recordingStarted(audioPath: path)), "capture starts while the models load")
        // Audio before the recognisers are ready waits; the mic speaks first, then the call.
        let loud = [Float](repeating: 0.2, count: 16_000), quiet = [Float](repeating: 0, count: 16_000)
        await c.onSamples(loud, of: nil, channels: (system: quiet, mic: loud))
        await c.onSamples(loud, of: nil, channels: (system: loud, mic: quiet))
        await c.handle(.recordingStop())
        let msgs = out.value
        let finals = msgs.compactMap { m -> (String, String)? in
            if case .asr(.final(let id, _, _, let text, _)) = m { return (id, text) } else { return nil }
        }
        XCTAssertTrue(finals.contains { $0.0.hasPrefix("mic-u") && $0.1 == "hello from the room." }, "\(finals)")
        XCTAssertTrue(finals.contains { $0.0.hasPrefix("system-u") && $0.1 == "hi from the call." }, "\(finals)")
        XCTAssertEqual(mic.box.value.heard, 16_000, "the mic recogniser hears only the mic")
        XCTAssertEqual(sys.box.value.heard, 16_000, "the system recogniser hears only the system channel")
        let turns = msgs.filter { if case .diarizationSegment = $0 { true } else { false } }
        XCTAssertEqual(turns.count, 2, "\(turns)")
        XCTAssertTrue(turns.contains(.diarizationSegment(source: .mic, speaker: 0, startMs: 0, endMs: 2000)))
        XCTAssertTrue(turns.contains(.diarizationSegment(source: .system, speaker: 1, startMs: 0, endMs: 2000)))
        let stopped = msgs.firstIndex { if case .recordingStopped = $0 { true } else { false } }!
        XCTAssertTrue(msgs.indices.filter { if case .diarizationSegment = msgs[$0] { true } else { false } }.allSatisfy { $0 < stopped },
                      "speaker turns arrive before the stop ack")
        XCTAssertEqual(mic.box.value.unloaded, 1); XCTAssertEqual(sys.box.value.unloaded, 1)
        try? FileManager.default.removeItem(atPath: path)
    }

    func testWithoutTheModelTheMixGoesToTheBackupAndNoSpeakers() async throws {
        let out = Box<[HelperMessage]>([])
        final class Missing: StreamingTranscriber, @unchecked Sendable {
            let name = "missing"; let loadsInstantly = false
            func prepare() async throws { throw ProtocolError("speech models not downloaded") }
            func append(_ s: [Float]) async {}
            func transcript() async -> String { "" }
            func finish() async -> String { "" }
            func reset() async {}
        }
        let backup = FakeASR()
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [FakeAudio("sck")], transcribers: [Missing(), backup],
                              helperVersion: "t", os: "linux", features: [])
        deps.systemTranscribers = [Missing()]
        deps.diarizers = [.mic: FakeDiarizer(speaker: 0)]
        let c = HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-pc-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        let loud = [Float](repeating: 0.2, count: 16_000)
        await c.onSamples(loud, of: nil, channels: (system: loud, mic: loud))
        await c.handle(.recordingStop())
        let msgs = out.value
        XCTAssertTrue(msgs.contains { if case .asr(.final(let id, _, _, "Hello from the fake recogniser.", _)) = $0 { !id.contains("-") } else { false } })
        XCTAssertFalse(msgs.contains { if case .diarizationSegment = $0 { true } else { false } })
        XCTAssertFalse(msgs.contains { if case .error(let code, _, _, _, _, _, _) = $0 { code == "asr.unavailable" } else { false } })
        try? FileManager.default.removeItem(atPath: path)
    }
}
