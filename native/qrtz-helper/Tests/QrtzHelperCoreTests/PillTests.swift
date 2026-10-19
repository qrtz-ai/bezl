import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

final class FakePillView: PillView, @unchecked Sendable {
    let lock = NSLock()
    var shows: [PillPhase] = []
    var levels: [Double] = []
    func show(_ p: PillPhase) { lock.withLock { shows.append(p) } }
    func level(_ rms: Double) { lock.withLock { levels.append(rms) } }
    var all: [PillPhase] { lock.withLock { shows } }
}

final class PillDriverTests: XCTestCase {
    func fast() -> PillDriver.Timing {
        var t = PillDriver.Timing()
        t.errorFlashMs = 40; t.startTimeoutMs = 60; t.insertTimeoutMs = 60
        return t
    }

    func sleep(_ ms: UInt64) async { try? await Task.sleep(nanoseconds: ms * 1_000_000) }

    func testKeyPressToInsertShowsListeningThenProcessingThenHides() {
        let v = FakePillView(), d = PillDriver(view: v)
        d.helper(.hotkey(.down))
        XCTAssertEqual(v.all, [.listening])  // at the press, before the host answers
        d.host(.dictationStart(id: "d1"))
        d.helper(.dictationStarted(id: "d1", preRollMs: 0))
        d.helper(.dictationLevel(rms: 0.3))
        XCTAssertEqual(v.levels, [0.3])
        d.helper(.hotkey(.up))
        d.host(.dictationStop(id: "d1"))
        XCTAssertEqual(d.state, .finalizing)
        d.helper(.dictationLevel(rms: 0.5))
        XCTAssertEqual(v.levels, [0.3])  // no level while processing
        d.helper(.dictationStopped(id: "d1", text: "hello", durationMs: 900))
        XCTAssertEqual(d.state, .inserting)
        d.host(.insertText(id: "d1", text: "hello", restoreClipboard: true))
        XCTAssertEqual(v.all, [.listening, .processing])  // up through finalizing and inserting
        d.helper(.insertResult(id: "d1", method: .axSelectedText, ok: true, app: "Notes", error: nil))
        XCTAssertEqual(v.all, [.listening, .processing, .hidden])
    }

    func testEmptyOrCancelledDictationHides() {
        let v = FakePillView(), d = PillDriver(view: v)
        d.helper(.dictationStarted(id: "a", preRollMs: 0))
        d.helper(.dictationStopped(id: "a", text: "  ", durationMs: 100))
        XCTAssertEqual(d.state, .hidden)
        d.helper(.dictationStarted(id: "b", preRollMs: 0))
        d.host(.dictationCancel(id: "b"))
        XCTAssertEqual(v.all, [.listening, .hidden, .listening, .hidden])
    }

    func testAnErrorFlashesLowercaseThenHides() async {
        let v = FakePillView(), d = PillDriver(view: v, timing: fast())
        d.helper(.hotkey(.down))
        d.helper(.error(code: "dictation.asr", message: "Speech recognition isn't ready yet", fatal: false, id: "d1"))
        XCTAssertEqual(d.state, .error("speech recognition isn't ready yet"))
        await sleep(150)
        XCTAssertEqual(v.all, [.listening, .error("speech recognition isn't ready yet"), .hidden])
    }

    func testFailedInsertAndLostMicAreErrors() async {
        let v = FakePillView(), d = PillDriver(view: v)
        d.helper(.dictationStarted(id: "a", preRollMs: 0))
        d.helper(.dictationStopped(id: "a", text: "hi", durationMs: 100))
        d.helper(.insertResult(id: "a", method: .none, ok: false, app: nil, error: "No Focused Field"))
        XCTAssertEqual(d.state, .inserting, "waits for the host's pill.flash")
        try? await Task.sleep(nanoseconds: 600_000_000)  // no flash from the host: the helper's own error
        XCTAssertEqual(d.state, .error("no focused field"))
        d.helper(.dictationStarted(id: "b", preRollMs: 0))
        d.helper(.dictationStopped(id: "b", text: "", durationMs: 100, reason: .micLost))
        XCTAssertEqual(d.state, .error("the microphone went away"))
    }

