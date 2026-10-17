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

/// N121: a diarizer whose model calls (or load) are slower than real time. Each processed block becomes one turn
/// starting where the previous one ended, so the turns sent show the audio was taken in order, without gaps.
final class SlowDiarizer: ChannelDiarizer, @unchecked Sendable {
    let name = "slow-diar"
    let appendNs: UInt64, prepareNs: UInt64
    let processed = Box<Int>(0)
    let abandoned = Box<Int>(0)
    init(appendNs: UInt64 = 0, prepareNs: UInt64 = 0) { self.appendNs = appendNs; self.prepareNs = prepareNs }
    func prepare() async throws { if prepareNs > 0 { try? await Task.sleep(nanoseconds: prepareNs) } }
    func reset() async {}
    func append(_ s: [Float]) async -> [DiarizedTurn] {
        if appendNs > 0 { try? await Task.sleep(nanoseconds: appendNs) }
        let a = processed.value
        processed.mutate { $0 += s.count }
        return [DiarizedTurn(speaker: 0, startMs: Double(a) / 16, endMs: Double(a + s.count) / 16)]
    }
    func finish() async -> [DiarizedTurn] { [] }
    func unload() async {}
    func abandon() -> [DiarizedTurn] {
        abandoned.mutate { $0 += 1 }
        let p = processed.value
        return p > 0 ? [DiarizedTurn(speaker: 1, startMs: 0, endMs: Double(p) / 16)] : []
    }
}

final class FailingDiarizer: ChannelDiarizer, @unchecked Sendable {
    struct NoModel: Error {}
    let name = "failing-diar"
    func prepare() async throws { throw NoModel() }
    func reset() async {}
    func append(_ s: [Float]) async -> [DiarizedTurn] { [] }
    func finish() async -> [DiarizedTurn] { [] }
    func unload() async {}
    func abandon() -> [DiarizedTurn] { [] }
}

