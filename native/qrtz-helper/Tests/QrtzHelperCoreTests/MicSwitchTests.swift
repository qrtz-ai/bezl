import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// A mic whose device can be switched under it, recovering the way mac `MicCapture` does (`MicRestart`).
final class SwitchMic: MicSource, @unchecked Sendable {
    let lock = NSLock()
    var cb: (@Sendable ([Float]) -> Void)?
    var open = false, deviceReturns = true
    var opens = 0
    var lost: (@Sendable (String) -> Void)?
    let restart = MicRestart(delaysMs: [0, 20, 40])
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws { lock.withLock { cb = onSamples; open = true; opens += 1 } }
    func stop() { restart.cancel(); lock.withLock { cb = nil; open = false } }
    func onDeviceLost(_ cb: @escaping @Sendable (String) -> Void) { lock.withLock { lost = cb } }
    /// Delivers only while the input is open (a stopped engine delivers nothing).
    func deliver(_ s: [Float]) { let c = lock.withLock { open ? cb : nil }; c?(s) }
    func switchDevice() {
        lock.withLock { open = false }  // the engine stopped itself
        restart.deviceChanged(attempt: { [self] in
            lock.withLock {
                guard deviceReturns, cb != nil else { return "no input device" }
                open = true; opens += 1
                return nil
            }
        }, done: { [self] r in if case .failure(let e) = r { lock.withLock { lost }?(e.reason) } })
    }
}

final class MicSwitchTests: XCTestCase {
    func sleep(_ ms: UInt64) async { try? await Task.sleep(nanoseconds: ms * 1_000_000) }
    func level(_ k: Int, _ n: Int) -> [Float] { [Float](repeating: Float(k) / 10, count: n) }

    func testReadersKeepTheirSubscriptionAcrossASwitch() async throws {
        let mic = SwitchMic(), hub = MicHub(source: mic)
        let got = Box<[String]>([])
        hub.permit("recording")
        try hub.view("recording", gated: true).start(onSamples: { _ in got.mutate { $0.append("recording") } })
        try hub.view("dictation").start(onSamples: { _ in got.mutate { $0.append("dictation") } })
        mic.deliver([0.1])
        mic.switchDevice()
        mic.deliver([0.1])  // during the switch: nothing
        await sleep(50)
        mic.deliver([0.1])
        XCTAssertEqual(got.value, ["recording", "dictation", "recording", "dictation"])
        XCTAssertEqual(hub.subscribers, ["recording", "dictation"])
        XCTAssertTrue(hub.isRunning)
    }

    func controller(_ mic: SwitchMic) -> (HelperController, Box<[HelperMessage]>) {
        let hub = MicHub(source: mic)
        let out = Box<[HelperMessage]>([])
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux",
                              features: [], dictationMic: hub.view("dictation"), dictationTranscribers: [ChunkASR()], micHub: hub)
        deps.preloadDictationAsr = false
        deps.dictationTail = DictationTail(tailMs: 0, drainTimeoutMs: 100, padMs: 0, finalizeTimeoutMs: 300)
        return (HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 }), out)
    }

    func stopped(_ msgs: [HelperMessage]) -> [(String, DictationStopReason?)] {
        msgs.compactMap { if case .dictationStopped(_, let t, _, let r, _) = $0 { (t, r) } else { nil } }
    }

    func testADictationSpanningASwitchHasTheWordsFromBothSides() async {
        let mic = SwitchMic()
        let (c, out) = controller(mic)
        await c.start()
        await c.handle(.dictationStart(id: "d1"))
        mic.deliver(level(1, 8960))
        mic.switchDevice()  // AirPods connect mid-sentence
        await sleep(50)
        mic.deliver(level(2, 8960))
        await sleep(50)
        await c.handle(.dictationStop(id: "d1"))
        let s = stopped(out.value)
        XCTAssertEqual(s.count, 1)
        XCTAssertEqual(s.first?.0, "w1 w2")
        XCTAssertNil(s.first?.1)
        XCTAssertEqual(mic.opens, 2)
    }

    func testADeviceThatNeverComesBackEndsTheDictationOnceWithWhatWasHeard() async {
        let mic = SwitchMic()
        mic.deviceReturns = false
        let (c, out) = controller(mic)
        await c.start()
        await c.handle(.dictationStart(id: "d1"))
        mic.deliver(level(1, 8960))
        await sleep(30)
        mic.switchDevice()
        // The restart's tries run out (a few hundred ms here); a loaded CI runner may take longer: poll, then settle.
        for _ in 0..<60 where stopped(out.value).isEmpty { await sleep(50) }
        await sleep(100)
        let s = stopped(out.value)
        XCTAssertEqual(s.count, 1, "exactly one stop")
        XCTAssertEqual(s.first?.1, .micLost)
        XCTAssertEqual(s.first?.0, "w1", "what was heard before the switch")
        XCTAssertEqual(out.value.filter { if case .log(.warn, let m) = $0 { m.hasPrefix("mic: input device lost") } else { false } }.count, 1)
        await c.handle(.dictationStop(id: "d1"))  // the host's late stop answers nothing more
        XCTAssertEqual(stopped(out.value).count, 1)
    }

    func testANewerSwitchSupersedesTheTriesOfAnOlderOne() async {
        let mic = SwitchMic()
        let hub = MicHub(source: mic)
        let lost = Box<Int>(0)
        hub.addLostListener { _ in lost.mutate { $0 += 1 } }
        try? hub.subscribe("dictation", { _ in })
        mic.deviceReturns = false
        mic.switchDevice()
        await sleep(10)
        mic.deviceReturns = true
        mic.switchDevice()  // the device came up: this change wins, the older tries are dropped
        await sleep(150)
        XCTAssertEqual(lost.value, 0)
        XCTAssertTrue(mic.lock.withLock { mic.open })
    }

    func testAStoppedMicDropsItsPendingTries() async {
        let mic = SwitchMic()
        let hub = MicHub(source: mic)
        let lost = Box<Int>(0)
        hub.addLostListener { _ in lost.mutate { $0 += 1 } }
        try? hub.subscribe("dictation", { _ in })
        mic.deviceReturns = false
        mic.switchDevice()
        hub.unsubscribe("dictation")
        await sleep(150)
        XCTAssertEqual(lost.value, 0)
    }

    func testAMeetingWhoseMicNeverComesBackSaysSoOnce() async {
        let mic = SwitchMic()
        mic.deviceReturns = false
        let hub = MicHub(source: mic)
        let out = Box<[HelperMessage]>([])
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [FakeAudio("tap")], transcribers: [FakeASR()], helperVersion: "t",
                              os: "linux", features: [], micHub: hub)
        deps.preloadDictationAsr = false
        let c = HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        await c.start()
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-n138-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        try? hub.subscribe("recording", { _ in })
        mic.switchDevice()
        await sleep(200)
        mic.switchDevice()  // lost again: still one line for this recording
        await sleep(200)
        let d = out.value.filter { if case .captureDegraded([.noMic], _) = $0 { true } else { false } }
        XCTAssertEqual(d.count, 1)
        await c.handle(.recordingStop())
        try? FileManager.default.removeItem(atPath: path)
    }
}