    func testHelperErrorsWhileHiddenOrBusyDontPopThePill() {
        let v = FakePillView(), d = PillDriver(view: v)
        d.helper(.error(code: "dictation.hotkey", message: "push-to-talk key fn unavailable", fatal: false))
        XCTAssertEqual(v.all, [])
        d.helper(.dictationStarted(id: "a", preRollMs: 0))
        d.helper(.hotkey(.up))
        d.helper(.error(code: "dictation.busy", message: "busy", fatal: false, id: "b"))
        XCTAssertEqual(d.state, .finalizing)
        d.helper(.error(code: "audio.unavailable", message: "x", fatal: false))
        XCTAssertEqual(d.state, .finalizing)
    }

    func testAPressThatNeverStartsAndAnInsertThatNeverLandsTimeOut() async {
        let v = FakePillView(), d = PillDriver(view: v, timing: fast())
        d.helper(.hotkey(.down))
        await sleep(150)
        XCTAssertEqual(d.state, .hidden)
        // A started dictation is not cut by the start timeout.
        d.helper(.hotkey(.down))
        d.helper(.dictationStarted(id: "a", preRollMs: 0))
        await sleep(150)
        XCTAssertEqual(d.state, .listening)
        d.helper(.dictationStopped(id: "a", text: "hi", durationMs: 100))
        await sleep(150)
        XCTAssertEqual(d.state, .hidden)
    }

    func testARefusedPressEndsAfterTheStartTimeoutEvenIfReleased() async {
        // N145: the host refused the press before dictation.start (and its repeated flash may be deduplicated):
        // the release can't wait for a stop that never comes.
        let v = FakePillView(), d = PillDriver(view: v, timing: fast())
        d.helper(.hotkey(.down))
        d.helper(.hotkey(.up))
        XCTAssertEqual(d.state, .finalizing)
        await sleep(150)
        XCTAssertEqual(d.state, .hidden)
    }

    func testAStartedReleaseThatNeverStopsHitsTheFinalizeBackstop() async {
        // N145: a started dictation's release is bounded by echo's own release budget.
        var t = fast(); t.finalizeBackstopMs = 80
        let v = FakePillView(), d = PillDriver(view: v, timing: t)
        d.helper(.hotkey(.down))
        d.host(.dictationStart(id: "d1"))
        d.helper(.dictationStarted(id: "d1", preRollMs: 0))
        d.helper(.hotkey(.up))
        await sleep(40)
        XCTAssertEqual(d.state, .finalizing, "not cut by the 60 ms start timeout")
        await sleep(120)
        XCTAssertEqual(d.state, .hidden)
        XCTAssertEqual(PillDriver.Timing().finalizeBackstopMs, nil, "the controller fills it from its DictationTail")
    }