final class DiarizerBacklogTests: XCTestCase {
    func record(_ d: ChannelDiarizer, seconds: Int, waitMs: UInt64, paceNs: UInt64 = 0) async -> (msgs: [HelperMessage], stopMs: Double, peak: Int, cap: Int) {
        let out = Box<[HelperMessage]>([])
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [FakeAudio("sck")],
                              transcribers: [SlowASR("mic", words: "hi.", delayNs: 1_000_000)],
                              helperVersion: "t", os: "linux", features: [])
        deps.systemTranscribers = [SlowASR("sys", words: "yo.", delayNs: 1_000_000)]
        deps.diarizers = [.mic: d]
        deps.diarizerBacklogSamples = 16_000 * 5
        deps.diarizerStopWaitMs = waitMs
        let c = HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-n121-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        let loud = [Float](repeating: 0.2, count: 16_000)
        for _ in 0..<seconds {
            await c.onSamples(loud, of: nil, channels: (system: loud, mic: loud))
            if paceNs > 0 { try? await Task.sleep(nanoseconds: paceNs) }
        }
        let t = Date()
        await c.handle(.recordingStop())
        let stopMs = Date().timeIntervalSince(t) * 1000
        let peak = await c.lastDiarizerPeaks[.mic] ?? -1
        try? FileManager.default.removeItem(atPath: path)
        return (out.value, stopMs, peak, deps.diarizerBacklogSamples)
    }

    func turns(_ msgs: [HelperMessage]) -> [(Int, Double, Double)] {
        msgs.compactMap { if case .diarizationSegment(.mic, let s, let a, let b) = $0 { (s, a, b) } else { nil } }
    }

    func testASlowModelStandsTheChannelDownBoundedAndTheStopStaysInsideTheAck() async {
        // 60 s of audio arrive at 33x real time; each 1 s block takes 100 ms to diarize, so the queue passes 5 s.
        let d = SlowDiarizer(appendNs: 100_000_000)
        let r = await record(d, seconds: 60, waitMs: 500, paceNs: 30_000_000)
        XCTAssertLessThanOrEqual(r.peak, r.cap, "the queue never holds more than the cap")
        XCTAssertGreaterThan(r.peak, 0)
        XCTAssertTrue(r.msgs.contains { if case .log(.warn, let m) = $0 { m.contains("diarizer mic fell 5 s behind") } else { false } }, "\(r.msgs)")
        XCTAssertLessThan(r.stopMs, 2_000, "recording.stopped well inside the host's 10 s ack")
        XCTAssertTrue(r.msgs.contains { if case .recordingStopped = $0 { true } else { false } })
        // Turns already sent stay, in order and without gaps (no block was dropped), and none come after the ack.
        let sent = turns(r.msgs).filter { $0.0 == 0 }
        XCTAssertFalse(sent.isEmpty)
        for (i, t) in sent.enumerated() { XCTAssertEqual(t.1, Double(i) * 1000); XCTAssertEqual(t.2, Double(i + 1) * 1000) }
        XCTAssertLessThan(sent.count, 60, "stood down before the end")
        let stopped = r.msgs.firstIndex { if case .recordingStopped = $0 { true } else { false } }!
        XCTAssertTrue(r.msgs[stopped...].allSatisfy { if case .diarizationSegment = $0 { false } else { true } })
        // The model is still busy for a moment; nothing it finishes later is sent.
        let after = r.msgs.count
        try? await Task.sleep(nanoseconds: 400_000_000)
        XCTAssertEqual(turns(Array(r.msgs.prefix(after))).count, turns(r.msgs).count)
    }

    func testAStandDownSendsItsOpenTurnAndSaysFromWhenOn() async {
        // N126: same slow model; past the cap the tracker's open turn goes out once, then diarization.unavailable.
        let d = SlowDiarizer(appendNs: 100_000_000)
        let r = await record(d, seconds: 60, waitMs: 500, paceNs: 30_000_000)
        let un = r.msgs.compactMap { m -> (CaptureSource, Double, DiarizationUnavailableReason)? in
            if case .diarizationUnavailable(let s, let a, let w) = m { (s, a, w) } else { nil }
        }
        XCTAssertEqual(un.count, 1, "once")
        XCTAssertEqual(un.first?.0, .mic)
        XCTAssertEqual(un.first?.2, .backlog)
        XCTAssertEqual(d.abandoned.value, 1)
        let open = turns(r.msgs).filter { $0.0 == 1 }
        XCTAssertEqual(open.count, 1, "the open turn, sent at the stand-down")
        let i = r.msgs.firstIndex { if case .diarizationUnavailable = $0 { true } else { false } }!
        let j = r.msgs.firstIndex { if case .diarizationSegment(.mic, 1, _, _) = $0 { true } else { false } }!
        XCTAssertLessThan(j, i, "turns first, then the stand-down line")
        // atMs: the audio the model was handed (≥ what it processed, whole seconds of input, before the end).
        let at = un.first!.1
        XCTAssertGreaterThanOrEqual(at, open[0].2)
        XCTAssertEqual(at.truncatingRemainder(dividingBy: 1000), 0)
        XCTAssertLessThan(at, 60_000)
        XCTAssertTrue(r.msgs[(i + 1)...].allSatisfy { if case .diarizationSegment(.mic, _, _, _) = $0 { false } else { true } })
    }

    func testAModelThatDoesNotLoadSaysSoFromTheStart() async {
        let d = FailingDiarizer()
        let r = await record(d, seconds: 2, waitMs: 500)
        XCTAssertTrue(r.msgs.contains(.diarizationUnavailable(source: .mic, atMs: 0, reason: .modelUnavailable)))
        XCTAssertEqual(r.msgs.filter { if case .diarizationUnavailable = $0 { true } else { false } }.count, 1)
        XCTAssertTrue(turns(r.msgs).isEmpty)
    }

    func testASlowLoadIsBoundedAndTheStopClosesItOut() async {
        // The model takes 10 s to load: the queue fills to the cap, stands down, and stop doesn't wait for the load.
        let d = SlowDiarizer(prepareNs: 10_000_000_000)
        let r = await record(d, seconds: 20, waitMs: 300)
        XCTAssertLessThanOrEqual(r.peak, r.cap)
        XCTAssertLessThan(r.stopMs, 1_500, "stop waits for the diarizer at most diarizerStopWaitMs")
        XCTAssertTrue(r.msgs.contains { if case .recordingStopped = $0 { true } else { false } })
        XCTAssertTrue(turns(r.msgs).isEmpty, "nothing was processed, so no speaker turns")
        XCTAssertEqual(d.processed.value, 0)
    }

    func testAStopThatOutwaitsTheModelSendsItsOpenTurnsOnce() async {
        // Within the cap (4 s queued) but each block takes 400 ms: stop waits 300 ms, then closes the run out.
        let d = SlowDiarizer(appendNs: 400_000_000)
        let r = await record(d, seconds: 4, waitMs: 300)
        XCTAssertLessThan(r.stopMs, 1_500)
        XCTAssertEqual(d.abandoned.value, 1, "closed out once")
        XCTAssertFalse(r.msgs.contains { if case .log(.warn, let m) = $0 { m.contains("fell") } else { false } }, "not stood down")
        XCTAssertTrue(r.msgs.contains { if case .log(.warn, let m) = $0 { m.contains("still busy at stop") } else { false } })
        let stopped = r.msgs.firstIndex { if case .recordingStopped = $0 { true } else { false } }!
        XCTAssertTrue(r.msgs[stopped...].allSatisfy { if case .diarizationSegment = $0 { false } else { true } })
    }
}

