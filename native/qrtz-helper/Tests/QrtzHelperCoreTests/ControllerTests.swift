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
    var cb: (@Sendable ([Float]) -> Void)?
    init(_ name: String, fails: Bool = false) { self.name = name; self.fails = fails }
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) async throws {
        if fails { throw ProtocolError("denied") }
        cb = onSamples
    }
    func stop() async { cb = nil }
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
        await c.handle(.recordingStop)
        let msgs = out.value
        XCTAssertEqual(msgs.first.map { if case .hello = $0 { true } else { false } }, true)
        XCTAssertTrue(msgs.contains(.pong(id: "x")))
        XCTAssertTrue(msgs.contains(.recordingStarted(audioPath: path)))
        XCTAssertTrue(msgs.contains(.recordingStopped(audioPath: path, durationMs: 1000)))
        XCTAssertTrue(msgs.contains { if case .asr(.final(_, _, _, "Hello from the fake recogniser.", _)) = $0 { true } else { false } })
        XCTAssertTrue(msgs.contains { if case .audioLevel = $0 { true } else { false } })
        XCTAssertTrue(msgs.contains(.log(level: .warn, msg: "audio tap failed: denied")))
        let isRec = await c.isRecording
        XCTAssertFalse(isRec)
    }
}