    func testBackToBackDictationsKeepTheNewerPill() {
        // N146 / N110: d2 is pressed while d1 inserts; d1's insert.result must not hide d2, and d2's levels show.
        let v = FakePillView(), d = PillDriver(view: v)
        d.helper(.hotkey(.down))
        d.host(.dictationStart(id: "d1"))
        d.helper(.dictationStarted(id: "d1", preRollMs: 0))
        d.helper(.hotkey(.up))
        d.helper(.dictationStopped(id: "d1", text: "one", durationMs: 500))
        d.host(.insertText(id: "d1", text: "one", restoreClipboard: true))
        XCTAssertEqual(d.state, .inserting)
        d.helper(.hotkey(.down))
        XCTAssertEqual(d.state, .listening, "a press during the insert shows the next dictation")
        d.host(.dictationStart(id: "d2"))
        d.helper(.dictationStarted(id: "d2", preRollMs: 0))
        d.helper(.insertResult(id: "d1", method: .axSelectedText, ok: true, app: "Notes", error: nil))
        XCTAssertEqual(d.state, .listening)
        XCTAssertEqual(d.dictationId, "d2")
        d.helper(.dictationLevel(rms: 0.4))
        XCTAssertEqual(v.levels, [0.4])
        d.helper(.insertResult(id: "d1", method: .axSelectedText, ok: false, app: "Notes", error: "late"))
        d.helper(.error(code: "dictation.asr", message: "old", fatal: false, id: "d1"))
        XCTAssertEqual(d.state, .listening, "nothing of d1 moves the pill any more")
        d.helper(.hotkey(.up))
        d.helper(.dictationStopped(id: "d2", text: "two", durationMs: 500))
        d.helper(.insertResult(id: "d2", method: .axSelectedText, ok: true, app: "Notes", error: nil))
        XCTAssertEqual(v.all, [.listening, .processing, .listening, .processing, .hidden])
    }

    func testAPressQueuedDuringFinalizingFollowsTheNewDictation() {
        // N146, the other order: d1 still finalizing when d2 is pressed; d1's stopped arrives while d2 listens.
        let v = FakePillView(), d = PillDriver(view: v)
        d.helper(.hotkey(.down))
        d.host(.dictationStart(id: "d1"))
        d.helper(.dictationStarted(id: "d1", preRollMs: 0))
        d.helper(.hotkey(.up))
        d.helper(.hotkey(.down))
        d.helper(.dictationStopped(id: "d1", text: "one", durationMs: 500))
        XCTAssertEqual(d.state, .listening)
        d.host(.insertText(id: "d1", text: "one", restoreClipboard: true))
        XCTAssertEqual(d.state, .listening)
        d.host(.dictationStart(id: "d2"))
        d.helper(.dictationStarted(id: "d2", preRollMs: 0))
        XCTAssertEqual(d.state, .listening)
    }

    func testAPressQueuedBehindALongFinalizeDoesntHideAtTheStartTimeout() {
        // N151: d2 is pressed while d1 finalizes for longer than the 2 s start timeout; talk sends d2's start late.
        // A manual clock: no sleeps, nothing races the wall clock.
        let clock = ManualClock()
        var t = PillDriver.Timing(); t.finalizeBackstopMs = 10_000
        let v = FakePillView(), d = PillDriver(view: v, timing: t, after: clock.after)
        d.helper(.hotkey(.down))
        d.host(.dictationStart(id: "d1"))
        d.helper(.dictationStarted(id: "d1", preRollMs: 0))
        d.helper(.hotkey(.up))
        d.helper(.hotkey(.down))
        clock.advance(5_000)
        XCTAssertEqual(d.state, .listening, "not hidden at the start timeout")
        d.helper(.dictationStopped(id: "d1", text: "one", durationMs: 500))
        d.host(.dictationStart(id: "d2"))
        d.helper(.dictationStarted(id: "d2", preRollMs: 0))
        clock.advance(20_000)
        XCTAssertEqual(d.state, .listening, "a started dictation has no start timeout")
        XCTAssertEqual(v.all, [.listening, .processing, .listening])
        // A press with nothing in flight still times out at 2 s if the host never starts it.
        let c3 = ManualClock()
        let d3 = PillDriver(view: FakePillView(), timing: t, after: c3.after)
        d3.helper(.hotkey(.down))
        c3.advance(1_999)
        XCTAssertEqual(d3.state, .listening)
        c3.advance(1)
        XCTAssertEqual(d3.state, .hidden)
    }