final class ModelsWarmTests: XCTestCase {
    func testWarmRunsOnceInTheBackgroundAndNeverNextToARecording() async {
        let out = Box<[HelperMessage]>([])
        let runs = Box<Int>(0)
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [FakeAudio("sck")], transcribers: [FakeASR()],
                              helperVersion: "t", os: "linux", features: [])
        deps.modelWarmup = { runs.mutate { $0 += 1 }; try? await Task.sleep(nanoseconds: 50_000_000); return "models warmed: test" }
        let c = HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        let first = await c.warmModels()
        XCTAssertNotNil(first)
        await c.handle(.modelsWarm)  // while the first runs: ignored
        await first?.value
        XCTAssertEqual(runs.value, 1)
        XCTAssertTrue(out.value.contains(.log(level: .info, msg: "models warmed: test")))
        // During a recording: skipped (no second model next to the recording's).
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-warm-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        let skipped = await c.warmModels()
        XCTAssertNil(skipped)
        XCTAssertTrue(out.value.contains(.log(level: .info, msg: "models.warm skipped: recording")))
        await c.handle(.recordingStop())
        XCTAssertEqual(runs.value, 1)
        try? FileManager.default.removeItem(atPath: path)
    }
}

/// N125: a diarizer whose `append` blocks until released, logging prepare/append/unload in order, with a real
/// loaded flag (append without a model returns no turns, as `NemotronChannelDiarizer` does).
final class GatedDiarizer: ChannelDiarizer, @unchecked Sendable {
    let name = "gated-diar"
    let lock = NSLock()
    var log: [String] = []
    var loaded = false
    var gate: CheckedContinuation<Void, Never>?
    var blocking = true
    var waiting = false
    func note(_ e: String) { lock.withLock { log.append(e) } }
    func prepare() async throws { lock.withLock { loaded = true; log.append("prepare") } }
    func reset() async {}
    func append(_ s: [Float]) async -> [DiarizedTurn] {
        note("append")
        if lock.withLock({ blocking }) {
            await withCheckedContinuation { c in lock.withLock { gate = c; waiting = true } }
        }
        return lock.withLock { loaded } ? [DiarizedTurn(speaker: 0, startMs: 0, endMs: Double(s.count) / 16)] : []
    }
    func release() { let c = lock.withLock { () -> CheckedContinuation<Void, Never>? in blocking = false; defer { gate = nil }; return gate }; c?.resume() }
    var isWaiting: Bool { lock.withLock { waiting } }
    func finish() async -> [DiarizedTurn] { [] }
    func unload() async { lock.withLock { loaded = false; log.append("unload") } }
    func abandon() -> [DiarizedTurn] { [] }
}

