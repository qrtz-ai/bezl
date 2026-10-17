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
