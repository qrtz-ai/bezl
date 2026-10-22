import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// The Fn/globe fix: with `fn` as the talk key and `talk.blockGlobeKey` on, the session tap swallows the Fn/globe
/// events (so macOS doesn't switch the input source) but still reports push-to-talk edges; nothing else is eaten.
final class HotkeyFilterTests: XCTestCase {
    typealias F = HotkeyFilter

    func testFnTalkKeyConsumesFnAndGlobeEventsAndStillReportsEdges() throws {
        var f = try XCTUnwrap(F(key: "fn", consumeFn: true))
        XCTAssertTrue(f.consumes)
        let down = f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: F.fnFlag)
        XCTAssertEqual(down.action, .down); XCTAssertTrue(down.consume)
        // Apple globe keyboards also send the globe key's own key down/up: eaten, no extra edges.
        XCTAssertEqual(f.handle(type: F.keyDown, keyCode: F.globeKeyCode, flags: F.fnFlag).consume, true)
        XCTAssertNil(f.handle(type: F.keyUp, keyCode: F.globeKeyCode, flags: F.fnFlag).action)
        let up = f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: 0)
        XCTAssertEqual(up.action, .up); XCTAssertTrue(up.consume)
    }

    /// ADR 0216: ctrl+fn is a ctrl press (⌃ held at fn's down edge), plain fn stays dictation, and the edges and
    /// the Fn consumption are the same either way.
    func testCtrlFnIsACtrlPressAndPlainFnIsDictation() throws {
        var f = try XCTUnwrap(F(key: "fn", consumeFn: true))
        let down = f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: F.fnFlag | F.controlFlag)
        XCTAssertEqual(down.action, .down); XCTAssertTrue(down.consume); XCTAssertTrue(f.withControl)
        // ⌃ let go before fn: still the ctrl press.
        _ = f.handle(type: F.flagsChanged, keyCode: 59, flags: F.fnFlag)
        let up = f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: 0)
        XCTAssertEqual(up.action, .up); XCTAssertTrue(f.withControl)
        // Plain fn: dictation.
        XCTAssertEqual(f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: F.fnFlag).action, .down)
        XCTAssertFalse(f.withControl)
        _ = f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: 0)
        // ⌃ pressed after fn went down: dictation.
        XCTAssertEqual(f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: F.fnFlag).action, .down)
        _ = f.handle(type: F.flagsChanged, keyCode: 59, flags: F.fnFlag | F.controlFlag)
        XCTAssertFalse(f.withControl)
        // Another talk key never makes a ctrl press.
        var o = try XCTUnwrap(F(key: "right-option", consumeFn: true))
        _ = o.handle(type: F.flagsChanged, keyCode: 61, flags: F.optionFlag | F.controlFlag)
        XCTAssertFalse(o.withControl)
    }

    /// Review M-E: a ⌃ that didn't come from the keyboard (a posted event, or ctrl's own marked one) never makes a
    /// ctrl press: the words are dictated as usual.
    func testOnlyAHardwareCtrlMakesACtrlPress() throws {
        var f = try XCTUnwrap(F(key: "fn", consumeFn: true))
        let both = F.fnFlag | F.controlFlag
        for (state, data) in [(Int64(0), Int64(0)), (Int64(2), Int64(0)), (F.hidSystemState, F.ctrlMarker)] {
            XCTAssertEqual(f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: both, sourceState: state, userData: data).action, .down)
            XCTAssertFalse(f.withControl, "state \(state) data \(data)")
            _ = f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: 0)
        }
        _ = f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: both, sourceState: F.hidSystemState, userData: 0)
        XCTAssertTrue(f.withControl)
        XCTAssertEqual(F.ctrlMarker, CtrlEventMarker.value, "the shared marker CtrlStopWatch.ctrlMarker also uses")
    }

    func testHotkeyLineCarriesCtrlOnlyWhenSet() throws {
        XCTAssertEqual(HelperMessage.hotkey(.down, ctrl: true).json.object?["ctrl"]?.bool, true)
        XCTAssertNil(HelperMessage.hotkey(.down).json.object?["ctrl"])
    }

    /// R2 (ADR 0216): a sleep/wake release says so; an ordinary edge carries no `sleep`.
    func testHotkeyLineCarriesSleepOnlyWhenSet() throws {
        XCTAssertEqual(HelperMessage.hotkey(.up, ctrl: true, sleep: true).json.object?["sleep"]?.bool, true)
        XCTAssertNil(HelperMessage.hotkey(.up, ctrl: true).json.object?["sleep"])
    }

    func testOtherEventsPassThroughWhileConsuming() throws {
        var f = try XCTUnwrap(F(key: "fn", consumeFn: true))
        // Typing, Fn+arrow (the Fn flag rides on the key event), and other modifiers are never dropped.
        for (type, code, flags) in [(F.keyDown, Int64(0), UInt64(0)), (F.keyDown, 123, F.fnFlag), (F.keyUp, 123, F.fnFlag),
                                    (F.flagsChanged, 56, UInt64(0x2_0000)), (F.flagsChanged, 61, F.optionFlag)] {
            let r = f.handle(type: type, keyCode: code, flags: flags)
            XCTAssertFalse(r.consume, "\(type) \(code)"); XCTAssertNil(r.action)
        }
    }

    /// Property: the tap can never become a keystroke source. For every key setting (with and without the globe
    /// block, key up or held), every event type and key code except Fn `flagsChanged` (63) and the globe key (179)
    /// gives `(nil, false)`: no push-to-talk edge, nothing dropped. The one other event that may carry an edge is the
    /// configured modifier's own `flagsChanged` (that is the push key itself), and it is never dropped. Fn and globe
    /// are only dropped with `fn` + the globe block on.
    func testNoOtherEventEverActsOrIsDropped() throws {
        let types: [UInt32] = Array(0...40) + [UInt32.max - 1, UInt32.max]
        let codes: [Int64] = Array(-2...300) + [Int64.min, Int64.max]
        let flagSets: [UInt64] = [0, F.fnFlag, F.optionFlag, F.commandFlag, F.controlFlag,
                                  F.fnFlag | F.optionFlag | F.commandFlag | F.controlFlag, UInt64.max]
        var checked = 0
        for key in ["fn", "right-option", "right-command", "right-control"] {
            for consumeFn in [false, true] {
                let up = try XCTUnwrap(F(key: key, consumeFn: consumeFn))
                var held = up
                XCTAssertEqual(held.handle(type: F.flagsChanged, keyCode: up.keyCode, flags: up.flag).action, .down)
                for base in [up, held] {
                    for type in types {
                        for code in codes {
                            let fnOrGlobe = (type == F.flagsChanged && code == F.fnKeyCode)
                                || ((type == F.keyDown || type == F.keyUp) && code == F.globeKeyCode)
                            let ownEdge = type == F.flagsChanged && code == up.keyCode
                            for flags in flagSets {
                                var f = base
                                let r = f.handle(type: type, keyCode: code, flags: flags)
                                let at = "\(key) consumeFn=\(consumeFn) held=\(base.down) type=\(type) code=\(code) flags=\(flags)"
                                if !fnOrGlobe && !ownEdge {
                                    if r.action != nil || r.consume { XCTFail(at); return }
                                    if f.down != base.down { XCTFail("state moved: \(at)"); return }
                                }
                                if r.consume && !(fnOrGlobe && key == "fn" && consumeFn) { XCTFail("dropped: \(at)"); return }
                                if r.action != nil && !ownEdge { XCTFail("edge: \(at)"); return }
                                checked += 1
                            }
                        }
                    }
                }
            }
        }
        XCTAssertEqual(checked, 4 * 2 * 2 * 43 * 305 * 7)
    }

    func testTheToggleOffLeavesFnToMacOS() throws {
        var f = try XCTUnwrap(F(key: "fn", consumeFn: false))
        XCTAssertFalse(f.consumes)
        let r = f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: F.fnFlag)
        XCTAssertEqual(r.action, .down); XCTAssertFalse(r.consume)
        XCTAssertFalse(f.handle(type: F.keyDown, keyCode: F.globeKeyCode, flags: F.fnFlag).consume)
    }

    func testOnlyWhileFnIsTheTalkKey() throws {
        var f = try XCTUnwrap(F(key: "right-option", consumeFn: true))
        XCTAssertFalse(f.consumes, "the right-hand modifiers are never consumed")
        XCTAssertFalse(f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: F.fnFlag).consume)
        XCTAssertFalse(f.handle(type: F.keyDown, keyCode: F.globeKeyCode, flags: 0).consume)
        let r = f.handle(type: F.flagsChanged, keyCode: 61, flags: F.optionFlag)
        XCTAssertEqual(r.action, .down); XCTAssertFalse(r.consume)
        XCTAssertNil(F(key: "caps-lock", consumeFn: true))
    }

    func testResetAfterTheTapIsDisabledReleasesAHeldKey() throws {
        var f = try XCTUnwrap(F(key: "fn", consumeFn: true))
        _ = f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: F.fnFlag)
        XCTAssertEqual(f.reset(), .up)
        XCTAssertNil(f.reset())
        XCTAssertEqual(f.handle(type: F.flagsChanged, keyCode: F.fnKeyCode, flags: F.fnFlag).action, .down)
    }

    final class FakeHotkeys: HotkeyMonitor, @unchecked Sendable {
        var starts: [(String, Bool)] = []
        func start(key: String, consumeFn: Bool, onChange: @escaping @Sendable (HotkeyAction) -> Void) -> Bool {
            starts.append((key, consumeFn)); return true
        }
        func stop() {}
    }

    func testConfigurePassesTheToggleAndRestartsTheMonitorWhenItChanges() async {
        let hk = FakeHotkeys()
        let c = HelperController(
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux", features: [],
                             dictationMic: FakeMic(), dictationTranscribers: [], insertion: nil, hotkeys: hk),
            send: { _ in }, now: { 0 })
        // An older host sends no consumeFn: on by default.
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "fn"))
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "fn", consumeFn: true))
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "fn", consumeFn: false))
        XCTAssertEqual(hk.starts.map { "\($0.0):\($0.1)" }, ["fn:true", "fn:false"])
    }

    func testConsumeFnOnTheWire() {
        guard case .success(.dictationConfigure(_, _, _, "fn", false, _, _, _, _)) =
                HostMessage.parse(line: #"{"preRollMs":0,"pushKey":"fn","streamAudio":false,"type":"dictation.configure","warmMic":false,"consumeFn":false}"#)
        else { return XCTFail() }
        guard case .failure = HostMessage.parse(line: #"{"preRollMs":0,"pushKey":"fn","streamAudio":false,"type":"dictation.configure","warmMic":false,"consumeFn":"yes"}"#)
        else { return XCTFail() }
    }

    /// N196 (ADR 0224): a lid closed while holding fn ends the press at sleep; a wake with the key still believed
    /// held ends it only when the session's flags show fn up.
    func testSleepAndWakeEndAHeldKey() {
        var f = HotkeyFilter(key: "fn", consumeFn: true)!
        XCTAssertNil(f.willSleep(), "nothing held: nothing to end")
        _ = f.handle(type: HotkeyFilter.flagsChanged, keyCode: HotkeyFilter.fnKeyCode, flags: HotkeyFilter.fnFlag)
        XCTAssertEqual(f.willSleep(), .up)
        XCTAssertFalse(f.down)
        XCTAssertNil(f.didWake(flags: 0), "already ended at sleep")
        // The sleep notice never came: the wake decides from the live flags.
        _ = f.handle(type: HotkeyFilter.flagsChanged, keyCode: HotkeyFilter.fnKeyCode, flags: HotkeyFilter.fnFlag)
        XCTAssertNil(f.didWake(flags: HotkeyFilter.fnFlag), "still really held: keeps going")
        XCTAssertTrue(f.down)
        XCTAssertEqual(f.didWake(flags: 0), .up)
        XCTAssertFalse(f.down)
        // The next real press starts clean.
        XCTAssertEqual(f.handle(type: HotkeyFilter.flagsChanged, keyCode: HotkeyFilter.fnKeyCode, flags: HotkeyFilter.fnFlag).action, .down)
    }

    /// Bozniak R2/MEDIUM (ADR 0216): a release the monitor makes up (sleep, wake, tap disabled) goes out as
    /// `hotkey{up, sleep: true}` through the engine, only for that edge; a real release carries no `sleep`.
    final class SyntheticHotkeys: HotkeyMonitor, @unchecked Sendable {
        let synthetic = SyntheticRelease()
        var filter = HotkeyFilter(key: "fn", consumeFn: true)!
        var onChange: (@Sendable (HotkeyAction) -> Void)?
        var pressWithControl: Bool { filter.withControl }
        var releaseAtSleep: Bool { synthetic.active }
        func start(key: String, consumeFn: Bool, onChange: @escaping @Sendable (HotkeyAction) -> Void) -> Bool {
            self.onChange = onChange; return true
        }
        func stop() {}
        func press(ctrl: Bool) {
            let r = filter.handle(type: HotkeyFilter.flagsChanged, keyCode: HotkeyFilter.fnKeyCode,
                                  flags: HotkeyFilter.fnFlag | (ctrl ? HotkeyFilter.controlFlag : 0))
            if let a = r.action { onChange?(a) }
        }
        func release() {
            let r = filter.handle(type: HotkeyFilter.flagsChanged, keyCode: HotkeyFilter.fnKeyCode, flags: 0)
            if let a = r.action { onChange?(a) }
        }
    }

    final class Lines: @unchecked Sendable {
        private let lock = NSLock(); private var all: [JSONValue] = []
        func add(_ v: JSONValue) { lock.lock(); all.append(v); lock.unlock() }
        var hotkeys: [JSONValue] { lock.lock(); defer { lock.unlock() }; return all.filter { $0.object?["type"]?.string == "hotkey" } }
    }

    func testSyntheticReleasesSetReleaseAtSleepOnlyForThatEdge() async throws {
        let hk = SyntheticHotkeys()
        let lines = Lines()
        let c = HelperController(
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux", features: [],
                             dictationMic: FakeMic(), dictationTranscribers: [], insertion: nil, hotkeys: hk),
            send: { m in lines.add(m.json) }, now: { 0 })
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "fn", consumeFn: true))
        // Tap disabled while ctrl+fn is held (MEDIUM), then sleep and wake (R2): each made-up release says so.
        hk.press(ctrl: true); hk.synthetic.report(hk.filter.reset(), to: hk.onChange)
        XCTAssertFalse(hk.releaseAtSleep, "cleared once the edge is reported")
        hk.press(ctrl: true); hk.synthetic.report(hk.filter.willSleep(), to: hk.onChange)
        hk.press(ctrl: true); hk.synthetic.report(hk.filter.didWake(flags: 0), to: hk.onChange)
        hk.synthetic.report(hk.filter.reset(), to: hk.onChange)  // nothing held: no edge
        // A real release carries no `sleep`.
        hk.press(ctrl: true); hk.release()
        try await Task.sleep(nanoseconds: 50_000_000)
        let ups = lines.hotkeys.filter { $0.object?["action"]?.string == "up" }
        XCTAssertEqual(ups.count, 4)
        XCTAssertEqual(ups.prefix(3).map { $0.object?["sleep"]?.bool }, [true, true, true])
        XCTAssertEqual(ups.prefix(3).map { $0.object?["ctrl"]?.bool }, [true, true, true])
        XCTAssertNil(ups[3].object?["sleep"])
        XCTAssertEqual(lines.hotkeys.filter { $0.object?["action"]?.string == "down" }.compactMap { $0.object?["sleep"] }.count, 0)
    }

    /// Snap's summon chord (left ⌘ + right ⌘): with right ⌘ as the talk key, right ⌘ pressed while left ⌘ is held
    /// starts no dictation; right ⌘ alone still does.
    func testRightCommandWithLeftCommandHeldIsNotPushToTalk() throws {
        var f = try XCTUnwrap(F(key: "right-command", consumeFn: true))
        let left: UInt64 = F.commandFlag | F.leftCommandDevice
        _ = f.handle(type: F.flagsChanged, keyCode: 55, flags: left)
        XCTAssertNil(f.handle(type: F.flagsChanged, keyCode: 54, flags: left | 0x10).action)
        XCTAssertNil(f.handle(type: F.flagsChanged, keyCode: 54, flags: left).action)
        _ = f.handle(type: F.flagsChanged, keyCode: 55, flags: 0)
        XCTAssertEqual(f.handle(type: F.flagsChanged, keyCode: 54, flags: F.commandFlag | 0x10).action, .down)
        XCTAssertEqual(f.handle(type: F.flagsChanged, keyCode: 54, flags: 0).action, .up)
    }
}
