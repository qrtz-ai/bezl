import XCTest
@testable import QrtzHelperCore

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

    func testOtherEventsPassThroughWhileConsuming() throws {
        var f = try XCTUnwrap(F(key: "fn", consumeFn: true))
        // Typing, Fn+arrow (the Fn flag rides on the key event), and other modifiers are never dropped.
        for (type, code, flags) in [(F.keyDown, Int64(0), UInt64(0)), (F.keyDown, 123, F.fnFlag), (F.keyUp, 123, F.fnFlag),
                                    (F.flagsChanged, 56, UInt64(0x2_0000)), (F.flagsChanged, 61, F.optionFlag)] {
            let r = f.handle(type: type, keyCode: code, flags: flags)
            XCTAssertFalse(r.consume, "\(type) \(code)"); XCTAssertNil(r.action)
        }
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
        guard case .success(.dictationConfigure(_, _, _, "fn", false)) =
                HostMessage.parse(line: #"{"preRollMs":0,"pushKey":"fn","streamAudio":false,"type":"dictation.configure","warmMic":false,"consumeFn":false}"#)
        else { return XCTFail() }
        guard case .failure = HostMessage.parse(line: #"{"preRollMs":0,"pushKey":"fn","streamAudio":false,"type":"dictation.configure","warmMic":false,"consumeFn":"yes"}"#)
        else { return XCTFail() }
    }
}
