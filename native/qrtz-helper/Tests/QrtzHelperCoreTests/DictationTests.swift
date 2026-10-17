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
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux", features: [], dictationMic: mic),
            send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        await c.handle(.dictationStart(id: "a"))
        XCTAssertEqual(mic.starts, 1)
        await c.handle(.dictationStop(id: "a"))
        XCTAssertEqual(mic.stops, 1)
        XCTAssertTrue(out.value.contains(.dictationStopped(id: "a", text: "", durationMs: 0)))
        await c.handle(.insertText(id: "a", text: "x", restoreClipboard: true))
        XCTAssertTrue(out.value.contains { if case .insertResult("a", .none, false, _, _) = $0 { true } else { false } })
    }
}

/// Like Nemotron: decodes only full 560 ms chunks (8960 samples). A chunk's words are its distinct non-zero levels
/// (level 0.1·k = word "wk"). `finish()` drops a partial chunk: a streaming model that needs audio after a word.
final class ChunkASR: StreamingTranscriber, @unchecked Sendable {
    let name = "chunk"
    let loadsInstantly = true
    let lock = NSLock()
    var buf: [Float] = []
    var words: [String] = []
    let finishNs: UInt64
    init(finishNs: UInt64 = 0) { self.finishNs = finishNs }
    func prepare() async throws {}
    func append(_ s: [Float]) async {
        lock.withLock {
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
        if finishNs > 0 { try? await Task.sleep(nanoseconds: finishNs) }
        return lock.withLock { buf.removeAll(); return words.joined(separator: " ") }
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

    func testASlowFinalizeIsBoundedAndUsesThePartial() async {
        let asr = ChunkASR(finishNs: 5_000_000_000)
        let (c, out) = controller(asr, DictationTail(tailMs: 0, padMs: 640, finalizeTimeoutMs: 200))
        await c.handle(.dictationStart(id: "d1"))
        await c.dictation.onSamples(level(1, 8960))
        await c.dictation.onSamples(level(2, 4000))
        let t = Date()
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertLessThan(Date().timeIntervalSince(t), 1.0)
        XCTAssertEqual(stoppedText(out.value), "w1 w2", "the pad already decoded the last chunk; the partial has it")
        XCTAssertTrue(out.value.contains { if case .log(.warn, let m) = $0 { m.contains("final text not ready") } else { false } })
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
        XCTAssertLessThan(DictationTail().budgetMs, 3000, "qrtz-talk waits 3000 ms for dictation.stopped")
    }
}
