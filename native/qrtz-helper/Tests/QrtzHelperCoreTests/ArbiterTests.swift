import Foundation
import XCTest
@testable import QrtzHelperCore

/// Mutable TCC state, so a test can revoke a permission mid-capture.
final class MutablePerms: PermissionsProvider, @unchecked Sendable {
    let st = Box<[OsPermission: PermissionState]>([.accessibility: .granted, .microphone: .granted, .screenRecording: .granted])
    func status() -> [OsPermission: PermissionState] { st.value }
    func request(_ p: OsPermission) async {}
    func set(_ p: OsPermission, _ s: PermissionState) { st.mutate { $0[p] = s } }
}

final class ArbiterTests: XCTestCase {
    static let vectors = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent("../../../../spec/vectors/helper/arbiter.json").standardized

    private func perm(_ s: String) -> OsPermission { OsPermission(rawValue: s)! }

    private func event(_ v: JSONValue) -> ArbiterEvent {
        let o = v.object!
        if let c = o["cmd"]?.string { return .cmd(c) }
        if let p = o["revoked"]?.string { return .revoked(perm(p)) }
        if let p = o["granted"]?.string { return .granted(perm(p)) }
        return .helperLost
    }

    private func state(_ v: JSONValue) -> ArbiterState {
        let o = v.object!
        var s = ArbiterState(denied: o["denied"]!.array!.map { perm($0.string!) })
        s.recording = o["recording"]!.bool!; s.dictating = o["dictating"]!.bool!
        s.mic = o["mic"]!.array!.map { $0.string! }
        return s
    }

    /// Same vectors as packages/core/src/helper/arbiter.test.ts: the TS and Swift arbiters cannot drift.
    func testSharedVectors() throws {
        let root = try JSONValue.parse(String(contentsOf: Self.vectors, encoding: .utf8))
        let cases = root.object!["cases"]!.array!
        XCTAssertEqual(cases.count, 10)
        var steps = 0
        for c in cases {
            let co = c.object!
            let name = co["name"]!.string!
            var s = ArbiterState(denied: co["initialDenied"]!.array!.map { perm($0.string!) })
            for (i, st) in co["steps"]!.array!.enumerated() {
                let so = st.object!, exp = so["expect"]!.object!
                let r = SessionArbiter.step(s, event(so["event"]!))
                let ctx = "\(name) step \(i)"
                XCTAssertEqual(r.decision, exp["decision"]!.string!, ctx)
                XCTAssertEqual(r.reason, exp["reason"]?.string, ctx)
                XCTAssertEqual(r.mux, exp["mux"]!.bool!, ctx)
                XCTAssertEqual(r.effects, exp["effects"]!.array!.map { $0.string! }, ctx)
                XCTAssertEqual(r.state, state(exp["state"]!), ctx)
                s = r.state; steps += 1
            }
        }
        XCTAssertGreaterThan(steps, 40)
    }

    func testMicHubRefcountsAndFansOut() throws {
        let mic = FakeMic(), hub = MicHub(source: mic)
        let a = Box<Int>(0), b = Box<Int>(0)
        try hub.subscribe("recording") { s in a.mutate { $0 += s.count } }
        try hub.subscribe("dictation") { s in b.mutate { $0 += s.count } }
        XCTAssertEqual(mic.starts, 1, "one device for both readers")
        mic.cb?([0, 0, 0])
        XCTAssertEqual(a.value, 3); XCTAssertEqual(b.value, 3)
        hub.unsubscribe("dictation")
        XCTAssertEqual(mic.stops, 0)
        mic.cb?([0])
        XCTAssertEqual(a.value, 4); XCTAssertEqual(b.value, 3)
        hub.unsubscribe("recording")
        XCTAssertEqual(mic.stops, 1); XCTAssertFalse(hub.isRunning)
        try hub.subscribe("dictation") { _ in }
        hub.dropAll()
        XCTAssertEqual(mic.starts, 2); XCTAssertEqual(mic.stops, 2); XCTAssertEqual(hub.subscribers, [])
    }

    private func controller(_ perms: PermissionsProvider, _ hub: MicHub, _ out: Box<[HelperMessage]>, audio: [AudioSource] = [FakeAudio("sck")]) -> HelperController {
        HelperController(
            deps: HelperDeps(permissions: perms, meetings: nil, audio: audio, transcribers: [FakeASR()], helperVersion: "t", os: "linux", features: [],
                             dictationMic: hub.view("dictation"), dictationTranscribers: [RecordingASR()], micHub: hub, permissionPollMs: 0),
            send: { m in out.mutate { $0.append(m) } }, now: { 0 })
    }

