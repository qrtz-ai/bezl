import Foundation
import XCTest
@testable import QrtzHelperCore

final class FakeMic: MicSource, @unchecked Sendable {
    var cb: (@Sendable ([Float]) -> Void)?
    var starts = 0, stops = 0
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws { starts += 1; cb = onSamples }
    func stop() { stops += 1; cb = nil }
}

final class RecordingASR: StreamingTranscriber, @unchecked Sendable {
    let name = "rec"
    var received = 0
    func prepare() async throws {}
    func append(_ s: [Float]) async { received += s.count }
    func transcript() async -> String { received > 0 ? "hello world" : "" }
    func finish() async -> String { received > 0 ? "hello world" : "" }
    func reset() async { received = 0 }
}

final class FakeInsertion: InsertionBackend, @unchecked Sendable {
    var axSelected = true, axValue = true, canPaste = true
    var clipboard: String? = "user clipboard"
    var changes = 0
    var log: [String] = []
    var otherWriterDuringPaste = false
    func focusedAppName() -> String? { "Notes" }
    func axSetSelectedText(_ t: String) -> Bool { log.append("sel"); return axSelected }
    func axInsertIntoValue(_ t: String) -> Bool { log.append("val"); return axValue }
    func pasteboardSnapshot() -> Any? { clipboard }
    func pasteboardSetString(_ t: String) { clipboard = t; changes += 1 }
    func pasteboardRestore(_ s: Any?) { clipboard = s as? String; changes += 1; log.append("restore") }
    func pasteboardChangeCount() -> Int { changes }
    func postPaste() -> Bool {
        log.append("cmd-v")
        if otherWriterDuringPaste { clipboard = "copied meanwhile"; changes += 1 }
        return canPaste
    }
}

final class DictationTests: XCTestCase {
    func testPreRollKeepsOnlyTheLastWindow() {
        var b = PreRollBuffer(capacity: 4)
        b.append([1, 2, 3]); b.append([4, 5, 6])
        XCTAssertEqual(b.drain(), [3, 4, 5, 6])
        XCTAssertEqual(b.count, 0)
    }

    func testPCM16Base64() {
        XCTAssertEqual(PCM16.base64([0, 1, -1]), Data([0, 0, 0xFF, 0x7F, 0x01, 0x80]).base64EncodedString())
    }

    func testInsertionPrefersAXSelectedText() async {
        let be = FakeInsertion()
        let r = await TextInserter(backend: be, restoreDelayMs: 0).insert("Hi.", restoreClipboard: true)
        XCTAssertEqual(r.method, .axSelectedText); XCTAssertTrue(r.ok); XCTAssertEqual(r.app, "Notes")
        XCTAssertEqual(be.clipboard, "user clipboard")
    }

    func testInsertionFallsBackToValueThenPasteAndRestoresClipboard() async {
        let be = FakeInsertion(); be.axSelected = false
        var r = await TextInserter(backend: be, restoreDelayMs: 0).insert("Hi.", restoreClipboard: true)
        XCTAssertEqual(r.method, .axValue)
        be.axValue = false; be.log = []
        r = await TextInserter(backend: be, restoreDelayMs: 0).insert("Hi.", restoreClipboard: true)
        XCTAssertEqual(r.method, .paste); XCTAssertTrue(r.ok)
        XCTAssertEqual(be.log, ["sel", "val", "cmd-v", "restore"])
        XCTAssertEqual(be.clipboard, "user clipboard")
    }

    func testPasteDoesNotClobberAClipboardWrittenMeanwhile() async {
        let be = FakeInsertion(); be.axSelected = false; be.axValue = false; be.otherWriterDuringPaste = true
        let r = await TextInserter(backend: be, restoreDelayMs: 0).insert("Hi.", restoreClipboard: true)
        XCTAssertEqual(r.method, .paste)
        XCTAssertEqual(be.clipboard, "copied meanwhile")
    }

