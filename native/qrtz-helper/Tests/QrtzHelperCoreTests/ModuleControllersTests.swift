import Foundation
import XCTest
@testable import QrtzHelperCore

/// ADR 0146 M2: the monolith wrapper hands recordings to nano's controller and dictation to echo's; each runs on its
/// own (as qrtz-nano / qrtz-echo will) without the other.
final class ModuleControllersTests: XCTestCase {
    func deps(_ audio: [AudioSource]) -> HelperDeps {
        var d = HelperDeps(permissions: FakePerms(), meetings: nil, audio: audio, transcribers: [FakeASR()], helperVersion: "t", os: "linux", features: [])
        d.preloadDictationAsr = false
        return d
    }

    /// Nano alone records a file end to end, with no dictation engine anywhere.
    func testNanoRecordsOnItsOwn() async throws {
        let out = Box<[HelperMessage]>([])
        let src = FakeAudio("tap")
        let nano = NanoController(deps: deps([src]), send: { m in out.mutate { $0.append(m) } }, now: { 0 },
                                  speech: SpeechSinks(), echoReference: EchoReference())
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-nano-\(UUID()).wav").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        let failed = await nano.startRecording(path)
        XCTAssertNil(failed)
        let recording = await nano.isRecording
        XCTAssertTrue(recording)
        src.cb?(CaptureBlock(system: [Float](repeating: 0.1, count: 1600), mic: nil))
        await nano.stopRecording()
        let stopped = await nano.isRecording
        XCTAssertFalse(stopped)
        XCTAssertTrue(out.value.contains(.recordingStarted(audioPath: path)))
        XCTAssertTrue(out.value.contains { if case .recordingStopped(let p, _) = $0 { p == path } else { false } })
    }

    /// A file nano can't open is reported back (the arbiter, which nano doesn't own, releases the admission).
    func testNanoReportsAnUnopenableFile() async {
        let nano = NanoController(deps: deps([FakeAudio("tap")]), send: { _ in }, now: { 0 },
                                  speech: SpeechSinks(), echoReference: EchoReference())
        let err = await nano.startRecording("/nonexistent-dir-\(UUID())/x.wav")
        XCTAssertNotNil(err)
        let recording = await nano.isRecording
        XCTAssertFalse(recording)
    }

    /// The wrapper routes: a recording lives in nano, the dictation engine is echo's, and both share one arbiter.
    func testWrapperRoutesToTheModuleControllers() async throws {
        let out = Box<[HelperMessage]>([])
        let c = HelperController(deps: deps([FakeAudio("tap")]), send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        XCTAssertTrue(c.dictation === c.echo.dictation)
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-route-\(UUID()).wav").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        await c.handle(.recordingStart(audioPath: path))
        let inNano = await c.nano.isRecording
        XCTAssertTrue(inNano)
        let admitted = await c.arbiter.recording
        XCTAssertTrue(admitted)
        await c.handle(.recordingStop())
        let after = await c.nano.isRecording
        XCTAssertFalse(after)
        let released = await c.arbiter.recording
        XCTAssertFalse(released)
    }

    /// Echo with no arbiter bound refuses to take the mic (a standalone echo binds its own).
    func testUnboundAdmitRefuses() async {
        let relay = AdmitRelay()
        let ok = await relay(.configure(warmMic: true))
        XCTAssertFalse(ok)
    }
}
