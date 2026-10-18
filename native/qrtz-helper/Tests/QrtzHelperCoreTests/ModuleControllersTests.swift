import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// ADR 0146 M2 / M2.5: the monolith runs nano's and echo's controllers behind one host; qrtz-nano and qrtz-echo run
/// one each behind a host of their own, with its own arbiter.
final class ModuleControllersTests: XCTestCase {
    func deps(_ audio: [AudioSource]) -> HelperDeps {
        var d = HelperDeps(permissions: FakePerms(), meetings: nil, audio: audio, transcribers: [FakeASR()], helperVersion: "t", os: "linux", features: [])
        d.preloadDictationAsr = false
        return d
    }

    func hostDeps(_ module: String) -> HostDeps {
        HostDeps(permissions: FakePerms(), permissionPollMs: 0, helperVersion: "t", os: "linux", features: [module], module: module)
    }

    /// qrtz-nano: nano's controller alone behind its own host.
    func nanoApp(_ audio: [AudioSource], out: Box<[HelperMessage]>) -> (ModuleHost, NanoController) {
        let link = HostLink(send: { m in out.mutate { $0.append(m) } })
        let speech = SpeechSinks()
        let nano = NanoController(deps: NanoDeps(audio: audio, transcribers: [FakeASR()]), link: link, now: { 0 }, speech: speech,
                                  echoReference: EchoReference())
        return (ModuleHost(deps: hostDeps("nano"), link: link, now: { 0 }, speech: speech, parts: [nano]), nano)
    }

    /// qrtz-echo: echo's controller alone behind its own host.
    func echoApp(_ asr: StreamingTranscriber, mic: MicSource, out: Box<[HelperMessage]>) -> (ModuleHost, EchoController) {
        let link = HostLink(send: { m in out.mutate { $0.append(m) } })
        let speech = SpeechSinks()
        let echo = EchoController(deps: EchoDeps(dictationMic: mic, dictationTranscribers: [asr], insertion: FakeInsertion(), preloadDictationAsr: false),
                                  link: link, now: { 0 }, speech: speech, echoReference: EchoReference(), permissions: FakePerms(), ownsWarm: true)
        return (ModuleHost(deps: hostDeps("echo"), link: link, now: { 0 }, speech: speech, parts: [echo]), echo)
    }

    func tmp(_ tag: String) -> String {
        FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-\(tag)-\(UUID()).wav").path
    }

    /// qrtz-nano records a file end to end through its own arbiter, with no dictation engine anywhere.
    func testNanoAppRecordsOnItsOwn() async throws {
        let out = Box<[HelperMessage]>([])
        let src = FakeAudio("tap")
        let (host, nano) = nanoApp([src], out: out)
        await host.start()
        let path = tmp("nano")
        defer { try? FileManager.default.removeItem(atPath: path) }
        await host.handle(.recordingStart(audioPath: path))
        let recording = await nano.isRecording
        XCTAssertTrue(recording)
        let admitted = await host.arbiter.recording
        XCTAssertTrue(admitted)
        src.cb?(CaptureBlock(system: [Float](repeating: 0.1, count: 1600), mic: nil))
        await host.handle(.recordingStop())
        let stopped = await nano.isRecording
        XCTAssertFalse(stopped)
        let released = await host.arbiter.recording
        XCTAssertFalse(released)
        XCTAssertEqual(out.value.first.map { if case .hello(_, _, _, let f) = $0 { f } else { [] } }, ["nano"])
        XCTAssertTrue(out.value.contains(.recordingStarted(audioPath: path)))
        XCTAssertTrue(out.value.contains { if case .recordingStopped(let p, _) = $0 { p == path } else { false } })
    }

    /// A file nano can't open releases its own arbiter's admission and names the start that died.
    func testNanoAppReleasesAnUnopenableFile() async {
        let out = Box<[HelperMessage]>([])
        let (host, nano) = nanoApp([FakeAudio("tap")], out: out)
        let path = "/nonexistent-dir-\(UUID())/x.wav"
        await host.handle(.recordingStart(audioPath: path))
        let recording = await nano.isRecording
        XCTAssertFalse(recording)
        let admitted = await host.arbiter.recording
        XCTAssertFalse(admitted)
        XCTAssertTrue(out.value.contains { if case .error("recording.file", _, _, _, _, let p, _) = $0 { p == path } else { false } })
    }

    /// qrtz-nano isn't echo: a dictation reaching it (a host routing bug) is logged and left alone.
    func testNanoAppLeavesDictationAlone() async {
        let out = Box<[HelperMessage]>([])
        let (host, _) = nanoApp([FakeAudio("tap")], out: out)
        await host.handle(.dictationStart(id: "d1"))
        let dictating = await host.arbiter.dictating
        XCTAssertFalse(dictating)
        XCTAssertTrue(out.value.contains(.log(level: .warn, msg: "nano: dictation.start isn't this app's; ignored")))
    }

