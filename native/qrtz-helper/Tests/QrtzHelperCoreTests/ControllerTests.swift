import Foundation
import XCTest
@testable import QrtzHelperCore

final class Box<T>: @unchecked Sendable { var v: T; let lock = NSLock(); init(_ v: T) { self.v = v }
    func mutate(_ f: (inout T) -> Void) { lock.lock(); f(&v); lock.unlock() }
    var value: T { lock.lock(); defer { lock.unlock() }; return v } }

struct FakePerms: PermissionsProvider {
    func status() -> [OsPermission: PermissionState] { [.accessibility: .granted, .microphone: .granted, .screenRecording: .granted] }
    func request(_ p: OsPermission) async {}
}

final class FakeAudio: AudioSource, @unchecked Sendable {
    let name: String; let fails: Bool
    var cb: (@Sendable (CaptureBlock) -> Void)?
    init(_ name: String, fails: Bool = false) { self.name = name; self.fails = fails }
    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void) async throws {
        if fails { throw ProtocolError("denied") }
        cb = onBlock
    }
    func stop() async { cb = nil }
}

final class CaptureFallbackTests: XCTestCase {
    /// H8: every source fails: one capture.fallback per source, the last with to = nil, then audio.unavailable.
    func testLastSourceFailingFallsBackToNone() async throws {
        let out = Box<[HelperMessage]>([])
        let c = HelperController(
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: [FakeAudio("tap", fails: true), FakeAudio("sck", fails: true)],
                             transcribers: [FakeASR()], helperVersion: "t", os: "linux", features: []),
            send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-fb-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        let fbs = out.value.filter { if case .captureFallback = $0 { true } else { false } }
        XCTAssertEqual(fbs, [.captureFallback(from: "tap", to: "sck", reason: "denied"), .captureFallback(from: "sck", to: nil, reason: "denied")])
        XCTAssertTrue(out.value.contains { if case .error(let code, _, _, _, _, _, _) = $0 { code == "audio.unavailable" } else { false } })
        XCTAssertTrue(out.value.contains(.recordingStarted(audioPath: path)))
        await c.handle(.recordingStop())
        try? FileManager.default.removeItem(atPath: path)
    }
}

final class FakeASR: StreamingTranscriber, @unchecked Sendable {
    let name = "fake"
    var text = ""
    func prepare() async throws {}
    func append(_ s: [Float]) async { text = "Hello from the fake recogniser." }
    func transcript() async -> String { text }
    func finish() async -> String { text }
    func reset() async { text = "" }
}

final class ControllerTests: XCTestCase {
    func testRecordingLifecycleWithAudioFallback() async throws {
        let out = Box<[HelperMessage]>([])
        let clock = Box<Double>(1000)
        let primary = FakeAudio("tap", fails: true), fallback = FakeAudio("sck")
        let c = HelperController(
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: [primary, fallback], transcribers: [FakeASR()], helperVersion: "t", os: "linux", features: []),
            send: { m in out.mutate { $0.append(m) } }, now: { clock.value })
        await c.start()
        await c.handle(.ping(id: "x"))
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        XCTAssertNotNil(fallback.cb)
        await c.onSamples([Float](repeating: 0.2, count: 16_000))
        clock.mutate { $0 += 1000 }
        await c.handle(.recordingStop())
        let msgs = out.value
        XCTAssertEqual(msgs.first.map { if case .hello = $0 { true } else { false } }, true)
        XCTAssertTrue(msgs.contains(.pong(id: "x")))
        XCTAssertTrue(msgs.contains(.recordingStarted(audioPath: path)))
        // H8: the fallback is a structured message, before recording.started, naming the next source.
        let fb = msgs.firstIndex(of: .captureFallback(from: "tap", to: "sck", reason: "denied"))
        XCTAssertNotNil(fb)
        XCTAssertLessThan(fb!, msgs.firstIndex(of: .recordingStarted(audioPath: path))!)
        XCTAssertFalse(msgs.contains { if case .log(_, let m) = $0 { m.contains("tap failed") } else { false } }, "no log line any more")
        XCTAssertTrue(msgs.contains(.recordingStopped(audioPath: path, durationMs: 1000)))
        XCTAssertTrue(msgs.contains { if case .asr(.final(_, _, _, "Hello from the fake recogniser.", _)) = $0 { true } else { false } })
        XCTAssertTrue(msgs.contains { if case .audioLevel = $0 { true } else { false } })
        let isRec = await c.isRecording
        XCTAssertFalse(isRec)
    }

    func testPauseSendsNothingToRecogniserButKeepsTimeline() async throws {
        let out = Box<[HelperMessage]>([])
        let clock = Box<Double>(0)
        let asr = FakeASR()
        let c = HelperController(
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: [FakeAudio("sck")], transcribers: [asr], helperVersion: "t", os: "linux", features: []),
            send: { m in out.mutate { $0.append(m) } }, now: { clock.value })
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        await c.handle(.recordingPause)
        await c.onSamples([Float](repeating: 0.2, count: 16_000))
        XCTAssertEqual(asr.text, "")
        await c.handle(.recordingResume)
        await c.onSamples([Float](repeating: 0.2, count: 16_000))
        XCTAssertEqual(asr.text, "Hello from the fake recogniser.")
        clock.mutate { $0 += 2000 }
        await c.handle(.recordingStop())
        XCTAssertTrue(out.value.contains(.recordingStopped(audioPath: path, durationMs: 2000)))
        XCTAssertEqual(try HostMessage(json: HostMessage.recordingPause.json), .recordingPause)
    }
}