    func testPasteFailureReportsAndRestores() async {
        let be = FakeInsertion(); be.axSelected = false; be.axValue = false; be.canPaste = false
        let r = await TextInserter(backend: be, restoreDelayMs: 0).insert("Hi.", restoreClipboard: true)
        XCTAssertEqual(r.method, .none); XCTAssertFalse(r.ok)
        XCTAssertEqual(be.clipboard, "user clipboard")
    }

    func testDictationLifecycleWithWarmMicPreRollAndAudioStream() async throws {
        let out = Box<[HelperMessage]>([])
        let clock = Box<Double>(0)
        let mic = FakeMic(), asr = RecordingASR()
        let c = HelperController(
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux", features: [],
                             dictationMic: mic, dictationTranscribers: [asr], insertion: FakeInsertion()),
            send: { m in out.mutate { $0.append(m) } }, now: { clock.value })
        await c.handle(.dictationConfigure(warmMic: true, preRollMs: 100, streamAudio: true, pushKey: ""))
        XCTAssertEqual(mic.starts, 1)
        // Warm mic: audio before the key press lands in the pre-roll (only the last 100 ms = 1600 samples kept).
        await c.dictation.onSamples([Float](repeating: 0.1, count: 4000))
        let pre = await c.dictation.preRollSamples
        XCTAssertEqual(pre, 1600)
        await c.handle(.dictationStart(id: "d1"))
        XCTAssertTrue(out.value.contains(.dictationStarted(id: "d1", preRollMs: 100)))
        XCTAssertEqual(asr.received, 1600)  // pre-roll reached the recogniser
        clock.mutate { $0 += 100 }
        await c.dictation.onSamples([Float](repeating: 0.2, count: 1600))
        await c.dictation.poll()
        await c.handle(.dictationStop(id: "d1"))
        let msgs = out.value
        XCTAssertTrue(msgs.contains { if case .dictationAsr("d1", .partial(_, _, "hello world")) = $0 { true } else { false } })
        XCTAssertTrue(msgs.contains { if case .dictationAsr("d1", .final(_, _, _, "hello world", _)) = $0 { true } else { false } })
        XCTAssertTrue(msgs.contains(.dictationStopped(id: "d1", text: "hello world", durationMs: 200)))
        XCTAssertEqual(msgs.filter { if case .dictationAudio = $0 { true } else { false } }.count, 2)
        XCTAssertTrue(msgs.contains { if case .dictationLevel = $0 { true } else { false } })
        XCTAssertEqual(mic.stops, 0, "warm mic stays on between dictations")
        await c.handle(.insertText(id: "d1", text: "Hello world.", restoreClipboard: true))
        XCTAssertTrue(out.value.contains(.insertResult(id: "d1", method: .axSelectedText, ok: true, app: "Notes", error: nil)))
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: ""))
        XCTAssertEqual(mic.stops, 1)
    }

    func testColdMicStopsAfterDictationAndInsertUnsupportedWithoutBackend() async {
        let out = Box<[HelperMessage]>([])
        let mic = FakeMic()
        let c = HelperController(
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux", features: [],
                             dictationMic: mic, dictationTranscribers: [RecordingASR()]),
            send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        await c.handle(.dictationStart(id: "a"))
        XCTAssertEqual(mic.starts, 1)
        await c.handle(.dictationStop(id: "a"))
        XCTAssertEqual(mic.stops, 1)
        XCTAssertTrue(out.value.contains(.dictationStopped(id: "a", text: "hello world", durationMs: 0)), "only the pad reached it: no captured audio")
        await c.handle(.insertText(id: "a", text: "x", restoreClipboard: true))
        XCTAssertTrue(out.value.contains { if case .insertResult("a", .none, false, _, _) = $0 { true } else { false } })
    }
}

