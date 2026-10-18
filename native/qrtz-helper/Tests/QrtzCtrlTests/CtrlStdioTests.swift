// Slice 5b (ADR 0110): ctrl on the helper's stdio. Which lines are ctrl's, the replies through `CtrlStdio`'s own
// queue (fixture shapes, in order), the Linux backend answering at once, and the plain-data parts of the macOS
// backend (scripts that can't launch or activate, key combos for per-process events).
import Foundation
import XCTest
@testable import QrtzCtrl

private final class Recorder: @unchecked Sendable {
    private let lock = NSLock()
    private var all: [String] = []
    func add(_ s: String) { lock.lock(); all.append(s); lock.unlock() }
    var lines: [String] { lock.lock(); defer { lock.unlock() }; return all }
}

private func obj(_ s: String) -> [String: Any] { try! JSONSerialization.jsonObject(with: Data(s.utf8)) as! [String: Any] }

private final class Echoing: CtrlBackend {
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], "com.apple.Safari") }
    func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw CtrlFailure("app.not_running") }
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { [] }
    func act(_ route: CtrlRoute, content: [String: String]) throws {}
    func measure(check: String, app: String) -> Int? { 3 }
    func restore(frontmostPid: Int32?) -> Bool { true }
    func laya(state: String, question: String, options: [String]) throws -> [Double] { throw CtrlFailure("laya.no_model") }
    func frontmostPid() -> Int32? { 100 }
    func pid(of app: String) -> Int32? { 200 }
}

final class CtrlStdioTests: XCTestCase {
    func testOnlyCtrlTypedLinesAreRoutedToCtrl() {
        XCTAssertTrue(CtrlStdio.isCtrlLine(#"{"type":"ctrl.catalog","id":"c0"}"#))
        XCTAssertFalse(CtrlStdio.isCtrlLine(#"{"type":"hello"}"#))
        XCTAssertFalse(CtrlStdio.isCtrlLine(#"{"type":"dictation.configure","dictionary":["\"ctrl.act"]}"#))
        XCTAssertFalse(CtrlStdio.isCtrlLine("{nope \"ctrl."))
    }

    func testRepliesComeBackInOrderOnCtrlsQueue() {
        let rec = Recorder()
        let io = CtrlStdio(backend: Echoing(), write: { rec.add($0) })
        io.submit(#"{"type":"ctrl.catalog","id":"c0"}"#)
        io.submit(#"{"type":"ctrl.measure","id":"c1","check":{"check":"tab_count","app":"com.apple.Safari"}}"#)
        io.submit(#"{"type":"ctrl.laya","id":"c2","state":"s","question":"q","options":["a"]}"#)
        io.submit(#"{"type":"ctrl.act","id":"c3","actionId":"a0","route":{"route":"click"},"content":{}}"#)
        io.drain()
        let r = rec.lines.map(obj)
        XCTAssertEqual(r.map { $0["id"] as? String }, ["c0", "c1", "c2", "c3"])
        XCTAssertEqual(r[0]["defaultBrowser"] as? String, "com.apple.Safari")
        XCTAssertEqual(r[1]["value"] as? Int, 3)
        XCTAssertEqual(r[2]["code"] as? String, "laya.no_model")
        XCTAssertEqual(r[3]["code"] as? String, "policy.refused", "a takeover route is refused before the backend")
    }

    func testOffMacEveryRequestAnswersAtOnce() {
        let rec = Recorder()
        let io = CtrlStdio(backend: UnsupportedCtrlBackend(), write: { rec.add($0) })
        for (i, t) in ["ctrl.catalog", "ctrl.probe"].enumerated() { io.submit(#"{"type":"\#(t)","id":"c\#(i)"}"#) }
        io.submit(#"{"type":"ctrl.snapshot","id":"c2","app":"com.apple.TextEdit","scope":null}"#)
        io.drain()
        XCTAssertEqual(rec.lines.map { obj($0)["code"] as? String }, Array(repeating: "platform.unsupported", count: 3))
        #if !os(macOS)
        XCTAssertTrue(makeCtrlBackend() is UnsupportedCtrlBackend)
        #endif
    }

    func testScriptsNeverLaunchActivateOrTakeArbitraryIds() {
        for v in CtrlRoute.scriptVerbs {
            let s = CtrlScripts.source(verb: v, bundleId: "com.apple.Safari")!
            XCTAssertTrue(s.hasPrefix("tell application id \"com.apple.Safari\"\n"))
            for word in ["activate", "launch", "open", "reopen", "saving no", "do shell script", "keystroke"] {
                XCTAssertFalse(s.contains(word), "\(v): \(word)")
            }
        }
        XCTAssertNil(CtrlScripts.source(verb: "quit", bundleId: "com.apple.Safari"))
        XCTAssertNil(CtrlScripts.source(verb: "close_tab", bundleId: "com.apple.Safari\" to activate --"))
        XCTAssertNil(CtrlScripts.source(verb: "close_tab", bundleId: ""))
        XCTAssertTrue(CtrlScripts.safeBundleId("org.mozilla.firefox"))
    }

    func testKeyCombosMapToVirtualKeysAndModifiers() {
        XCTAssertEqual(CtrlKeyCombo.parse("cmd+w"), CtrlKeyCombo(keyCode: 0x0D, command: true, shift: false, option: false, control: false))
        XCTAssertEqual(CtrlKeyCombo.parse("cmd+option+w")?.option, true)
        XCTAssertEqual(CtrlKeyCombo.parse("⌘⇧T"), CtrlKeyCombo(keyCode: 0x11, command: true, shift: true, option: false, control: false))
        XCTAssertEqual(CtrlKeyCombo.parse("escape")?.keyCode, 0x35)
        XCTAssertNil(CtrlKeyCombo.parse("cmd+f13"))
        XCTAssertNil(CtrlKeyCombo.parse(""))
    }
}
