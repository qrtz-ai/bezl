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
    static let edges = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent("../../../../spec/vectors/helper/arbiter-edges.json").standardized

    private func perm(_ s: String) -> OsPermission { OsPermission(rawValue: s)! }

    private func event(_ v: JSONValue) -> ArbiterEvent {
        let o = v.object!
        if let c = o["cmd"]?.string { return .cmd(c, audioPath: o["audioPath"]?.string) }
        if let f = o["recordingFileFailed"]?.object { return .recordingFileFailed(audioPath: f["audioPath"]!.string!) }
        if let p = o["revoked"]?.string { return .revoked(perm(p)) }
        if let p = o["granted"]?.string { return .granted(perm(p)) }
        if let c = o["configure"]?.object { return .configure(warmMic: c["warmMic"]!.bool!) }
        return .helperLost
    }

    private func state(_ v: JSONValue) -> ArbiterState {
        let o = v.object!
        var s = ArbiterState(denied: o["denied"]!.array!.map { perm($0.string!) })
        s.recording = o["recording"]!.bool!; s.dictating = o["dictating"]!.bool!
        s.activeAudioPath = o["activeAudioPath"]!.string  // JSON null -> nil
        s.warm = o["warm"]!.bool!
        s.mic = o["mic"]!.array!.map { $0.string! }
        return s
    }

    /// Same vectors as packages/core/src/helper/arbiter.test.ts: the TS and Swift arbiters cannot drift.
    func testSharedVectors() throws {
        let steps = try replay(Self.vectors, expectCases: 16)
        XCTAssertGreaterThan(steps, 40)
    }

    /// Bozniak's edge vectors (spec/vectors/helper/arbiter-edges.json), also replayed by arbiter-model.test.ts.
    func testEdgeVectors() throws {
        let steps = try replay(Self.edges, expectCases: nil)
        XCTAssertGreaterThan(steps, 0)
    }

    private func replay(_ url: URL, expectCases: Int?) throws -> Int {
        let root = try JSONValue.parse(String(contentsOf: url, encoding: .utf8))
        let cases = root.object!["cases"]!.array!
        if let n = expectCases { XCTAssertEqual(cases.count, n) }
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
                XCTAssertNil(SessionArbiter.invariantViolation(r.state), ctx)
                s = r.state; steps += 1
            }
        }
        return steps
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
        XCTAssertTrue(out.value.contains(.error(code: "dictation.busy", message: "a dictation is already running", fatal: false, id: "d2", activeId: "d1")))
        mic.cb?([Float](repeating: 0.1, count: 1600))
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertTrue(out.value.contains { if case .dictationStopped("d1", _, _, nil) = $0 { true } else { false } })
        XCTAssertEqual(hub.subscribers, ["recording"]); XCTAssertEqual(mic.stops, 0, "meeting keeps the tap")
        await c.handle(.recordingStop())
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
        XCTAssertTrue(msgs.contains(.permissionRevoked(permission: .microphone, atMs: 0, degraded: [.noMic])))
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
        XCTAssertTrue(out.value.contains(.permissionRevoked(permission: .screenRecording, atMs: 0, degraded: [.audioUnavailable])))
        await c.checkPermissions()
        XCTAssertEqual(out.value.filter { if case .permissionRevoked = $0 { true } else { false } }.count, 2, "revocations fire once")
        await c.handle(.recordingStop())
        try? FileManager.default.removeItem(atPath: path)
    }

    func testAccessibilityRevokedDuringDictationKeepsDictating() async {
        let out = Box<[HelperMessage]>([]), mic = FakeMic(), hub = MicHub(source: mic), perms = MutablePerms()
        let c = controller(perms, hub, out, audio: [])
        await c.handle(.dictationStart(id: "d1"))
        perms.set(.accessibility, .denied)
        await c.checkPermissions()
        XCTAssertTrue(out.value.contains(.permissionRevoked(permission: .accessibility, atMs: 0, degraded: [])))
        let active = await c.dictation.isActive
        XCTAssertTrue(active, "insertion falls back to the clipboard; dictation keeps going")
        perms.set(.accessibility, .granted)
        await c.checkPermissions()
        let st = await c.arbiter
        XCTAssertEqual(st.denied, [])
        await c.handle(.dictationStop(id: "d1"))
    }

    private func tmp(_ n: String) -> String { FileManager.default.temporaryDirectory.appendingPathComponent("\(n)-\(UUID()).wav").path }

    /// Bug 1: configure warm, record, revoke the mic with no dictation, re-grant: the next dictation must hear.
    func testWarmMicHearsAgainAfterRevokeAndRegrant() async throws {
        let out = Box<[HelperMessage]>([]), mic = FakeMic(), hub = MicHub(source: mic), perms = MutablePerms()
        let c = controller(perms, hub, out)
        await c.handle(.dictationConfigure(warmMic: true, preRollMs: 300, streamAudio: false, pushKey: ""))
        XCTAssertEqual(hub.subscribers, ["dictation"])
        let path = tmp("warm")
        await c.handle(.recordingStart(audioPath: path))
        perms.set(.microphone, .denied)
        await c.checkPermissions()
        XCTAssertEqual(hub.subscribers, []); XCTAssertFalse(hub.isRunning)
        var running = await c.dictation.isMicRunning
        XCTAssertFalse(running, "engine forgot the dropped reader")
        perms.set(.microphone, .granted)
        await c.checkPermissions()
        running = await c.dictation.isMicRunning
        XCTAssertTrue(running); XCTAssertEqual(hub.subscribers, ["dictation"], "warm reader is back, recording is not resubscribed")
        await c.handle(.dictationStart(id: "d1"))
        XCTAssertNotNil(mic.cb, "device is open again")
        mic.cb?([Float](repeating: 0.1, count: 1600))
        try await Task.sleep(nanoseconds: 100_000_000)  // samples hop through the engine's ordered stream
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertTrue(out.value.contains { if case .dictationStopped("d1", let text, _, nil) = $0 { text == "hello world" } else { false } }, "dictation heard audio")
        XCTAssertEqual(hub.subscribers, ["dictation"], "warm mic keeps the reader after stop")
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 300, streamAudio: false, pushKey: ""))
        XCTAssertEqual(hub.subscribers, [])
        await c.handle(.recordingStop())
        try? FileManager.default.removeItem(atPath: path)
    }

    /// Bug 2: a second recording.start answers recording.busy; a stop for another file is ignored.
    func testRecordingBusyAndMismatchedStopPath() async throws {
        let out = Box<[HelperMessage]>([]), hub = MicHub(source: FakeMic())
        let c = controller(MutablePerms(), hub, out)
        let a = tmp("a"), b = tmp("b")
        await c.handle(.recordingStart(audioPath: a))
        await c.handle(.recordingStart(audioPath: b))
        XCTAssertTrue(out.value.contains(.error(code: "recording.busy", message: "a recording is already running", fatal: false, audioPath: b, activeAudioPath: a)))
        await c.handle(.recordingStop(audioPath: b))
        var rec = await c.isRecording
        XCTAssertTrue(rec, "stale stop for another path does not kill the real recording")
        await c.handle(.recordingStop(audioPath: a))
        rec = await c.isRecording
        XCTAssertFalse(rec)
        XCTAssertTrue(out.value.contains { if case .recordingStopped(a, _) = $0 { true } else { false } })
        for p in [a, b] { try? FileManager.default.removeItem(atPath: p) }
    }

    /// Bug 3: with the mic denied the arbiter does not subscribe the recording, so its gated hub view is skipped.
    func testRecordingWithMicDeniedNeverOpensTheMic() async throws {
        let out = Box<[HelperMessage]>([]), mic = FakeMic(), hub = MicHub(source: mic), perms = MutablePerms()
        perms.set(.microphone, .denied)
        let view = hub.view("recording", gated: true)
        let c = controller(perms, hub, out, audio: [FakeAudio("sck")])
        let path = tmp("denied")
        await c.handle(.recordingStart(audioPath: path))
        try view.start { _ in }
        XCTAssertEqual(mic.starts, 0); XCTAssertEqual(hub.subscribers, [])
        await c.handle(.recordingStop())
        perms.set(.microphone, .granted)
        await c.checkPermissions()
        await c.handle(.recordingStart(audioPath: path))
        try view.start { _ in }
        XCTAssertEqual(hub.subscribers, ["recording"], "permitted once the arbiter subscribed it")
        await c.handle(.recordingStop())
        XCTAssertEqual(hub.subscribers, [])
        try? FileManager.default.removeItem(atPath: path)
    }

    /// Bug 4: an unsubscribe while source.start is still running must not leave the device on.
    func testMicHubUnsubscribeDuringStartStopsTheDevice() throws {
        let mic = SlowMic(), hub = MicHub(source: mic)
        let done = DispatchSemaphore(value: 0)
        DispatchQueue.global().async { try? hub.subscribe("dictation") { _ in }; done.signal() }
        mic.entered.wait()
        hub.unsubscribe("dictation")
        mic.release.signal()
        done.wait()
        XCTAssertFalse(hub.isRunning); XCTAssertEqual(mic.stops.value, 1, "orphaned start is stopped")
        // Resubscribe during start keeps it running (the joiner waits for the start in flight).
        DispatchQueue.global().async { try? hub.subscribe("dictation") { _ in }; done.signal() }
        mic.entered.wait()
        hub.unsubscribe("dictation")
        let joined = DispatchSemaphore(value: 0)
        DispatchQueue.global().async { try? hub.subscribe("recording") { _ in }; joined.signal() }
        waitUntil { hub.subscribers == ["recording"] }
        mic.release.signal()
        done.wait(); joined.wait()
        XCTAssertTrue(hub.isRunning); XCTAssertEqual(mic.stops.value, 1)
        XCTAssertEqual(mic.starts.value, 2, "no second device start while one was in flight")
    }

    /// Re-review 3: a reader that joins while source.start is in flight must not stay attached to a device that
    /// never opened. It waits for the start, rethrows its error, and the hub is clean for the next start.
    func testMicHubStartFailureDetachesAndRethrowsToJoiners() throws {
        let mic = SlowMic(), hub = MicHub(source: mic)
        mic.failNext.mutate { $0 = true }
        let first = Box<Error?>(nil), joiner = Box<Error?>(nil)
        let done = DispatchSemaphore(value: 0)
        DispatchQueue.global().async {
            do { try hub.subscribe("dictation") { _ in } } catch { first.mutate { $0 = error } }
            done.signal()
        }
        mic.entered.wait()
        DispatchQueue.global().async {
            do { try hub.subscribe("recording") { _ in } } catch { joiner.mutate { $0 = error } }
            done.signal()
        }
        waitUntil { Set(hub.subscribers) == ["dictation", "recording"] }
        mic.release.signal()
        done.wait(); done.wait()
        XCTAssertNotNil(first.value, "the starter sees the failure")
        XCTAssertNotNil(joiner.value, "the joiner is told too, instead of believing it has a mic")
        XCTAssertEqual(hub.subscribers, [], "nobody stays attached to a device that never opened")
        XCTAssertFalse(hub.isRunning)
        XCTAssertEqual(mic.starts.value, 1)
        // The next subscribe starts the device afresh and works.
        DispatchQueue.global().async { try? hub.subscribe("recording") { _ in }; done.signal() }
        mic.entered.wait(); mic.release.signal(); done.wait()
        XCTAssertTrue(hub.isRunning); XCTAssertEqual(hub.subscribers, ["recording"]); XCTAssertEqual(mic.starts.value, 2)
    }

    /// Re-review 1 (d1 lockout): the helper still runs d1 after a host reload. Each new start answers
    /// dictation.busy{id, activeId: d1}; a cancel for the refused id is a no-op; the host's cancel{activeId}
    /// frees the helper so the next start works. start{d1} while d1 runs names d1 as both ids and leaves it running.
    func testDictationBusyNamesBothIdsAndCancelOfActiveIdUnlocks() async {
        let out = Box<[HelperMessage]>([]), hub = MicHub(source: FakeMic())
        let c = controller(MutablePerms(), hub, out, audio: [])
        await c.handle(.dictationStart(id: "d1"))
        await c.handle(.dictationStart(id: "d1"))
        XCTAssertTrue(out.value.contains(.error(code: "dictation.busy", message: "a dictation is already running", fatal: false, id: "d1", activeId: "d1")))
        var active = await c.dictation.activeId
        XCTAssertEqual(active, "d1", "a duplicate start never disturbs the running dictation")
        await c.handle(.dictationStart(id: "d2"))
        XCTAssertTrue(out.value.contains(.error(code: "dictation.busy", message: "a dictation is already running", fatal: false, id: "d2", activeId: "d1")))
        await c.handle(.dictationCancel(id: "d2"))
        active = await c.dictation.activeId
        XCTAssertEqual(active, "d1", "cancel for the refused id is a no-op (the old lockout)")
        await c.handle(.dictationCancel(id: "d1"))  // the host cleans up the orphan named by activeId
        active = await c.dictation.activeId
        XCTAssertNil(active)
        await c.handle(.dictationStart(id: "d3"))
        XCTAssertTrue(out.value.contains { if case .dictationStarted("d3", _) = $0 { true } else { false } }, "retry succeeds")
        XCTAssertFalse(out.value.contains(.error(code: "dictation.busy", message: "a dictation is already running", fatal: false, id: "d3", activeId: "d1")))
        await c.handle(.dictationCancel(id: "d3"))
    }

    /// Re-review 2 (orphan recording): recording A outlives the host's view of it. start{B} answers
    /// recording.busy{audioPath: B, activeAudioPath: A}; stop{B} leaves A running; stop{activeAudioPath} ends A,
    /// and the retry of B starts.
    func testRecordingBusyNamesActiveFileAndStoppingItUnlocks() async {
        let out = Box<[HelperMessage]>([]), hub = MicHub(source: FakeMic())
        let c = controller(MutablePerms(), hub, out)
        let a = tmp("orphan-a"), b = tmp("orphan-b")
        await c.handle(.recordingStart(audioPath: a))
        await c.handle(.recordingStart(audioPath: b))
        XCTAssertTrue(out.value.contains(.error(code: "recording.busy", message: "a recording is already running", fatal: false, audioPath: b, activeAudioPath: a)))
        await c.handle(.recordingStop(audioPath: b))
        var rec = await c.isRecording
        XCTAssertTrue(rec, "the attempt's own cleanup stop does not end the orphan")
        await c.handle(.recordingStop(audioPath: a))
        XCTAssertTrue(out.value.contains { if case .recordingStopped(a, _) = $0 { true } else { false } })
        await c.handle(.recordingStart(audioPath: b))
        XCTAssertTrue(out.value.contains(.recordingStarted(audioPath: b)), "retry succeeds")
        rec = await c.isRecording
        XCTAssertTrue(rec)
        await c.handle(.recordingStop(audioPath: b))
        for p in [a, b] { try? FileManager.default.removeItem(atPath: p) }
    }

    /// File rule (Bozniak): a recording never runs without a file. The arbiter refuses a start with no path, and
    /// recordingFileFailed tears the admitted recording down; the invariant holds in every state the vectors reach.
    func testArbiterFileRuleAndInvariant() {
        var s = ArbiterState()
        let refused = SessionArbiter.step(s, .cmd("recording.start"))
        XCTAssertEqual(refused.decision, "refuse"); XCTAssertEqual(refused.reason, "no-audio-path"); XCTAssertEqual(refused.state, s)
        s = SessionArbiter.step(s, .cmd("dictation.start")).state
        s = SessionArbiter.step(s, .cmd("recording.start", audioPath: "/rec/a.wav")).state
        XCTAssertEqual(s.activeAudioPath, "/rec/a.wav")
        XCTAssertEqual(SessionArbiter.step(s, .recordingFileFailed(audioPath: "/rec/b.wav")).reason, "other-recording")
        let r = SessionArbiter.step(s, .recordingFileFailed(audioPath: "/rec/a.wav"))
        XCTAssertEqual(r.decision, "accept")
        XCTAssertEqual(r.effects, ["recording.lost", "mic.unsubscribe:recording"])
        XCTAssertFalse(r.state.recording); XCTAssertNil(r.state.activeAudioPath)
        XCTAssertTrue(r.state.dictating); XCTAssertEqual(r.state.mic, ["dictation"])
        var broken = ArbiterState(); broken.recording = true
        XCTAssertEqual(SessionArbiter.invariantViolation(broken), "recording without an activeAudioPath")
        var stale = ArbiterState(); stale.activeAudioPath = "/rec/a.wav"
        XCTAssertEqual(SessionArbiter.invariantViolation(stale), "activeAudioPath set while not recording")
    }

    /// File rule through the controller: WavWriter fails, so the helper fails closed. It releases the recording's
    /// mic permit, answers error{recording.file, audioPath}, never sends recording.started, keeps the dictation, and
    /// is idle for recordings again: the next start is admitted (no recording.busy with a nil activeAudioPath).
    func testRecordingFileFailureTearsTheRecordingDown() async throws {
        let out = Box<[HelperMessage]>([]), mic = FakeMic(), hub = MicHub(source: mic)
        let view = hub.view("recording", gated: true)
        let c = controller(MutablePerms(), hub, out)
        await c.handle(.dictationStart(id: "d1"))
        let bad = "/dev/null/qrtz-\(UUID()).wav"  // parent is not a directory: the file can never be created
        await c.handle(.recordingStart(audioPath: bad))
        XCTAssertTrue(out.value.contains { if case .error("recording.file", _, false, _, _, bad, nil) = $0 { true } else { false } })
        XCTAssertFalse(out.value.contains { if case .recordingStarted = $0 { true } else { false } })
        var rec = await c.isRecording
        XCTAssertFalse(rec)
        var st = await c.arbiter
        XCTAssertFalse(st.recording); XCTAssertNil(st.activeAudioPath)
        XCTAssertTrue(st.dictating); XCTAssertEqual(st.mic, ["dictation"])
        try view.start { _ in }
        XCTAssertEqual(hub.subscribers, ["dictation"], "the recording's mic permit was revoked")
        let good = tmp("after-file-failure")
        await c.handle(.recordingStart(audioPath: good))
        XCTAssertFalse(out.value.contains { if case .error("recording.busy", _, _, _, _, _, _) = $0 { true } else { false } })
        XCTAssertTrue(out.value.contains(.recordingStarted(audioPath: good)))
        st = await c.arbiter
        XCTAssertEqual(st.activeAudioPath, good)
        await c.handle(.recordingStop(audioPath: good))
        rec = await c.isRecording
        XCTAssertFalse(rec)
        await c.handle(.dictationCancel(id: "d1"))
        try? FileManager.default.removeItem(atPath: good)
    }
}