/// Like Nemotron: decodes only full 560 ms chunks (8960 samples). A chunk's words are its distinct non-zero levels
/// (level 0.1·k = word "wk"). `finish()` drops a partial chunk: a streaming model that needs audio after a word.
/// Counts any append that reaches it while a finish() runs (the shared-recogniser hazard).
final class ChunkASR: StreamingTranscriber, @unchecked Sendable {
    let name = "chunk"
    let loadsInstantly = true
    let lock = NSLock()
    var buf: [Float] = []
    var words: [String] = []
    var finishing = false
    var appendsWhileFinishing = 0
    var appended = 0
    var finishNs: UInt64
    let appendNs: UInt64
    init(finishNs: UInt64 = 0, appendNs: UInt64 = 0) { self.finishNs = finishNs; self.appendNs = appendNs }
    func prepare() async throws {}
    func append(_ s: [Float]) async {
        lock.withLock { if finishing { appendsWhileFinishing += 1 } }
        if appendNs > 0, s.contains(where: { $0 != 0 }) { try? await Task.sleep(nanoseconds: appendNs) }
        lock.withLock {
            if finishing { appendsWhileFinishing += 1 }
            appended += s.count
            buf += s
            while buf.count >= 8960 {
                for v in buf.prefix(8960) where v > 0 {
                    let w = "w\(Int((v * 10).rounded()))"
                    if words.last != w { words.append(w) }
                }
                buf.removeFirst(8960)
            }
        }
    }
    func transcript() async -> String { lock.withLock { words.joined(separator: " ") } }
    func finish() async -> String {
        let ns = lock.withLock { finishing = true; return finishNs }
        if ns > 0 { try? await Task.sleep(nanoseconds: ns) }
        return lock.withLock { finishing = false; buf.removeAll(); return words.joined(separator: " ") }
    }
    func reset() async { lock.withLock { buf.removeAll(); words.removeAll() } }
}

