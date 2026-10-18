// The ctrl helper protocol against the shared fixture (ADR 0108): every host line parses, the server's replies have
// the fixture's shapes, and the policy re-check refuses what the ladder refuses before the backend is called.
import Foundation
import XCTest
@testable import QrtzCtrl

private let fixture = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    .appendingPathComponent("../../Fixtures/ctrl/protocol.jsonl").standardized

private func lines() throws -> [[String: Any]] {
    try String(contentsOf: fixture, encoding: .utf8).split(separator: "\n").map {
        try JSONSerialization.jsonObject(with: Data($0.utf8)) as! [String: Any]
    }
}

private func json(_ s: String) -> NSDictionary { try! JSONSerialization.jsonObject(with: Data(s.utf8)) as! NSDictionary }

private final class Fake: CtrlBackend {
    var acted: [CtrlRoute] = []
    var front: Int32? = 100
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) {
        ([CtrlCatalogApp(bundleId: "com.apple.Safari", name: "Safari", browser: true, running: true),
          CtrlCatalogApp(bundleId: "com.apple.TextEdit", name: "TextEdit", browser: false, running: false)], "com.apple.Safari")
    }
    func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw CtrlFailure("app.not_running") }
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] {
        [CtrlAXNode(id: "e:close", role: "AXButton", subrole: "AXCloseButton", title: "", actions: ["AXPress"], children: 0),
         CtrlAXNode(id: "e:tools", role: "AXToolbar", subrole: nil, title: "formatting", actions: [], children: 2)]
    }
    func act(_ route: CtrlRoute, content: [String: String]) throws { acted.append(route) }
    func measure(check: String, app: String) -> Int? { check == "tab_count" ? 0 : nil }
    func restore(frontmostPid: Int32?) -> Bool { frontmostPid != nil }
    func laya(state: String, question: String, options: [String]) throws -> [Double] { [0.75, 0.25] }
    func frontmostPid() -> Int32? { front }
    func pid(of app: String) -> Int32? { app == "com.apple.Notes" ? 100 : 200 }
    func preview(app: String, what: String) throws -> [CtrlPreviewItem] {
        [CtrlPreviewItem(title: "notes.txt", unsaved: false), CtrlPreviewItem(title: "draft.txt", unsaved: true)]
    }
}

final class CtrlProtocolTests: XCTestCase {
    func testEveryHostLineOfTheFixtureParses() throws {
        let all = try lines()
        let host = all.filter { !(($0["type"] as! String).hasSuffix(".result")) && $0["type"] as! String != "ctrl.error" }
        XCTAssertEqual(host.count, 9)
        for o in host {
            let line = String(decoding: try JSONSerialization.data(withJSONObject: o), as: UTF8.self)
            XCTAssertNotNil(CtrlRequest.parse(line), "\(o["type"]!)")
        }
    }

    func testRepliesHaveTheFixtureShapes() throws {
        let all = try lines()
        let server = CtrlServer(backend: Fake())
        let want = { (t: String) in NSDictionary(dictionary: all.first { $0["type"] as? String == t }!) }
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.catalog","id":"c0"}"#)), want("ctrl.catalog.result"))
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.snapshot","id":"c2","app":"com.apple.TextEdit","scope":null}"#)),
                       want("ctrl.snapshot.result"))
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.measure","id":"c5","check":{"check":"tab_count","app":"com.apple.Safari"}}"#)),
                       want("ctrl.measure.result"))
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.restore","id":"c6","frontmostPid":100}"#)), want("ctrl.restore.result"))
        let act = all.first { $0["type"] as? String == "ctrl.act" }!
        let actLine = String(decoding: try JSONSerialization.data(withJSONObject: act), as: UTF8.self)
        XCTAssertEqual(json(server.handle(line: actLine)), want("ctrl.act.result"))
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.probe","id":"c8"}"#)), want("ctrl.error"))
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.preview","id":"c9","app":"com.apple.TextEdit","what":"windows"}"#)),
                       want("ctrl.preview.result"))
    }

    func testABackendWithoutAPreviewSaysSoAndNeverActs() {
        // The host falls back to a count on any error; a preview is read-only either way.
        let server = CtrlServer(backend: UnsupportedCtrlBackend())
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.preview","id":"p","app":"com.apple.TextEdit","what":"windows"}"#)),
                       ["type": "ctrl.error", "id": "p", "code": "platform.unsupported"] as NSDictionary)
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.preview","id":"q","app":"x","what":"files"}"#))["code"] as? String,
                       "helper.bad_request", "only windows or tabs")
    }

    func testThePolicyIsRecheckedBeforeTheBackendActs() {
        let fake = Fake()
        let server = CtrlServer(backend: fake)
        func act(_ route: String) -> NSDictionary {
            json(server.handle(line: #"{"type":"ctrl.act","id":"x","actionId":"a0","route":\#(route),"content":{}}"#))
        }
        let refused: NSDictionary = ["type": "ctrl.error", "id": "x", "code": "policy.refused"]
        XCTAssertEqual(act(#"{"route":"pid_key","app":"com.apple.Safari","combo":"cmd+l"}"#), refused, "focus-intent shortcut")
        XCTAssertEqual(act(#"{"route":"pid_key","app":"com.apple.Notes","combo":"cmd+w"}"#), refused, "keys to the frontmost app")
        XCTAssertEqual(act(#"{"route":"ax","app":"com.apple.TextEdit","role":"AXPopUpButton","element":"e:m","op":{"op":"perform","action":"AXPress"}}"#),
                       refused, "opens a menu")
        XCTAssertEqual(act(#"{"route":"ax","app":"com.apple.TextEdit","role":"AXWindow","element":"e:w","op":{"op":"perform","action":"AXRaise"}}"#),
                       refused, "raise")
        XCTAssertEqual(act(#"{"route":"ax","app":"com.apple.TextEdit","role":"AXWindow","element":"e:w","op":{"op":"set_attribute","attribute":"AXMain"}}"#),
                       refused, "focus write")
        XCTAssertEqual(act(#"{"route":"activate","app":"com.apple.Safari"}"#), refused)
        XCTAssertEqual(act(#"{"route":"scripting","app":"com.apple.Safari","verb":"activate"}"#), refused, "not a template verb")
        XCTAssertTrue(fake.acted.isEmpty, "nothing refused reached the backend")
        XCTAssertEqual(act(#"{"route":"pid_key","app":"com.apple.Safari","combo":"cmd+w"}"#)["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.pidKey(app: "com.apple.Safari", combo: "cmd+w")])
    }

    func testAPinnedFlagRidesThePreviewOnlyWhenKnown() {
        let a = CtrlPreviewItem(title: "mail", unsaved: false, pinned: true)
        let b = CtrlPreviewItem(title: "doc", unsaved: nil)
        XCTAssertEqual(a.pinned, true)
        XCTAssertNil(b.pinned)
    }

    func testABadLineIsAnErrorNotACrash() {
        let server = CtrlServer(backend: Fake())
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.nope","id":"z"}"#)),
                       ["type": "ctrl.error", "id": "z", "code": "helper.bad_request"] as NSDictionary)
        XCTAssertEqual(json(server.handle(line: "not json"))["code"] as? String, "helper.bad_request")
    }
}
