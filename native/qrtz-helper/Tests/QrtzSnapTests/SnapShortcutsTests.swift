import XCTest
@testable import QrtzSnap

final class SnapShortcutsTests: XCTestCase {
    func testDefaultsAreSpectaclesAndAllRegister() {
        XCTAssertEqual(SnapShortcuts.defaults.count, SnapAction.allCases.count)
        let r = SnapShortcuts.resolve(overrides: [:])
        XCTAssertEqual(r.refused, [:])
        XCTAssertEqual(r.shortcuts.count, 18)
        XCTAssertEqual(r.shortcuts[.leftHalf]?.description, "alt+cmd+left")
        XCTAssertEqual(r.shortcuts[.redo]?.description, "alt+shift+cmd+z")
        XCTAssertEqual(r.shortcuts[.makeLarger]?.description, "ctrl+alt+shift+right")
        XCTAssertEqual(Set(r.shortcuts.values).count, 18, "no two defaults share keys")
    }

    func testCarbonCodesAndModifiers() {
        let s = SnapShortcut(binding: "ctrl+alt+cmd+right")!
        XCTAssertEqual(s.keyCode, 124)
        XCTAssertEqual(s.carbonModifiers, 256 | 2048 | 4096)
        XCTAssertEqual(SnapShortcut(binding: "alt+cmd+c")!.keyCode, 8)
        XCTAssertEqual(SnapShortcut(binding: "shift+f13")!.keyCode, 105)
    }

    func testParsingAcceptsAliasesAndAnyOrder() {
        XCTAssertEqual(SnapShortcut(binding: "Command+Option+Left")?.description, "alt+cmd+left")
        XCTAssertNil(SnapShortcut(binding: "hyper+left"))
        XCTAssertNil(SnapShortcut(binding: "cmd+"))
        XCTAssertNil(SnapShortcut(binding: "cmd+nokey"))
    }

    func testOverridesClearAndRefuse() {
        let r = SnapShortcuts.resolve(overrides: [
            "center": "",
            "fullscreen": "ctrl+alt+return",
            "topHalf": "shift+t",
            "bottomHalf": "alt+cmd+left",
            "nonsense": "cmd+k",
        ])
        XCTAssertNil(r.shortcuts[.center])
        XCTAssertNil(r.refused[.center], "cleared is not refused")
        XCTAssertEqual(r.shortcuts[.fullscreen]?.description, "ctrl+alt+return")
        XCTAssertEqual(r.refused[.topHalf], "needs ⌘, ⌥ or ⌃")
        XCTAssertEqual(r.refused[.bottomHalf], "same keys as left half")
        XCTAssertEqual(r.refused[.leftHalf], "same keys as bottom half")
        XCTAssertNil(r.shortcuts[.leftHalf])
    }

    func testFunctionKeysNeedNoModifier() {
        XCTAssertTrue(SnapShortcut(binding: "f13")!.isValid)
        XCTAssertFalse(SnapShortcut(binding: "x")!.isValid)
    }
}

final class FakeBackend: SnapBackend {
    var axTrusted = true
    var taken: Set<SnapAction> = []
    var registered: [SnapAction: SnapShortcut] = [:]
    var disabled: Set<String> = []
    var status = false
    func register(_ s: [SnapAction: SnapShortcut]) -> [SnapAction: String] {
        registered = s.filter { !taken.contains($0.key) }
        return Dictionary(uniqueKeysWithValues: taken.map { ($0, "another app uses these keys") })
    }
    func perform(_ a: SnapAction) -> SnapOutcome { .moved(SnapRect(x: 0, y: 0, width: 1, height: 1)) }
    func setDisabledApps(_ apps: Set<String>) { disabled = apps }
    func setStatusItem(_ shown: Bool) { status = shown }
}

final class SnapProtocolTests: XCTestCase {
    func obj(_ s: String) -> [String: Any] {
        (try! JSONSerialization.jsonObject(with: Data(s.utf8))) as! [String: Any]
    }