final class DictationTailTests: XCTestCase {
    func controller(_ asr: StreamingTranscriber, _ tail: DictationTail, mic: FakeMic = FakeMic()) -> (HelperController, Box<[HelperMessage]>) {
        let out = Box<[HelperMessage]>([])
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux",
                              features: [], dictationMic: mic, dictationTranscribers: [asr])
        deps.dictationTail = tail
        return (HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 }), out)
    }

    func stoppedText(_ msgs: [HelperMessage]) -> String? {
        msgs.compactMap { if case .dictationStopped(_, let t, _, _) = $0 { t } else { nil } }.first
    }

    func level(_ k: Int, _ n: Int) -> [Float] { [Float](repeating: Float(k) / 10, count: n) }

    func testTheLastWordInTheFinalPartialChunkIsInTheFinalText() async {
        // "w1" fills a chunk; "w2" (the last word) is only 250 ms: the final, partial chunk.
        let asr = ChunkASR()
        let (c, out) = controller(asr, DictationTail(tailMs: 0, padMs: 640))
        await c.handle(.dictationStart(id: "d1"))
        await c.dictation.onSamples(level(1, 8960))
        await c.dictation.onSamples(level(2, 4000))
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertEqual(stoppedText(out.value), "w1 w2", "the silence pad pushes the last partial chunk through")
        XCTAssertTrue(out.value.contains(.dictationStopped(id: "d1", text: "w1 w2", durationMs: (8960 + 4000) / 16)),
                      "the pad isn't counted as dictated audio")
    }

    func testWithoutThePadTheLastWordIsLost() async {
        // The bug: release → finish() right away, the partial chunk never decoded.
        let (c, out) = controller(ChunkASR(), DictationTail(tailMs: 0, padMs: 0))
        await c.handle(.dictationStart(id: "d1"))
        await c.dictation.onSamples(level(1, 8960))
        await c.dictation.onSamples(level(2, 4000))
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertEqual(stoppedText(out.value), "w1")
    }

    func testAudioAfterTheReleaseAndStillQueuedGetsIn() async {
        // The mic keeps delivering after fn-up: blocks already queued and the tail (the end of the last word).
        let mic = FakeMic()
        let (c, out) = controller(ChunkASR(), DictationTail(tailMs: 200, padMs: 640), mic: mic)
        await c.handle(.dictationStart(id: "d1"))
        guard let cb = mic.cb else { return XCTFail("mic attached") }
        cb(level(1, 8960))
        for _ in 0..<5 { cb(level(2, 1360)) }  // queued in the mic stream when the stop arrives
        let stop = Task { await c.handle(.dictationStop(id: "d1")) }
        try? await Task.sleep(nanoseconds: 80_000_000)
        cb(level(3, 1360))  // arrives 80 ms after the release
        await stop.value
        XCTAssertEqual(stoppedText(out.value), "w1 w2 w3")
    }

    func testASlowFinalizeIsWaitedForNotReplacedByThePartial() async {
        // A 400 ms finish against a 300 ms base: the limit scales with the dictation (+100 ms per second), so it is
        // waited for and its final is sent, not the partial.
        let asr = ChunkASR(finishNs: 400_000_000)
        let (c, out) = controller(asr, DictationTail(tailMs: 0, padMs: 0, finalizeTimeoutMs: 300))
        await c.handle(.dictationStart(id: "d1"))
        for _ in 0..<4 { await c.dictation.onSamples(level(1, 8000)) }  // 2 s of dictation: limit 300 + 200 ms
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertEqual(stoppedText(out.value), "w1")
        XCTAssertFalse(out.value.contains { if case .log(.warn, let m) = $0 { m.contains("not ready") } else { false } })
    }

    func testAHungFinalizeSendsThePartialAndHoldsTheRecogniserUntilItFinishes() async {
        let asr = ChunkASR(finishNs: 1_000_000_000)
        let (c, out) = controller(asr, DictationTail(tailMs: 0, padMs: 640, finalizeTimeoutMs: 150))
        await c.handle(.dictationStart(id: "d1"))
        await c.dictation.onSamples(level(1, 8960))
        await c.dictation.onSamples(level(2, 4000))
        try? await Task.sleep(nanoseconds: 250_000_000)  // a poll: the partial is "w1"
        let t = Date()
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertLessThan(Date().timeIntervalSince(t), 0.6, "bounded: 150 ms + 0.8 s / 10")
        XCTAssertEqual(stoppedText(out.value), "w1", "the last partial, read before the finish, not from the recogniser mid-finish")
        XCTAssertTrue(out.value.contains { if case .log(.warn, let m) = $0 { m.contains("not ready") } else { false } })
        // A re-press while that finish still runs never touches the recogniser: no reset, no append. With no other
        // recogniser it is refused (N134), never started deaf.
        await c.handle(.dictationStart(id: "d2"))
        await c.dictation.onSamples(level(3, 8960))
        XCTAssertTrue(out.value.contains { if case .log(.warn, let m) = $0 { m.contains("still finishing") } else { false } })
        XCTAssertTrue(out.value.contains { if case .error("dictation.asr", _, false, "d2", _, _, _) = $0 { true } else { false } })
        XCTAssertFalse(out.value.contains(.dictationStarted(id: "d2", preRollMs: 0)))
        XCTAssertEqual(asr.lock.withLock { asr.appendsWhileFinishing }, 0)
        // Once it returns, the next dictation has it again.
        try? await Task.sleep(nanoseconds: 1_000_000_000)
        asr.lock.withLock { asr.finishNs = 0 }
        out.mutate { $0.removeAll() }
        await c.handle(.dictationStart(id: "d3"))
        await c.dictation.onSamples(level(4, 8960))
        await c.handle(.dictationStop(id: "d3"))
        XCTAssertTrue(stoppedText(out.value)?.hasSuffix("w4") == true, "\(out.value)")  // after the refused press's pre-roll
    }

    func testNothingReachesTheRecogniserAfterTheDrain() async {
        // Gap 1: the mic keeps delivering while finish() runs; those blocks go to the pre-roll, never append().
        let mic = FakeMic()
        let asr = ChunkASR(finishNs: 400_000_000)
        let (c, out) = controller(asr, DictationTail(tailMs: 0, padMs: 640), mic: mic)
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 300, streamAudio: false, pushKey: "", consumeFn: true))
        await c.handle(.dictationStart(id: "d1"))
        guard let cb = mic.cb else { return XCTFail("mic attached") }
        cb(level(1, 8960))
        let stop = Task { await c.handle(.dictationStop(id: "d1")) }
        for _ in 0..<6 { try? await Task.sleep(nanoseconds: 50_000_000); cb(level(5, 1360)) }
        await stop.value
        XCTAssertEqual(asr.lock.withLock { asr.appendsWhileFinishing }, 0)
        // The first block the tap delivers after the release (the one it was filling) is in; the later ones aren't.
        XCTAssertEqual(stoppedText(out.value), "w1 w5")
        XCTAssertTrue(out.value.contains(.dictationStopped(id: "d1", text: "w1 w5", durationMs: (8960 + 1360) / 16)))
    }

    func testTheShortTailWaitsForTheBlockTheTapWasFillingAtTheRelease() async {
        // 50 ms tail; the tap's next block comes 90 ms after the release (≈ one 85 ms block): it carries the end of
        // the last word ("w7") and must reach the recogniser. Without the next-block wait it fell after the barrier.
        for wait in [120, 0] {
            let mic = FakeMic()
            let (c, out) = controller(ChunkASR(), DictationTail(tailMs: 50, padMs: 640, nextBlockWaitMs: wait), mic: mic)
            await c.handle(.dictationStart(id: "d1"))
            guard let cb = mic.cb else { return XCTFail("mic attached") }
            cb(level(1, 8960))
            let t = Date()
            let stop = Task { await c.handle(.dictationStop(id: "d1")) }
            try? await Task.sleep(nanoseconds: 90_000_000)
            cb(level(7, 1360))
            await stop.value
            let ms = Date().timeIntervalSince(t) * 1000
            if wait > 0 {
                XCTAssertEqual(stoppedText(out.value), "w1 w7")
                XCTAssertLessThan(ms, 400, "release to stopped: tail + one block + pad/finish")
            } else {
                XCTAssertEqual(stoppedText(out.value), "w1", "the control: the block after the barrier is lost")
            }
        }
    }

    func testABlockThatLandsDuringTheTailIsTheOneWaitedFor() async {
        // The tap's block lands 20 ms into the 50 ms tail: that is the block it was filling at the release, so the
        // drain doesn't wait another whole block (was: tail + up to one more block, ~135 ms on the Mac).
        let mic = FakeMic()
        let (c, out) = controller(ChunkASR(), DictationTail(tailMs: 50, padMs: 640, nextBlockWaitMs: 120), mic: mic)
        await c.handle(.dictationStart(id: "d1"))
        guard let cb = mic.cb else { return XCTFail("mic attached") }
        cb(level(1, 8960))
        let t = Date()
        let stop = Task { await c.handle(.dictationStop(id: "d1")) }
        try? await Task.sleep(nanoseconds: 20_000_000)
        cb(level(7, 1360))
        await stop.value
        let ms = Date().timeIntervalSince(t) * 1000
        XCTAssertEqual(stoppedText(out.value), "w1 w7")
        XCTAssertLessThan(ms, 50 + 100, "the tail, then pad/finish: no wait for a second block")
    }

    func testTheDrainLimitScalesToWhatIsQueued() async {
        // The recogniser takes 60 ms per 85 ms block; 8 blocks queued at the release need ~480 ms, far past the
        // 50 ms base, within 50 + 2 × 680 ms.
        let mic = FakeMic()
        let asr = ChunkASR(appendNs: 60_000_000)
        let (c, out) = controller(asr, DictationTail(tailMs: 0, drainTimeoutMs: 50, padMs: 640), mic: mic)
        await c.handle(.dictationStart(id: "d1"))
        guard let cb = mic.cb else { return XCTFail("mic attached") }
        for _ in 0..<8 { cb(level(6, 1360)) }
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertFalse(out.value.contains { if case .log(.warn, let m) = $0 { m.contains("not drained") } else { false } })
        XCTAssertTrue(out.value.contains(.dictationStopped(id: "d1", text: "w6", durationMs: 8 * 1360 / 16)))
    }

    func testABacklogPastTheCapStandsTheMicDownExplicitly() async {
        // Gap 3: no silent bufferingNewest drop. 300 ms cap: 3 blocks queue, the 4th stands the mic down.
        let mic = FakeMic()
        let asr = ChunkASR(appendNs: 100_000_000)
        let (c, out) = controller(asr, DictationTail(tailMs: 0, padMs: 0, backlogCapMs: 300), mic: mic)
        await c.handle(.dictationStart(id: "d1"))
        guard let cb = mic.cb else { return XCTFail("mic attached") }
        for _ in 0..<20 { cb(level(1, 1360)) }
        await c.handle(.dictationStop(id: "d1"))
        let errs = out.value.filter { if case .error(let code, _, _, _, _, _, _) = $0 { code == "dictation.backlog" } else { false } }
        XCTAssertEqual(errs.count, 1, "said once")
        XCTAssertLessThanOrEqual(asr.lock.withLock { asr.appended }, 4800, "nothing past the cap reached the recogniser")
        // The next dictation is re-armed.
        out.mutate { $0.removeAll() }
        await c.handle(.dictationStart(id: "d2"))
        mic.cb?(level(2, 1360))  // a fresh tap callback (the cold mic was released after d1)
        await c.handle(.dictationStop(id: "d2"))
        XCTAssertTrue(out.value.contains(.dictationStopped(id: "d2", text: "", durationMs: 85)))
    }

    func testCancelDuringTheTailAnswersOnce() async {
        let (c, out) = controller(ChunkASR(), DictationTail(tailMs: 300))
        await c.handle(.dictationStart(id: "d1"))
        let stop = Task { await c.handle(.dictationStop(id: "d1")) }
        try? await Task.sleep(nanoseconds: 50_000_000)
        await c.handle(.dictationCancel(id: "d1"))
        await stop.value
        XCTAssertEqual(out.value.filter { if case .dictationStopped = $0 { true } else { false } }.count, 1)
        XCTAssertEqual(stoppedText(out.value), "")
    }

    func testTheBudgetFitsTheHostsStopWait() {
        XCTAssertLessThan(DictationTail().budgetMs(), 3000, "qrtz-talk waits 3000 ms for dictation.stopped")
    }
}

