import Foundation
import XCTest
@testable import QrtzHelperCore

/// N143: a system tap for the dictation's length, in order.
final class FakeTap: SystemAudioTap, @unchecked Sendable {
    let name = "fake-tap"
    var likely = true, fails = false
    /// Fail only the first start, after this delay (N152).
    var failFirstAfterMs: UInt64 = 0
    var starts = 0
    let calls = Box<[String]>([])
    let cb = Box<(@Sendable ([Float]) -> Void)?>(nil)
    func echoLikely() -> Bool { likely }
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) async throws {
        calls.mutate { $0.append("start") }
        starts += 1
        if failFirstAfterMs > 0, starts == 1 {
            try? await Task.sleep(nanoseconds: failFirstAfterMs * 1_000_000)
            throw ProtocolError("slow failure")
        }
        if fails { throw ProtocolError("no tap") }
        cb.mutate { $0 = onSamples }
    }
    func stop() async { calls.mutate { $0.append("stop") }; cb.mutate { $0 = nil } }
}

/// Hears a block when its RMS is above a whisper. A block with a DC level is the user ("u<level>"); the sine is the
/// system sentence coming back through the mic ("echo").
final class HearingASR: StreamingTranscriber, @unchecked Sendable {
    let name = "hearing"
    let loadsInstantly = true
    let words = Box<[String]>([])
    func prepare() async throws {}
    func append(_ s: [Float]) async {
        guard DSP.rms(s) > 0.01 else { return }
        let mean = s.reduce(0, +) / Float(s.count)
        let w = mean > 0.05 ? "u\(Int((mean * 10).rounded()))" : "echo"
        words.mutate { if $0.last != w { $0.append(w) } }
    }
    func transcript() async -> String { words.value.joined(separator: " ") }
    func finish() async -> String { words.value.joined(separator: " ") }
    func reset() async { words.mutate { $0.removeAll() } }
}

struct NoScreenPerms: PermissionsProvider {
    func status() -> [OsPermission: PermissionState] { [.accessibility: .granted, .microphone: .granted, .screenRecording: .notDetermined] }
    func request(_ p: OsPermission) async {}
}

/// Up to 1 s for the tap to deliver (it starts in the background).
func tapStarted(_ tap: FakeTap) async {
    for _ in 0..<200 {
        if tap.cb.value != nil { return }
        try? await Task.sleep(nanoseconds: 5_000_000)
    }
}

final class DictationEchoTests: XCTestCase {
    static let n = 1360
    /// The known sentence the Mac plays: a 0.5 sine.
    func sentence(_ k: Int) -> [Float] { (0..<Self.n).map { 0.5 * sin(Float(k * Self.n + $0) * 0.07) } }
    let silence = [Float](repeating: 0, count: 1360)