    func testHelloSaysWhichModule() {
        let h = obj(SnapLines.hello(version: "0.1.0"))
        XCTAssertEqual(h["type"] as? String, "hello")
        XCTAssertEqual(h["features"] as? [String], ["module:snap"])
    }

    func testConfigureRegistersAndReportsStatus() {
        let b = FakeBackend()
        b.taken = [.undo]
        var lines: [String] = []
        let s = SnapServer(backend: b, write: { lines.append($0) })
        s.handle(#"{"type":"snap.configure","shortcuts":{"center":"","redo":"shift+z"},"disabledApps":["com.a"],"statusItem":false}"#)
        XCTAssertEqual(b.disabled, ["com.a"])
        XCTAssertFalse(b.status)
        XCTAssertNil(b.registered[.center])
        let st = obj(lines.last!)
        XCTAssertEqual(st["type"] as? String, "snap.status")
        XCTAssertEqual(st["axTrusted"] as? Bool, true)
        let refused = st["refused"] as! [String: String]
        XCTAssertEqual(refused["undo"], "another app uses these keys")
        XCTAssertEqual(refused["redo"], "needs ⌘, ⌥ or ⌃")
        XCTAssertEqual((st["registered"] as! [String: String])["leftHalf"], "alt+cmd+left")
        XCTAssertNil((st["registered"] as! [String: String])["undo"])
    }

    /// Jared, 6:18 PM: snap worked but its view still said "needs accessibility access": the status went out once,
    /// before qrtz was switched on. A change of the grant now re-sends it, with the shortcuts it already holds.
    func testAGrantSwitchedOnLaterIsReportedWithoutARestart() {
        let b = FakeBackend()
        b.axTrusted = false
        var lines: [String] = []
        let s = SnapServer(backend: b, write: { lines.append($0) })
        XCTAssertFalse(s.refreshTrust(), "nothing before the first configure")
        s.configure(SnapConfig())
        XCTAssertEqual(obj(lines.last!)["axTrusted"] as? Bool, false)
        let n = lines.count
        XCTAssertFalse(s.refreshTrust(), "unchanged: nothing sent")
        XCTAssertEqual(lines.count, n)
        b.axTrusted = true
        XCTAssertTrue(s.refreshTrust())
        let st = obj(lines.last!)
        XCTAssertEqual(st["type"] as? String, "snap.status")
        XCTAssertEqual(st["axTrusted"] as? Bool, true)
        XCTAssertEqual((st["registered"] as! [String: String])["leftHalf"], "alt+cmd+left")
        XCTAssertFalse(s.refreshTrust())
        b.axTrusted = false
        XCTAssertTrue(s.refreshTrust(), "switched off again: reported too")
        XCTAssertEqual(obj(lines.last!)["axTrusted"] as? Bool, false)
    }

    func testActAndShutdown() {
        var lines: [String] = []
        let s = SnapServer(backend: FakeBackend(), write: { lines.append($0) })
        XCTAssertTrue(s.handle(#"{"type":"snap.act","action":"leftHalf"}"#))
        XCTAssertEqual(obj(lines.last!)["type"] as? String, "snap.moved")
        XCTAssertTrue(s.handle(#"{"type":"snap.act","action":"bogus"}"#))
        XCTAssertTrue(s.handle(#"{"type":"ctrl.snapshot"}"#))
        XCTAssertEqual(lines.count, 1)
        XCTAssertFalse(s.handle(#"{"type":"shutdown"}"#))
    }

    func testOffMacNothingRegisters() {
        var lines: [String] = []
        let s = SnapServer(backend: UnsupportedSnapBackend(), write: { lines.append($0) })
        s.configure(SnapConfig())
        let st = obj(lines.last!)
        XCTAssertEqual((st["registered"] as! [String: String]).count, 0)
        XCTAssertEqual((st["refused"] as! [String: String]).count, 18)
    }

    func testDisableAppLine() {
        let o = obj(SnapLines.disableApp("com.x", disabled: true))
        XCTAssertEqual(o["type"] as? String, "snap.disable-app")
        XCTAssertEqual(o["bundleId"] as? String, "com.x")
    }
}