/// N131: an `append` that blocks until released (any block with level `blockAt`, or every zero-only pad block when
/// `blockPad`); otherwise a ChunkASR. Counts appends that reach it after a close gave up on it.
final class BlockingASR: StreamingTranscriber, @unchecked Sendable {
    let name = "blocking"
    let loadsInstantly = true
    let inner = ChunkASR()
    let lock = NSLock()
    var gate: [CheckedContinuation<Void, Never>] = []
    var blocking = true
    var inFlight = 0
    var finishes = 0
    var prepareFails = false
    let blockAt: Float?, blockPad: Bool
    init(blockAt: Float? = nil, blockPad: Bool = false) { self.blockAt = blockAt; self.blockPad = blockPad }
    struct NoModel: Error {}
    func prepare() async throws { if lock.withLock({ prepareFails }) { throw NoModel() } }
    func append(_ s: [Float]) async {
        let hit = (blockAt.map { b in s.contains(b) } ?? false) || (blockPad && !s.isEmpty && s.allSatisfy { $0 == 0 })
        if hit, lock.withLock({ blocking }) {
            lock.withLock { inFlight += 1 }
            await withCheckedContinuation { c in lock.withLock { gate.append(c) } }
            lock.withLock { inFlight -= 1 }
        }
        await inner.append(s)
    }
    func release() { let g = lock.withLock { () -> [CheckedContinuation<Void, Never>] in blocking = false; defer { gate.removeAll() }; return gate }; for c in g { c.resume() } }
    func transcript() async -> String { await inner.transcript() }
    func finish() async -> String { lock.withLock { finishes += 1 }; return await inner.finish() }
    func reset() async { await inner.reset() }
}