    func testMuxRecordingAndDictationShareOneMicTap() async throws {
        let out = Box<[HelperMessage]>([]), mic = FakeMic(), hub = MicHub(source: mic)
        let c = controller(MutablePerms(), hub, out)
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("mux-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        try hub.subscribe("recording") { _ in }  // what the macOS capture source does through its hub view
        await c.handle(.dictationStart(id: "d1"))
        XCTAssertEqual(mic.starts, 1, "dictation joined the running tap instead of opening a second device")
        XCTAssertEqual(Set(hub.subscribers), ["recording", "dictation"])
        let st = await c.arbiter
        XCTAssertTrue(st.recording && st.dictating)
        XCTAssertTrue(out.value.contains(.log(level: .info, msg: "mux: recording and dictation share the mic tap")))
        await c.handle(.dictationStart(id: "d2"))
        XCTAssertTrue(out.value.contains(.log(level: .warn, msg: "dictation.start while already-dictating; ignored")))
        mic.cb?([Float](repeating: 0.1, count: 1600))
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertTrue(out.value.contains { if case .dictationStopped("d1", _, _, nil) = $0 { true } else { false } })
        XCTAssertEqual(hub.subscribers, ["recording"]); XCTAssertEqual(mic.stops, 0, "meeting keeps the tap")
        await c.handle(.recordingStop)
        XCTAssertTrue(out.value.contains { if case .recordingStopped = $0 { true } else { false } })
        try? FileManager.default.removeItem(atPath: path)
    }

    func testMicRevokedMidMuxStopsDictationAndKeepsRecording() async throws {
        let out = Box<[HelperMessage]>([]), mic = FakeMic(), hub = MicHub(source: mic), perms = MutablePerms()
        let c = controller(perms, hub, out)
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("rev-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        await c.handle(.dictationStart(id: "d1"))
        mic.cb?([Float](repeating: 0.1, count: 1600))
        await c.checkPermissions()
        XCTAssertFalse(out.value.contains { if case .permissionRevoked = $0 { true } else { false } }, "no change, no event")
        perms.set(.microphone, .denied)
        await c.checkPermissions()
        let msgs = out.value
        XCTAssertTrue(msgs.contains(.permissionRevoked(permission: .microphone, atMs: 0)))
        XCTAssertTrue(msgs.contains { if case .dictationStopped("d1", _, _, .permissionRevoked) = $0 { true } else { false } })
        XCTAssertFalse(hub.isRunning, "mic tap dropped for every reader")
        let st = await c.arbiter
        XCTAssertTrue(st.recording); XCTAssertFalse(st.dictating)
        XCTAssertEqual(st.denied, [.microphone])
        await c.handle(.dictationStart(id: "d2"))
        XCTAssertTrue(out.value.contains(.error(code: "dictation.mic", message: "microphone access denied", fatal: false)))
        // Screen revoked too: one more event, recording still running (the host flags audio-unavailable).
        perms.set(.screenRecording, .denied)
        await c.checkPermissions()
        XCTAssertTrue(out.value.contains(.permissionRevoked(permission: .screenRecording, atMs: 0)))
        await c.checkPermissions()
        XCTAssertEqual(out.value.filter { if case .permissionRevoked = $0 { true } else { false } }.count, 2, "revocations fire once")
        await c.handle(.recordingStop)
        try? FileManager.default.removeItem(atPath: path)
    }

    func testAccessibilityRevokedDuringDictationKeepsDictating() async {
        let out = Box<[HelperMessage]>([]), mic = FakeMic(), hub = MicHub(source: mic), perms = MutablePerms()
        let c = controller(perms, hub, out, audio: [])
        await c.handle(.dictationStart(id: "d1"))
        perms.set(.accessibility, .denied)
        await c.checkPermissions()
        XCTAssertTrue(out.value.contains(.permissionRevoked(permission: .accessibility, atMs: 0)))
        let active = await c.dictation.isActive
        XCTAssertTrue(active, "insertion falls back to the clipboard; dictation keeps going")
        perms.set(.accessibility, .granted)
        await c.checkPermissions()
        let st = await c.arbiter
        XCTAssertEqual(st.denied, [])
        await c.handle(.dictationStop(id: "d1"))
    }
}