    func controller(tap: FakeTap?, perms: PermissionsProvider = FakePerms(), audio: [AudioSource] = [])
        -> (HelperController, Box<[HelperMessage]>, HearingASR) {
        let out = Box<[HelperMessage]>([])
        let asr = HearingASR()
        var deps = HelperDeps(permissions: perms, meetings: nil, audio: audio, transcribers: [], helperVersion: "t", os: "linux",
                              features: [], dictationMic: FakeMic(), dictationTranscribers: [asr])
        deps.dictationTail = DictationTail(tailMs: 0, padMs: 0)
        deps.dictationTap = tap
        return (HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 }), out, asr)
    }

    func stoppedText(_ msgs: [HelperMessage]) -> String? {
        msgs.compactMap { if case .dictationStopped(_, let t, _, _, _) = $0 { t } else { nil } }.first
    }
    func logged(_ msgs: [HelperMessage], _ s: String) -> Bool { msgs.contains { if case .log(_, let m) = $0 { m.contains(s) } else { false } } }

    /// The system plays the sentence the whole time; the mic hears it at -20 dB, plus the user's words in blocks 4–5
    /// (over the sentence) and 9–10. `system` feeds whatever reference is running before each mic block.
    func dictate(_ c: HelperController, user: Float = 0.3, system: (([Float]) -> Void)?) async {
        await c.handle(.dictationStart(id: "d1"))
        for k in 0..<12 {
            let sys = (9...10).contains(k) ? silence : sentence(k)
            system?(sys)
            let word: Float = (4...5).contains(k) ? user : (9...10).contains(k) ? user + 0.1 : 0
            await c.dictation.onSamples(zip(sys, silence).map { s, _ in 0.1 * s + word })
        }
        await c.handle(.dictationStop(id: "d1"))
    }

    func testWithoutAGateTheSentenceOnTheSpeakersIsTyped() async {
        let (c, out, _) = controller(tap: nil)
        await dictate(c, system: nil)
        XCTAssertEqual(stoppedText(out.value), "echo u3 echo u4 echo", "the test hears the echo when nothing gates it")
    }

    func testTheDictationTapGatesTheSentenceAndKeepsTheUsersWords() async {
        let tap = FakeTap()
        let (c, out, _) = controller(tap: tap)
        // Two short presses first: each starts and stops its own tap.
        for id in ["p1", "p2"] {
            await c.handle(.dictationStart(id: id))
            await tapStarted(tap)
            await c.handle(.dictationCancel(id: id))
            await c.dictation.echo?.settled()
        }
        out.mutate { $0.removeAll() }

        // The real run: the tap is up before the first mic block.
        await c.handle(.dictationStart(id: "d1"))
        await tapStarted(tap)
        for k in 0..<12 {
            let sys = (9...10).contains(k) ? silence : sentence(k)
            tap.cb.value?(sys)
            let word: Float = (4...5).contains(k) ? 0.3 : (9...10).contains(k) ? 0.4 : 0
            await c.dictation.onSamples(sys.map { 0.1 * $0 + word })
        }
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertEqual(stoppedText(out.value), "u3 u4", "only the user's words")
        XCTAssertTrue(logged(out.value, "echo gate: fake-tap for this dictation"))
        await c.dictation.echo?.settled()
        XCTAssertEqual(tap.calls.value, ["start", "stop", "start", "stop", "start", "stop"], "one tap per dictation, stopped before the next starts")
        XCTAssertFalse(c.echoReference.isLive)
    }

    func testASoftVoiceOverALoudVideoPasses() async {
        // The meeting's EchoGate would hold this down (the system is louder than the mic); the dictation gate knows
        // the echo comes back at -20 dB, and the voice lifts the mic well above it.
        let gate = DictationEchoGate()
        for k in 0..<4 { var m = sentence(k).map { 0.1 * $0 }; gate.process(system: sentence(k), mic: &m) }
        XCTAssertLessThan(gate.gain, 0.1)
        var voice = sentence(4).map { 0.1 * $0 + 0.1 }
        gate.process(system: sentence(4), mic: &voice)
        XCTAssertEqual(gate.gain, 1)
        XCTAssertEqual(EchoGate(context: MeetingContext()).target(systemRms: DSP.rms(sentence(4)), micRms: DSP.rms(voice)), 0.316, accuracy: 0.001)
    }

    func testInAMeetingTheRecordingsSystemChannelIsTheReferenceAndNoTapStarts() async throws {
        let tap = FakeTap()
        let audio = FakeAudio("tap")
        let (c, out, _) = controller(tap: tap, audio: [audio])
        let path = NSTemporaryDirectory() + "n143-\(UUID().uuidString).wav"
        defer { try? FileManager.default.removeItem(atPath: path) }
        await c.handle(.recordingStart(audioPath: path))
        guard let cb = audio.cb else { return XCTFail("recording source started") }
        await dictate(c, system: { s in cb(CaptureBlock(system: s, mic: [Float](repeating: 0, count: s.count))) })
        XCTAssertEqual(stoppedText(out.value), "u3 u4")
        XCTAssertTrue(logged(out.value, "echo gate: the meeting's system audio"))
        XCTAssertEqual(tap.calls.value, [], "the meeting's channel, not a second tap")
        await c.handle(.recordingStop())
        XCTAssertFalse(c.echoReference.isLive)
    }

    func testWithoutTheSystemAudioGrantTheMicIsRawAndNothingPrompts() async {
        let tap = FakeTap()
        let (c, out, _) = controller(tap: tap, perms: NoScreenPerms())
        await dictate(c, system: { tap.cb.value?($0) })
        XCTAssertEqual(tap.calls.value, [])
        XCTAssertTrue(logged(out.value, "no system audio grant"))
        XCTAssertEqual(stoppedText(out.value), "echo u3 echo u4 echo")
    }

    func testHeadphonesOrSilenceStartNoTap() async {
        let tap = FakeTap()
        tap.likely = false
        let (c, out, _) = controller(tap: tap)
        await dictate(c, system: nil)
        XCTAssertEqual(tap.calls.value, [])
        XCTAssertTrue(logged(out.value, "headphones or nothing playing"))
    }

    func testATapThatFailsLeavesTheRawMic() async {
        let tap = FakeTap()
        tap.fails = true
        let (c, out, _) = controller(tap: tap)
        await c.handle(.dictationStart(id: "d1"))
        await c.dictation.echo?.settled()
        XCTAssertTrue(out.value.contains { if case .log(.warn, let m) = $0 { m.contains("fake-tap didn't start") } else { false } })
        XCTAssertFalse(c.echoReference.isLive)
        await c.dictation.onSamples(sentence(0).map { 0.1 * $0 })
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertEqual(stoppedText(out.value), "echo")
    }

    func testALateTapFailureNeverDetachesTheNextDictationsHold() async {
        // N152: d1's tap fails 100 ms in, after d1 ended and d2 began: d2 keeps its reference.
        let tap = FakeTap()
        tap.failFirstAfterMs = 100
        let (c, out, _) = controller(tap: tap)
        await c.handle(.dictationStart(id: "d1"))
        await c.handle(.dictationCancel(id: "d1"))
        await c.handle(.dictationStart(id: "d2"))
        XCTAssertTrue(c.echoReference.isLive)
        await tapStarted(tap)
        XCTAssertTrue(c.echoReference.isLive, "d1's failure didn't drop d2's hold")
        XCTAssertNotNil(tap.cb.value)
        tap.cb.value?(sentence(0))
        XCTAssertNotNil(c.echoReference.take(1360), "d2's tap samples are kept")
        await c.handle(.dictationCancel(id: "d2"))
        await c.dictation.echo?.settled()
        XCTAssertFalse(c.echoReference.isLive)
        XCTAssertTrue(logged(out.value, "slow failure"))
    }

    func testTheEchoGateSettingOffMeansNoTapAndTheRawMic() async {
        // N153: talk.echoGate off.
        let tap = FakeTap()
        let (c, out, _) = controller(tap: tap)
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "", echoGate: false))
        await dictate(c, system: { tap.cb.value?($0) })
        XCTAssertEqual(tap.calls.value, [])
        XCTAssertEqual(stoppedText(out.value), "echo u3 echo u4 echo")
    }

    func testARunningDictationKeepsItsGateWhenTheSettingChanges() {
        // N158: talk.echoGate is latched at begin(); a change applies from the next dictation.
        let ref = EchoReference()
        ref.attach("recording")
        let echo = DictationEcho(reference: ref, tap: nil, permitted: { true })
        func block(_ k: Int) -> Float {
            ref.push(sentence(k), from: "recording")
            return Float(DSP.rms(echo.process(sentence(k).map { 0.15 * $0 })))
        }
        func raw(_ k: Int) -> Float { Float(DSP.rms(sentence(k).map { 0.15 * $0 })) }
        echo.begin()
        for k in 0..<4 { _ = block(k) }
        XCTAssertLessThan(block(4), raw(4) / 4, "gated")
        echo.enabled = false
        XCTAssertLessThan(block(5), raw(5) / 4, "still gated after the setting went off mid-dictation")
        echo.end()
        echo.begin()
        XCTAssertEqual(block(6), raw(6), accuracy: 1e-4, "the next dictation is raw")
        echo.enabled = true
        XCTAssertEqual(block(7), raw(7), accuracy: 1e-4, "and stays raw until it ends")
        echo.end()
    }
}