    func testAPressQueuedBehindAnInsertDoesntHideAtTheStartTimeout() {
        // N157: same while d1 is inserting (talk queues d2 until d1's insert is done).
        let clock = ManualClock()
        var t = PillDriver.Timing(); t.finalizeBackstopMs = 10_000; t.insertTimeoutMs = 60_000
        let d = PillDriver(view: FakePillView(), timing: t, after: clock.after)
        d.helper(.hotkey(.down))
        d.host(.dictationStart(id: "d1"))
        d.helper(.dictationStarted(id: "d1", preRollMs: 0))
        d.helper(.hotkey(.up))
        d.helper(.dictationStopped(id: "d1", text: "one", durationMs: 500))
        XCTAssertEqual(d.state, .inserting)
        d.helper(.hotkey(.down))
        clock.advance(5_000)
        XCTAssertEqual(d.state, .listening, "not hidden at the start timeout")
        d.host(.dictationStart(id: "d2"))
        XCTAssertEqual(d.state, .listening)
        clock.advance(9_999)
        XCTAssertEqual(d.state, .listening)
    }

    func testAColdLoadAfterTheHostsStartDoesntHideThePill() async {
        // N147: the start timeout runs only until the host's dictation.start; the recogniser may load for ~26 s.
        let v = FakePillView(), d = PillDriver(view: v, timing: fast())
        d.helper(.hotkey(.down))
        d.host(.dictationStart(id: "d1"))
        await sleep(150)
        XCTAssertEqual(d.state, .listening)
        XCTAssertEqual(v.all, [.listening])
        d.helper(.dictationStarted(id: "d1", preRollMs: 0))
        XCTAssertEqual(d.state, .listening)
    }

    func testWithAutoInsertOffThePillEndsAtStopped() async {
        // N147: no insert.text will come; don't sit on processing for the insert timeout.
        let v = FakePillView(), d = PillDriver(view: v)
        d.autoInsert = false
        d.helper(.hotkey(.down))
        d.host(.dictationStart(id: "d1"))
        d.helper(.dictationStarted(id: "d1", preRollMs: 0))
        d.helper(.hotkey(.up))
        d.helper(.dictationStopped(id: "d1", text: "hello", durationMs: 500))
        XCTAssertEqual(d.state, .hidden)
    }

    func testConfigureCarriesAutoInsertToThePill() async {
        let v = FakePillView()
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux",
                              features: [], dictationMic: FakeMic(), dictationTranscribers: [])
        deps.pill = v
        let c = HelperController(deps: deps, send: { _ in }, now: { 0 })
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "", autoInsert: false))
        XCTAssertEqual(c.pill?.autoInsert, false)
        let line = HostMessage.parse(line: #"{"type":"dictation.configure","warmMic":false,"preRollMs":0,"streamAudio":false,"pushKey":"","autoInsert":false}"#)
        guard case .success(.dictationConfigure(_, _, _, _, _, _, false, _)) = line else { return XCTFail("autoInsert parsed: \(line)") }
        guard case .failure = HostMessage.parse(line: #"{"type":"dictation.configure","warmMic":false,"preRollMs":0,"streamAudio":false,"pushKey":"","autoInsert":1}"#) else { return XCTFail("a non-bool autoInsert is refused") }
    }

    func testAccessibilityLabels() {
        XCTAssertEqual(PillPhase.listening.accessibilityLabel, "listening")
        XCTAssertEqual(PillPhase.processing.accessibilityLabel, "processing")
        XCTAssertEqual(PillPhase.error("mic off").accessibilityLabel, "mic off")
    }

    /// Through the controller: the helper's own dictation lines drive the pill.
    func testTheControllerDrivesThePill() async {
        let v = FakePillView(), mic = FakeMic()
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux",
                              features: [], dictationMic: mic, dictationTranscribers: [RecordingASR()], insertion: FakeInsertion())
        deps.pill = v
        deps.preloadDictationAsr = false
        deps.dictationTail = DictationTail(tailMs: 0, drainTimeoutMs: 0, padMs: 0)
        let c = HelperController(deps: deps, send: { _ in }, now: { 0 })
        await c.handle(.dictationStart(id: "d1"))
        await c.dictation.onSamples([Float](repeating: 0.2, count: 1600))
        await c.dictation.poll()
        await c.handle(.dictationStop(id: "d1"))
        XCTAssertEqual(c.pill?.state, .inserting)
        await c.handle(.insertText(id: "d1", text: "hello world", restoreClipboard: false))
        XCTAssertEqual(v.all, [.listening, .processing, .hidden])
    }

    func testANoRecogniserPressFlashesTheError() async {
        let v = FakePillView()
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux",
                              features: [], dictationMic: FakeMic(), dictationTranscribers: [])
        deps.pill = v
        let c = HelperController(deps: deps, send: { _ in }, now: { 0 })
        await c.handle(.dictationStart(id: "d1"))
        guard case .error = c.pill?.state else { return XCTFail("\(String(describing: c.pill?.state))") }
    }
}

