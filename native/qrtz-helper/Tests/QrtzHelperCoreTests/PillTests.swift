import Foundation
import XCTest
@testable import QrtzHelperCore

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

    func testFailedInsertAndLostMicAreErrors() {
        let v = FakePillView(), d = PillDriver(view: v)
        d.helper(.dictationStarted(id: "a", preRollMs: 0))
        d.helper(.dictationStopped(id: "a", text: "hi", durationMs: 100))
        d.helper(.insertResult(id: "a", method: .none, ok: false, app: nil, error: "No Focused Field"))
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

    func testGeometry() {
        let f = PillGeometry.frame(visibleX: 0, y: 70, width: 1440, height: 800)
        XCTAssertEqual(f.x, 600); XCTAssertEqual(f.y, 94); XCTAssertEqual(f.w, 240); XCTAssertEqual(f.h, 52)
        XCTAssertEqual(PillGeometry.errorTextWidth, 184)
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
        let file = try Data(contentsOf: Self.repo.appendingPathComponent("packages/ui/src/fonts/GeistPixel-Circle.woff2"))
        XCTAssertEqual(PillFont.woff2, file, "run native/qrtz-helper/scripts/gen-pill-font.sh")
        XCTAssertEqual(String(decoding: file.prefix(4), as: UTF8.self), "wOF2")
    }
}