    /// qrtz-echo dictates through its own arbiter, and a recording reaching it is left alone.
    func testEchoAppDictatesOnItsOwn() async {
        let out = Box<[HelperMessage]>([])
        let asr = RecordingASR(), mic = FakeMic()
        let (host, echo) = echoApp(asr, mic: mic, out: out)
        await host.start()
        await host.handle(.dictationStart(id: "d1"))
        let dictating = await host.arbiter.dictating
        XCTAssertTrue(dictating)
        await echo.dictation.onSamples([Float](repeating: 0.2, count: 3200))
        await host.handle(.dictationStop(id: "d1"))
        let after = await host.arbiter.dictating
        XCTAssertFalse(after)
        XCTAssertTrue(out.value.contains { if case .dictationStopped("d1", "hello world", _, _, _) = $0 { true } else { false } })
        let path = tmp("echo")
        await host.handle(.recordingStart(audioPath: path))
        let recording = await host.arbiter.recording
        XCTAssertFalse(recording)
        XCTAssertFalse(FileManager.default.fileExists(atPath: path))
        XCTAssertTrue(out.value.contains(.log(level: .warn, msg: "echo: recording.start isn't this app's; ignored")))
    }

    /// N174: qrtz-echo answers `models.warm` itself by prewarming its recogniser (when a push key is set), so the
    /// first press after a download is warm; there is no nano in its app to warm first.
    func testEchoAppPrewarmsOnModelsWarm() async throws {
        let out = Box<[HelperMessage]>([])
        let asr = SlowLoadASR(loadMs: 10)
        let (host, _) = echoApp(asr, mic: FakeMic(), out: out)
        await host.start()
        await host.handle(.modelsWarm)
        try await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertEqual(asr.loads, 0, "no push key: nothing to prewarm")
        await host.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "fn"))
        await host.handle(.modelsWarm)
        for _ in 0..<50 where !out.value.contains(where: { if case .log(_, let m) = $0 { m.hasPrefix("echo asr: slow resident") } else { false } }) {
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTAssertTrue(out.value.contains { if case .log(_, let m) = $0 { m.hasPrefix("echo asr: slow resident") } else { false } },
                      "the prewarm says the recogniser is resident")
        XCTAssertEqual(asr.loads, 1, "the configure's prewarm and the warm's share one load")
    }

    /// N174: in the monolith nano owns `models.warm` and its warm (fresh instances) never touches echo's resident
    /// recogniser; a recording's unload doesn't either.
    func testEchoModelsStayResidentAcrossNanosWarmAndUnload() async throws {
        let echoAsr = SlowLoadASR(loadMs: 5), recAsr = SlowLoadASR(loadMs: 5)
        let src = FakeAudio("tap")
        var d = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [src], transcribers: [recAsr], helperVersion: "t", os: "linux",
                           features: [], dictationMic: FakeMic(), dictationTranscribers: [echoAsr], insertion: FakeInsertion())
        d.modelWarmup = { "models warmed: fake" }
        let out = Box<[HelperMessage]>([])
        let c = HelperController(deps: d, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        await c.start()
        await (await c.startupPreload)?.value
        XCTAssertEqual(echoAsr.loads, 1)
        await (await c.warmModels())?.value
        let path = tmp("resident")
        defer { try? FileManager.default.removeItem(atPath: path) }
        await c.handle(.recordingStart(audioPath: path))
        src.cb?(CaptureBlock(system: [Float](repeating: 0.1, count: 1600), mic: nil))
        await c.handle(.recordingStop())
        XCTAssertEqual(echoAsr.unloads, 0)
        XCTAssertEqual(echoAsr.loads, 1)
        XCTAssertTrue(out.value.contains(.log(level: .info, msg: "models warmed: fake")))
    }

    /// The monolith routes: a recording lives in nano, the dictation engine is echo's, and both share one arbiter.
    func testWrapperRoutesToTheModuleControllers() async throws {
        let out = Box<[HelperMessage]>([])
        let c = HelperController(deps: deps([FakeAudio("tap")]), send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        XCTAssertTrue(c.dictation === c.echo.dictation)
        let path = tmp("route")
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

    /// A part whose link was never bound to a host refuses to take the mic.
    func testUnboundLinkRefuses() async {
        let link = HostLink(send: { _ in })
        let ok = await link.admit(.configure(warmMic: true))
        XCTAssertFalse(ok)
    }
}