final class PillHostTests: XCTestCase {
    func testAClipboardFallbackIsInfoNotAnError() async {
        let v = FakePillView(), d = PillDriver(view: v)
        d.helper(.dictationStarted(id: "a", preRollMs: 0))
        d.helper(.dictationStopped(id: "a", text: "hi", durationMs: 100))
        d.helper(.insertResult(id: "a", method: .none, ok: false, app: nil, error: "no focused field"))
        d.host(.pillFlash(kind: .info, message: "Copied to clipboard"))
        try? await Task.sleep(nanoseconds: 600_000_000)
        XCTAssertEqual(v.all, [.listening, .processing, .info("copied to clipboard")], "no error shown first")
        XCTAssertEqual(PillPhase.info("copied to clipboard").accessibilityLabel, "copied to clipboard")
    }

    func testAHostOnlyFailureFlashesOnAHiddenPill() async {
        var t = PillDriver.Timing(); t.errorFlashMs = 40
        let v = FakePillView(), d = PillDriver(view: v, timing: t)
        d.host(.pillFlash(kind: .error, message: "Couldn't insert; copied to clipboard"))
        XCTAssertEqual(d.state, .error("couldn't insert; copied to clipboard"))
        try? await Task.sleep(nanoseconds: 150_000_000)
        XCTAssertEqual(d.state, .hidden)
    }