/// N35: a source whose in-source mic fails (reports `no-mic`) and keeps recording system audio; `failsAfter` makes it
/// throw after reporting, like a source that gives up entirely.
final class MicFailingAudio: AudioSource, @unchecked Sendable {
    let name: String; let failsAfter: Bool
    var degrade: (@Sendable (RevocationDegradation, String) -> Void)?
    init(_ name: String, failsAfter: Bool = false) { self.name = name; self.failsAfter = failsAfter }
    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void) async throws {
        XCTFail("the controller starts sources with onDegraded")
    }
    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void,
               onDegraded: @escaping @Sendable (RevocationDegradation, String) -> Void) async throws {
        degrade = onDegraded
        onDegraded(.noMic, "mic unavailable, recording system audio only: device busy")
        onDegraded(.noMic, "mic unavailable, recording system audio only: device busy")
        if failsAfter { throw ProtocolError("denied") }
    }
    func stop() async {}
}

final class CaptureDegradedTests: XCTestCase {
    private func run(_ audio: [AudioSource], _ body: (HelperController, String, Box<[HelperMessage]>) async throws -> Void) async throws {
        let out = Box<[HelperMessage]>([])
        let c = HelperController(
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: audio, transcribers: [FakeASR()], helperVersion: "t", os: "linux", features: []),
            send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-deg-\(UUID()).wav").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        try await body(c, path, out)
    }
    private func degraded(_ ms: [HelperMessage]) -> [HelperMessage] {
        ms.filter { if case .captureDegraded = $0 { true } else { false } }
    }
    private let expected = HelperMessage.captureDegraded(degraded: [.noMic], reason: "mic unavailable, recording system audio only: device busy")

    /// The in-source mic failing at start: one capture.degraded{no-mic}, after recording.started, no stderr-only line.
    func testInSourceMicFailureIsSentOnceAfterRecordingStarted() async throws {
        let src = MicFailingAudio("tap")
        try await run([src]) { c, path, out in
            await c.handle(.recordingStart(audioPath: path))
            let ms = out.value
            XCTAssertEqual(degraded(ms), [expected])
            XCTAssertLessThan(ms.firstIndex(of: .recordingStarted(audioPath: path))!, ms.firstIndex(of: expected)!)
            // Later in the same recording: still once per recording.
            src.degrade?(.noMic, "again")
            try await Task.sleep(nanoseconds: 50_000_000)
            XCTAssertEqual(degraded(out.value), [expected])
            await c.handle(.recordingStop())
            // After the stop: nothing (only while recording).
            src.degrade?(.noMic, "late")
            try await Task.sleep(nanoseconds: 50_000_000)
            XCTAssertEqual(degraded(out.value), [expected])
            // The next recording reports its own failure again.
            await c.handle(.recordingStart(audioPath: path + ".2"))
            XCTAssertEqual(degraded(out.value), [expected, expected])
            await c.handle(.recordingStop())
            try? FileManager.default.removeItem(atPath: path + ".2")
        }
    }

    /// A source that reports then fails to start never degraded the recording: only its capture.fallback counts.
    func testReportsOfASourceThatFailedToStartAreDropped() async throws {
        let bad = MicFailingAudio("tap", failsAfter: true)
        try await run([bad, FakeAudio("sck")]) { c, path, out in
            await c.handle(.recordingStart(audioPath: path))
            XCTAssertEqual(degraded(out.value), [])
            XCTAssertTrue(out.value.contains(.captureFallback(from: "tap", to: "sck", reason: "denied")))
            bad.degrade?(.noMic, "late")
            try await Task.sleep(nanoseconds: 50_000_000)
            XCTAssertEqual(degraded(out.value), [])
            await c.handle(.recordingStop())
        }
    }

    /// A late report for an aborted recording is dropped too.
    func testNothingAfterAbort() async throws {
        let src = MicFailingAudio("tap")
        try await run([src]) { c, path, out in
            await c.handle(.recordingStart(audioPath: path))
            await c.handle(.recordingAbort(audioPath: path))
            src.degrade?(.noMic, "late")
            try await Task.sleep(nanoseconds: 50_000_000)
            XCTAssertEqual(degraded(out.value), [expected])
        }
    }

    /// A source without in-source inputs keeps working through the default `start(onBlock:onDegraded:)`.
    func testPlainSourcesUseTheDefault() async throws {
        let plain = FakeAudio("sck")
        try await run([plain]) { c, path, out in
            await c.handle(.recordingStart(audioPath: path))
            XCTAssertNotNil(plain.cb)
            XCTAssertEqual(degraded(out.value), [])
            await c.handle(.recordingStop())
        }
    }
}