final class DictationBoundsTests: XCTestCase {
    func controller(_ asrs: [StreamingTranscriber], _ tail: DictationTail, mic: FakeMic = FakeMic()) -> (HelperController, Box<[HelperMessage]>) {
        let out = Box<[HelperMessage]>([])
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux",
                              features: [], dictationMic: mic, dictationTranscribers: asrs)
        deps.dictationTail = tail
        return (HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 }), out)
    }
    func level(_ k: Int, _ n: Int) -> [Float] { [Float](repeating: Float(k) / 10, count: n) }
    func stoppedText(_ msgs: [HelperMessage], _ id: String) -> String? {
        msgs.compactMap { if case .dictationStopped(id, let t, _, _) = $0 { t } else { nil } }.first
    }
    func warned(_ msgs: [HelperMessage], _ s: String) -> Bool { msgs.contains { if case .log(.warn, let m) = $0 { m.contains(s) } else { false } } }

    func testAHungAppendAtTheReleaseIsBoundedAndHoldsTheRecogniser() async {
        // N131: the recogniser hangs inside an append (level 0.9) as fn goes up.
        let mic = FakeMic()
        let asr = BlockingASR(blockAt: 0.9)
        let spare = ChunkASR()
        let tail = DictationTail(tailMs: 50, drainTimeoutMs: 200, padMs: 640, finalizeTimeoutMs: 300)
        let (c, out) = controller([asr, spare], tail, mic: mic)
        await c.handle(.dictationStart(id: "d1"))
        guard let cb = mic.cb else { return XCTFail("mic attached") }
        cb(level(1, 8960))
        try? await Task.sleep(nanoseconds: 250_000_000)  // decoded, polled: the partial is "w1"
        cb(level(9, 1360))  // hangs
        cb(level(1, 1360))  // queued behind it
        try? await Task.sleep(nanoseconds: 20_000_000)
        let t = Date()
        await c.handle(.dictationStop(id: "d1"))
        let ms = Date().timeIntervalSince(t) * 1000
        let budget = Double(tail.budgetMs(audioMs: 700, queuedMs: 85))
        XCTAssertLessThan(ms, budget + 100, "dictation.stopped within the helper's budget")
        XCTAssertEqual(stoppedText(out.value, "d1"), "w1", "the last partial")
        XCTAssertTrue(warned(out.value, "append still running"))
        XCTAssertEqual(asr.lock.withLock { asr.finishes }, 0, "no pad, no finish on a recogniser mid-append")
        // The next dictation skips it (uses the spare) until that append returns.
        await c.handle(.dictationStart(id: "d2"))
        XCTAssertTrue(warned(out.value, "blocking still finishing"))
        mic.cb?(level(2, 8960))
        await c.handle(.dictationStop(id: "d2"))
        XCTAssertEqual(stoppedText(out.value, "d2"), "w2")
        asr.release()
        try? await Task.sleep(nanoseconds: 100_000_000)
        out.mutate { $0.removeAll() }
        await c.handle(.dictationStart(id: "d3"))
        XCTAssertFalse(warned(out.value, "still finishing"), "released once its append returned")
        await c.handle(.dictationStop(id: "d3"))
    }

    func testASlowAppendAfterADrainTimeoutIsAwaitedAndFinalized() async {
        // N139: 200 ms per 85 ms block (under half real time) with ~0.7 s queued at the release: the drain times out
        // while an append is in flight. close() now has the release deadline minus a reserve, so that append is
        // awaited and pad + finish run: the text has the last words, not the last partial.
        let mic = FakeMic()
        let asr = ChunkASR(appendNs: 200_000_000)
        let tail = DictationTail(tailMs: 0, drainTimeoutMs: 100, padMs: 640, finalizeTimeoutMs: 300)
        let (c, out) = controller([asr], tail, mic: mic)
        await c.handle(.dictationStart(id: "d1"))
        guard let cb = mic.cb else { return XCTFail("mic attached") }
        cb(level(1, 8960))
        try? await Task.sleep(nanoseconds: 350_000_000)  // decoded and polled: the partial is "w1"
        for _ in 0..<8 { cb(level(5, 1360)) }
        try? await Task.sleep(nanoseconds: 20_000_000)
        let t = Date()
        await c.handle(.dictationStop(id: "d1"))
        let ms = Date().timeIntervalSince(t) * 1000
        XCTAssertLessThan(ms, Double(tail.budgetMs(audioMs: 560 + 680, queuedMs: 680)) + 150, "within the helper's budget")
        XCTAssertFalse(warned(out.value, "append still running"), "the slow append was awaited")
        XCTAssertEqual(stoppedText(out.value, "d1"), "w1 w5", "finish(), with the words after the partial")
    }

    func testAHungPadIsInsideTheFinalizeDeadline() async {
        // N131: the pad's own append hangs; the deadline started before it, so the stop still answers in time.
        let asr = BlockingASR(blockPad: true)
        let tail = DictationTail(tailMs: 0, drainTimeoutMs: 100, padMs: 640, finalizeTimeoutMs: 200)
        let (c, out) = controller([asr], tail)
        await c.handle(.dictationStart(id: "d1"))
        await c.dictation.onSamples(level(1, 8960))
        try? await Task.sleep(nanoseconds: 250_000_000)
        let t = Date()
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertLessThan(Date().timeIntervalSince(t) * 1000, Double(tail.budgetMs(audioMs: 560)) + 100)
        XCTAssertEqual(stoppedText(out.value, "d1"), "w1")
        XCTAssertTrue(warned(out.value, "final text not ready"))
        await c.handle(.dictationStart(id: "d2"))  // held until the pad and finish return: refused (N134)
        XCTAssertTrue(out.value.contains { if case .error("dictation.asr", _, false, "d2", _, _, _) = $0 { true } else { false } })
        asr.release()
    }

    func testAStartWithNoRecogniserIsRefusedNotDeaf() async {
        // N134: every recogniser fails prepare (or is busy): error{dictation.asr} for that id, no started, mic released.
        let mic = FakeMic()
        let a = BlockingASR(), b = BlockingASR()
        a.prepareFails = true; b.prepareFails = true
        let (c, out) = controller([a, b], DictationTail(tailMs: 0), mic: mic)
        await c.handle(.dictationStart(id: "d1"))
        XCTAssertTrue(out.value.contains { if case .error("dictation.asr", let m, false, "d1", _, _, _) = $0 { m.contains("ready") } else { false } })
        XCTAssertFalse(out.value.contains { if case .dictationStarted = $0 { true } else { false } })
        let active = await c.dictation.isActive
        XCTAssertFalse(active)
        XCTAssertEqual(mic.stops, mic.starts, "the mic reader is released")
        // Once one loads again, the next press works.
        b.lock.withLock { b.prepareFails = false }
        await c.handle(.dictationStart(id: "d2"))
        XCTAssertTrue(out.value.contains { if case .dictationStarted("d2", _) = $0 { true } else { false } })
        await c.handle(.dictationStop(id: "d2"))
        XCTAssertNotNil(stoppedText(out.value, "d2"))
    }
}