    func testShowPillOffShowsNothing() async {
        let v = FakePillView()
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux",
                              features: [], dictationMic: FakeMic(), dictationTranscribers: [RecordingASR()], insertion: FakeInsertion())
        deps.pill = v
        deps.preloadDictationAsr = false
        let c = HelperController(deps: deps, send: { _ in }, now: { 0 })
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "", pill: false))
        await c.handle(.dictationStart(id: "d1"))
        await c.handle(.pillFlash(kind: .error, message: "x"))
        XCTAssertFalse(v.all.contains { $0 != .hidden })
        await c.handle(.dictationCancel(id: "d1"))
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "", pill: true))
        await c.handle(.dictationStart(id: "d2"))
        XCTAssertEqual(v.all.last, .listening)
    }

    func testTheLinesRoundTrip() throws {
        for m: HostMessage in [.pillFlash(kind: .info, message: "copied to clipboard"),
                               .dictationConfigure(warmMic: true, preRollMs: 0, streamAudio: false, pushKey: "fn", consumeFn: false, pill: false)] {
            XCTAssertEqual(try HostMessage.parse(line: m.json.serialized()).get(), m)
        }
        XCTAssertThrowsError(try HostMessage.parse(line: #"{"type":"pill.flash","kind":"warn","message":"x"}"#).get())
        XCTAssertThrowsError(try HostMessage.parse(line: #"{"type":"pill.flash","kind":"info","message":""}"#).get())
    }
}

final class PillWaveTests: XCTestCase {
    func testTargetAndEasing() {
        XCTAssertEqual(PillWave.target(.live, 0), 0.06)
        XCTAssertEqual(PillWave.target(.live, 1), 1, accuracy: 1e-12)
        XCTAssertEqual(PillWave.target(.live, 0.25), 0.06 + 0.94 * 0.5, accuracy: 1e-12)
        XCTAssertEqual(PillWave.target(.idle, 0.9), 0.14)
        // ~60 ms rise, ~220 ms fall (one time constant: 1 - 1/e of the way).
        XCTAssertEqual(PillWave.step(0, 1, 60), 1 - exp(-1), accuracy: 1e-12)
        XCTAssertEqual(PillWave.step(1, 0, 220), exp(-1), accuracy: 1e-12)
        XCTAssertEqual(PillWave.step(0, 1, 10_000), PillWave.step(0, 1, 250))  // a long frame is capped
        XCTAssertEqual(PillWave.target(.live, .nan), 0.06)
    }

    func testTheWaveIsPinnedAtTheEdgesAndThreeLayers() {
        let f = PillWave.frame(amp: 1, phase: 0.7)
        XCTAssertEqual(f.count, 3)
        for l in f {
            XCTAssertEqual(l.count, 49)
            XCTAssertEqual(l.first!.y, 12, accuracy: 1e-9)
            XCTAssertEqual(l.last!.y, 12, accuracy: 1e-9)
            XCTAssertEqual(l.last!.x, 160)
            XCTAssertTrue(l.allSatisfy { $0.y >= 1 && $0.y <= 23 })
        }
    }

    static let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../../..").standardized

    /// The native pill and the web Waveform must not drift apart (62b99fc's design).
    func testTheConstantsMatchTheWebWaveform() throws {
        let ts = try String(contentsOf: Self.repo.appendingPathComponent("packages/ui/src/Waveform.tsx"), encoding: .utf8)
        for needle in ["WAVE_LIVE_FLOOR = 0.06", "WAVE_IDLE_AMP = 0.14", "WAVE_ATTACK_MS = 60", "WAVE_RELEASE_MS = 220",
                       "live: 7, idle: 2.2", "{ cycles: 1.6, gain: 1, opacity: 1, rate: 1, offset: 0 }",
                       "{ cycles: 2.3, gain: 0.66, opacity: 0.5, rate: -1.3, offset: 1.9 }",
                       "{ cycles: 3.1, gain: 0.4, opacity: 0.28, rate: 1.7, offset: 4.1 }", "points = 48",
                       "Math.pow(Math.sin(Math.PI * clamp01(u)), 1.6)", "Math.min(dtMs, 250)"] {
            XCTAssertTrue(ts.contains(needle), needle)
        }
    }

    func testTheEmbeddedFontIsTheRenderersFile() throws {
        let file = try Data(contentsOf: Self.repo.appendingPathComponent("packages/ui/src/fonts/GeistPixel-Square.woff2"))
        XCTAssertEqual(PillFont.woff2, file, "run native/qrtz-helper/scripts/gen-pill-font.sh")
        XCTAssertEqual(String(decoding: file.prefix(4), as: UTF8.self), "wOF2")
    }
}

/// A clock the test moves: `PillDriver`'s timeouts fire in `advance`, in due order, on the test's thread.
final class ManualClock: @unchecked Sendable {
    private let lock = NSLock()
    private var now = 0
    private var seq = 0
    private var timers: [(due: Int, seq: Int, fire: @Sendable () -> Void)] = []

    var after: PillDriver.After {
        { [self] ms, fire in lock.withLock { seq += 1; timers.append((now + max(0, ms), seq, fire)) } }
    }

    func advance(_ ms: Int) {
        let end = lock.withLock { now + ms }
        while true {
            let next: (due: Int, seq: Int, fire: @Sendable () -> Void)? = lock.withLock {
                guard let i = timers.indices.filter({ timers[$0].due <= end }).min(by: { (timers[$0].due, timers[$0].seq) < (timers[$1].due, timers[$1].seq) })
                else { return nil }
                let t = timers.remove(at: i)
                now = t.due
                return t
            }
            guard let next else { break }
            next.fire()
        }
        lock.withLock { now = end }
    }
}