/// An audio source whose stop never returns until released (a wedged flush).
final class WedgedAudio: AudioSource, @unchecked Sendable {
    let name = "wedged"
    let stopping = Box<Bool>(false)
    private let held = Box<[CheckedContinuation<Void, Never>]>([])
    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void) async throws {}
    func stop() async {
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            held.mutate { $0.append(c) }; stopping.mutate { $0 = true }
        }
    }
    func release() { let cs = held.value; held.mutate { $0 = [] }; for c in cs { c.resume() } }
}

final class AbortTests: XCTestCase {
    /// N2: recording.stop wedges in the audio source's stop. recording.abort{audioPath} (handled out of band) tears
    /// that recording down without the flush, answers recording.aborted, sends no recording.stopped, keeps the
    /// dictation on the shared mic, and lets the next recording start. The released stop then touches nothing.
    func testAbortTearsDownAWedgedStopAndKeepsTheDictation() async throws {
        let out = Box<[HelperMessage]>([]), hub = MicHub(source: FakeMic()), audio = WedgedAudio()
        let c = HelperController(
            deps: HelperDeps(permissions: MutablePerms(), meetings: nil, audio: [audio], transcribers: [FakeASR()], helperVersion: "t", os: "linux", features: [],
                             dictationMic: hub.view("dictation"), dictationTranscribers: [RecordingASR()], micHub: hub, permissionPollMs: 0),
            send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        let a = FileManager.default.temporaryDirectory.appendingPathComponent("abort-a-\(UUID()).wav").path
        let b = FileManager.default.temporaryDirectory.appendingPathComponent("abort-b-\(UUID()).wav").path
        await c.handle(.dictationStart(id: "d1"))
        await c.handle(.recordingStart(audioPath: a))
        XCTAssertTrue(out.value.contains(.recordingStarted(audioPath: a)))
        let stopTask = Task { await c.handle(.recordingStop()) }
        let end = Date().addingTimeInterval(5)
        while !audio.stopping.value && Date() < end { try await Task.sleep(nanoseconds: 1_000_000) }
        XCTAssertTrue(audio.stopping.value, "the stop is wedged in the audio source")
        await c.handle(.recordingAbort(audioPath: a))
        XCTAssertTrue(out.value.contains(.recordingAborted(audioPath: a)))
        var rec = await c.isRecording
        XCTAssertFalse(rec)
        let st = await c.arbiter
        XCTAssertFalse(st.recording); XCTAssertNil(st.activeAudioPath)
        XCTAssertTrue(st.dictating, "the dictation survives the abort"); XCTAssertEqual(st.mic, ["dictation"])
        // The helper is free again: a new recording starts while the old stop is still wedged.
        await c.handle(.recordingStart(audioPath: b))
        XCTAssertTrue(out.value.contains(.recordingStarted(audioPath: b)))
        audio.release()
        await stopTask.value
        XCTAssertFalse(out.value.contains { if case .recordingStopped(a, _) = $0 { true } else { false } }, "no late stop ack for the aborted file")
        rec = await c.isRecording
        XCTAssertTrue(rec, "the released old stop did not touch the new recording")
        // Scoped and idempotent: an abort for a file nobody records is answered and changes nothing.
        await c.handle(.recordingAbort(audioPath: a))
        XCTAssertEqual(out.value.filter { $0 == .recordingAborted(audioPath: a) }.count, 2)
        rec = await c.isRecording
        XCTAssertTrue(rec)
        let stop2 = Task { await c.handle(.recordingAbort(audioPath: b)) }
        await stop2.value
        audio.release()
        await c.handle(.dictationCancel(id: "d1"))
        for p in [a, b] { try? FileManager.default.removeItem(atPath: p) }
    }
}

/// Polls `cond` (the hub's lock is released while a joiner waits, so its getters stay usable).
func waitUntil(timeout: TimeInterval = 5, _ cond: () -> Bool) {
    let end = Date().addingTimeInterval(timeout)
    while !cond() && Date() < end { usleep(1_000) }
    XCTAssertTrue(cond(), "condition not reached in time")
}

final class SlowMic: MicSource, @unchecked Sendable {
    struct Failed: Error {}
    let entered = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
    let starts = Box<Int>(0), stops = Box<Int>(0), failNext = Box<Bool>(false)
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws {
        starts.mutate { $0 += 1 }; entered.signal(); release.wait()
        if failNext.value { failNext.mutate { $0 = false }; throw Failed() }
    }
    func stop() { stops.mutate { $0 += 1 } }
}
