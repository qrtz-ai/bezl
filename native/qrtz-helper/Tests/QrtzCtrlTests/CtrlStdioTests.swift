// ctrl on the helper's stdio (ADR 0110, protocol 2 per ADR 0214). Which lines are ctrl's, the replies through
// `CtrlStdio`'s own queue in order, and the Linux backend answering at once (with no stop watcher, so the host
// never acts).
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
    func tree(app: String, scope: String?, depth: Int) throws -> (root: CtrlTreeNode, focused: String?) {
        (CtrlTreeNode(id: "e:1", role: "AXWindow"), nil)
    }
    func menuShortcuts(app: String) throws -> [CtrlMenuShortcut] { [] }
    func act(_ action: CtrlAction, sig: String?) throws {}
    func observe(arm: Bool?) -> CtrlObserved { CtrlObserved(armed: arm ?? false) }
    func mayAct() -> Bool { true }
    func laya(model: String?, state: String, question: String, options: [String]) throws -> [Double] { throw CtrlFailure("laya.no_model") }
}

final class CtrlStdioTests: XCTestCase {
    func testOnlyCtrlTypedLinesAreRoutedToCtrl() {
        XCTAssertTrue(CtrlStdio.isCtrlLine(#"{"type":"ctrl.catalog","id":"c0"}"#))
        XCTAssertFalse(CtrlStdio.isCtrlLine(#"{"type":"hello"}"#))
        XCTAssertFalse(CtrlStdio.isCtrlLine(#"{"type":"dictation.configure","dictionary":["\"ctrl.ax.act"]}"#))
        XCTAssertFalse(CtrlStdio.isCtrlLine("{nope \"ctrl."))
    }

    func testQrtzCtrlSaysHelloAndKnowsTheHostsGoodbye() {
        let hello = obj(CtrlStdio.helloLine(version: "0.1.0"))
        XCTAssertEqual(hello["type"] as? String, "hello")
        XCTAssertEqual(hello["features"] as? [String], ["module:ctrl"])
        XCTAssertFalse(CtrlStdio.isCtrlLine(CtrlStdio.helloLine(version: "0.1.0")))
        XCTAssertTrue(CtrlStdio.isShutdownLine(#"{"type":"shutdown"}"#))
        XCTAssertFalse(CtrlStdio.isShutdownLine(#"{"type":"ctrl.ax.act","id":"c1","action":{"text":"shutdown"}}"#))
    }

    func testRepliesComeBackInOrderOnCtrlsQueue() {
        let rec = Recorder()
        let io = CtrlStdio(backend: Echoing(), write: { rec.add($0) })
        io.submit(#"{"type":"ctrl.catalog","id":"c0"}"#)
        io.submit(#"{"type":"ctrl.tree","id":"c1","app":"com.apple.Safari","scope":null,"depth":6}"#)
        io.submit(#"{"type":"ctrl.laya","id":"c2","state":"s","question":"q","options":["a"]}"#)
        io.submit(#"{"type":"ctrl.ax.act","id":"c3","runId":"r","action":{"action":"key_combo","app":"x","combo":"escape"}}"#)
        io.submit(#"{"type":"ctrl.observe","id":"c4","runId":"r","arm":true}"#)
        io.drain()
        let r = rec.lines.map(obj)
        XCTAssertEqual(r.map { $0["id"] as? String }, ["c0", "c1", "c2", "c3", "c4"])
        XCTAssertEqual(r[0]["defaultBrowser"] as? String, "com.apple.Safari")
        XCTAssertEqual((r[1]["root"] as? [String: Any])?["role"] as? String, "AXWindow")
        XCTAssertEqual(r[2]["code"] as? String, "laya.no_model")
        XCTAssertEqual(r[3]["err"] as? String, "act.refused", "the stop key is never sent")
        XCTAssertEqual(r[4]["armed"] as? Bool, true)
    }

    func testOffMacEveryRequestAnswersAtOnceAndNothingIsArmed() {
        let rec = Recorder()
        let io = CtrlStdio(backend: UnsupportedCtrlBackend(), write: { rec.add($0) })
        io.submit(#"{"type":"ctrl.catalog","id":"c0"}"#)
        io.submit(#"{"type":"ctrl.tree","id":"c1","app":"com.apple.Notes","depth":6}"#)
        io.submit(#"{"type":"ctrl.menu.shortcuts","id":"c2","app":"com.apple.Notes"}"#)
        io.submit(#"{"type":"ctrl.observe","id":"c3","runId":"r","arm":true}"#)
        io.drain()
        let r = rec.lines.map(obj)
        XCTAssertEqual(r.prefix(3).map { $0["code"] as? String }, Array(repeating: "platform.unsupported", count: 3))
        XCTAssertEqual(r[3]["armed"] as? Bool, false)
        #if !os(macOS)
        XCTAssertTrue(makeCtrlBackend() is UnsupportedCtrlBackend)
        #endif
    }
}
