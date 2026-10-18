import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// Item 2 (docs/echo/dictation-tuning.md): the press opens the mic, not the host's dictation.start.
final class ArmedMicTests: XCTestCase {
    final class KeyHotkeys: HotkeyMonitor, @unchecked Sendable {
        let cb = Box<(@Sendable (HotkeyAction) -> Void)?>(nil)
        func start(key: String, consumeFn: Bool, onChange: @escaping @Sendable (HotkeyAction) -> Void) -> Bool { cb.mutate { $0 = onChange }; return true }
        func stop() {}
        func press(_ a: HotkeyAction) { cb.value?(a) }
    }

    func make(warm: Bool = false) async -> (HelperController, FakeMic, RecordingASR, KeyHotkeys, Box<[HelperMessage]>) {
        let out = Box<[HelperMessage]>([])
        let mic = FakeMic(), asr = RecordingASR(), hk = KeyHotkeys()
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux", features: [],
                              dictationMic: mic, dictationTranscribers: [asr], insertion: nil, hotkeys: hk)
        deps.dictationTail = DictationTail(tailMs: 0, padMs: 0)
        let c = HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        await c.handle(.dictationConfigure(warmMic: warm, preRollMs: 300, streamAudio: false, pushKey: "fn"))
        return (c, mic, asr, hk, out)
    }

    /// Polls (the press reaches the controller through a task); generous, so a loaded runner doesn't fail it.
    func until(_ what: String, _ ok: @escaping () async -> Bool) async {
        for _ in 0..<1_000 { if await ok() { return }; try? await Task.sleep(nanoseconds: 5_000_000) }
        XCTFail("timed out waiting for \(what)")
    }

    func testThePressOpensTheMicAndTheStartGetsEverythingSinceIt() async {
        let (c, mic, asr, hk, out) = await make()
        XCTAssertEqual(mic.starts, 0)
        hk.press(.down)
        await until("armed") { await c.isArmed && mic.starts == 1 }
        // 600 ms heard while the host was still on its way to dictation.start: more than the 300 ms pre-roll.
        await c.dictation.onSamples([Float](repeating: 0.1, count: 9_600))
        await c.handle(.dictationStart(id: "d1"))
        XCTAssertTrue(out.value.contains(.dictationStarted(id: "d1", preRollMs: 600)))
        XCTAssertEqual(asr.received, 9_600, "the head reached the recogniser")
        let armed = await c.isArmed
        XCTAssertFalse(armed)
        hk.press(.up)
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertEqual(mic.stops, 1, "the reader goes with the dictation")
        let st = await c.arbiter
        XCTAssertFalse(st.warm); XCTAssertEqual(st.mic, [])
        let timing = await c.dictation.lastTiming
        XCTAssertEqual(timing?.armed, true)
        XCTAssertTrue(timing?.logLine.contains("armed=1") ?? false)
    }

    func testAPressThatNeverStartsLetsTheMicGo() async {
        let (c, mic, _, hk, _) = await make()
        hk.press(.down)
        await until("armed") { await c.isArmed && mic.starts == 1 }
        hk.press(.up)
        // The host refused it (no dictation.start): the mic goes after the grace.
        await until("disarmed") { await !c.isArmed && mic.stops == 1 }
        let st = await c.arbiter
        XCTAssertFalse(st.warm); XCTAssertEqual(st.mic, [])
        let pre = await c.dictation.preRollSamples
        XCTAssertEqual(pre, 0)
    }

    func testAWarmMicIsNotArmed() async {
        let (c, mic, _, hk, _) = await make(warm: true)
        XCTAssertEqual(mic.starts, 1)
        hk.press(.down)
        try? await Task.sleep(nanoseconds: 50_000_000)
        let armed = await c.isArmed
        XCTAssertFalse(armed)
        XCTAssertEqual(mic.starts, 1)
    }

    func testAPressWhileListeningIsNotAnArm() async {
        // Toggle mode's second press (the stop) while a dictation listens.
        let (c, mic, _, hk, _) = await make()
        await c.handle(.dictationStart(id: "d1"))
        XCTAssertEqual(mic.starts, 1)
        hk.press(.down)
        try? await Task.sleep(nanoseconds: 50_000_000)
        let armed = await c.isArmed
        XCTAssertFalse(armed)
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertEqual(mic.stops, 1)
    }
}