final class DiarizerRunScopeTests: XCTestCase {
    func controller(_ d: ChannelDiarizer, out: Box<[HelperMessage]>) -> HelperController {
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [FakeAudio("sck")],
                              transcribers: [SlowASR("mic", words: "hi.", delayNs: 1_000_000)],
                              helperVersion: "t", os: "linux", features: [])
        deps.systemTranscribers = [SlowASR("sys", words: "yo.", delayNs: 1_000_000)]
        deps.diarizers = [.mic: d]
        deps.diarizerStopWaitMs = 200
        return HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
    }

    func waitFor(_ f: @escaping @Sendable () -> Bool) async {
        var n = 0
        while n < 200, !f() { try? await Task.sleep(nanoseconds: 10_000_000); n += 1 }
    }

    func path() -> String { FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-n125-\(UUID()).wav").path }

    func runBackToBack(abortFirst: Bool) async {
        let d = GatedDiarizer()
        let out = Box<[HelperMessage]>([])
        let c = controller(d, out: out)
        let loud = [Float](repeating: 0.2, count: 16_000)
        let p1 = path(), p2 = path()
        await c.handle(.recordingStart(audioPath: p1))
        await c.onSamples(loud, of: nil, channels: (system: loud, mic: loud))
        await waitFor { d.isWaiting }  // the first run is inside a model call
        if abortFirst { await c.handle(.recordingAbort(audioPath: p1)) } else { await c.handle(.recordingStop()) }  // closed out after 200 ms
        // The next meeting starts at once, then the old model call returns.
        await c.handle(.recordingStart(audioPath: p2))
        d.release()
        out.mutate { $0.removeAll() }
        for _ in 0..<3 { await c.onSamples(loud, of: nil, channels: (system: loud, mic: loud)) }
        await waitFor { d.lock.withLock { d.log.filter { $0 == "append" }.count } >= 4 }
        await c.handle(.recordingStop())
        let log = d.lock.withLock { d.log }
        let secondPrepare = log.lastIndex(of: "prepare")!
        let lastAppend = log.lastIndex(of: "append")!
        XCTAssertGreaterThan(lastAppend, secondPrepare)
        XCTAssertFalse(log[secondPrepare...lastAppend].contains("unload"), "the old run's unload landed under the new recording: \(log)")
        XCTAssertTrue(out.value.contains { if case .diarizationSegment(.mic, _, _, _) = $0 { true } else { false } }, "the second meeting has speakers")
        try? FileManager.default.removeItem(atPath: p1); try? FileManager.default.removeItem(atPath: p2)
    }

    func testAClosedOutRunNeverUnloadsTheNextRecordingsModel() async { await runBackToBack(abortFirst: false) }
    func testAnAbortedRunNeverUnloadsTheNextRecordingsModel() async { await runBackToBack(abortFirst: true) }
}

final class ModelsWarmCancelTests: XCTestCase {
    func testARecordingThatStartsDuringTheWarmCancelsItBetweenModels() async {
        // N127: three "models" of 200 ms each, checking isCancelled between them like mac warmModels.
        let out = Box<[HelperMessage]>([])
        let loaded = Box<[String]>([]), resident = Box<Int>(0)
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [FakeAudio("sck")], transcribers: [FakeASR()],
                              helperVersion: "t", os: "linux", features: [])
        deps.modelWarmup = {
            for m in ["asr", "diarizer", "echo"] {
                resident.mutate { $0 += 1 }
                try? await Task.sleep(nanoseconds: 200_000_000)
                loaded.mutate { $0.append(m) }
                resident.mutate { $0 -= 1 }  // unloaded right after its load
                if Task.isCancelled { return "models.warm cancelled after \(m)" }
            }
            return "models warmed: test"
        }
        let c = HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        let warm = await c.warmModels()
        try? await Task.sleep(nanoseconds: 50_000_000)  // inside the first model
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-n127-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        await warm?.value
        XCTAssertEqual(loaded.value, ["asr"], "stopped after the model it was in")
        XCTAssertEqual(resident.value, 0, "nothing left loaded next to the recording")
        XCTAssertTrue(out.value.contains(.log(level: .info, msg: "models.warm cancelled: recording")))
        XCTAssertTrue(out.value.contains(.log(level: .info, msg: "models.warm cancelled after asr")))
        XCTAssertFalse(out.value.contains(.log(level: .info, msg: "models warmed: test")))
        await c.handle(.recordingStop())
        // After the recording a new warm runs in full.
        let again = await c.warmModels()
        XCTAssertNotNil(again)
        await again?.value
        XCTAssertTrue(out.value.contains(.log(level: .info, msg: "models warmed: test")))
        try? FileManager.default.removeItem(atPath: path)
    }
}

