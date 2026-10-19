// The ctrl helper protocol against the shared fixture (ADR 0214): every host line parses, the server's replies have
// the fixture's shapes, and the helper's own policy re-check refuses what the host would refuse before the backend
// is called (no stop key, no non-http address, no free-form combo, no handle it didn't make, no oversized text).
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

private func obj(_ s: String) -> [String: Any] { try! JSONSerialization.jsonObject(with: Data(s.utf8)) as! [String: Any] }
private func line(_ o: [String: Any]) -> String {
    String(decoding: try! JSONSerialization.data(withJSONObject: o, options: [.sortedKeys]), as: UTF8.self)
}

private final class Fake: CtrlBackend {
    var acted: [CtrlAction] = []
    var sigs: [String?] = []
    var layaModels: [String?] = []
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) {
        ([CtrlCatalogApp(bundleId: "com.apple.Safari", name: "Safari", browser: true, running: true),
          CtrlCatalogApp(bundleId: "com.apple.Notes", name: "Notes", browser: false, running: false)], "com.apple.Safari")
    }
    func tree(app: String, scope: String?, depth: Int) throws -> (root: CtrlTreeNode, focused: String?) {
        guard app == "com.apple.Safari" else { throw CtrlFailure("app.not_running") }
        let field = CtrlTreeNode(id: "e:2", role: "AXTextField", placeholder: "search or enter website name", value: "",
                                 actions: ["AXPress"], focused: true, sig: "8c2b1f0a6d3e4b71")
        let favs = CtrlTreeNode(id: "e:3", role: "AXGroup", title: "favorites", more: 4, sig: "11aa22bb33cc44dd")
        return (CtrlTreeNode(id: "e:1", role: "AXWindow", title: "start page", children: [field, favs], sig: "0f1e2d3c4b5a6978"), "e:2")
    }
    func menuShortcuts(app: String) throws -> [CtrlMenuShortcut] {
        [CtrlMenuShortcut(id: "m:1", path: ["file", "new tab"], combo: "cmd+t"),
         CtrlMenuShortcut(id: "m:2", path: ["file", "open location…"], combo: "cmd+l")]
    }
    func act(_ action: CtrlAction, sig: String?) throws {
        if sig == "stale" { throw CtrlFailure("act.stale") }
        acted.append(action); sigs.append(sig)
    }
    func observe(arm: Bool?) -> CtrlObserved { CtrlObserved(frontmost: "com.apple.Safari", armed: true) }
    let watcher: CtrlStopWatch = { let w = CtrlStopWatch(); w.arm(started: true); return w }()
    func mayAct() -> Bool { watcher.mayAct }
    func laya(model: String?, state: String, question: String, options: [String]) throws -> [Double] {
        layaModels.append(model)
        return [0.75, 0.25]
    }
    func warmLaya(model: String) throws -> CtrlLayaWarmer.State { .started }
}

final class CtrlProtocolTests: XCTestCase {
    /// Review C2: after Esc the helper refuses every act with `guard.stopped` and never reaches the backend.
    func testAStoppedWatcherRefusesEveryActWithGuardStopped() throws {
        let f = Fake()
        let s = CtrlServer(backend: f)
        let act = try XCTUnwrap(lines().first { ($0["type"] as? String) == "ctrl.ax.act" })
        XCTAssertEqual(obj(s.handle(line: line(act)))["ok"] as? Bool, true)
        f.watcher.record(.keyDown(code: CtrlKeys.escape, marker: 0))
        let r = obj(s.handle(line: line(act)))
        XCTAssertEqual(r["ok"] as? Bool, false)
        XCTAssertEqual(r["err"] as? String, "guard.stopped")
        XCTAssertEqual(f.acted.count, 1)
        let u = Fake(); u.watcher.disarm()  // nothing watches: refuses too
        XCTAssertEqual(obj(CtrlServer(backend: u).handle(line: line(act)))["err"] as? String, "guard.stopped")
        XCTAssertTrue(u.acted.isEmpty)
    }

    func testEveryHostLineOfTheFixtureParses() throws {
        let host = try lines().filter { o in
            let t = o["type"] as! String
            return !t.hasSuffix(".result") && t != "ctrl.error"
        }
        XCTAssertGreaterThanOrEqual(host.count, 10)
        for o in host { XCTAssertNotNil(CtrlRequest.parse(line(o)), "\(o)") }
    }

    func testRepliesHaveTheFixturesShapes() throws {
        let all = try lines()
        let server = CtrlServer(backend: Fake())
        for want in all where (want["type"] as! String).hasSuffix(".result") {
            let id = want["id"] as! String
            guard let req = all.first(where: { $0["id"] as? String == id && !($0["type"] as! String).hasSuffix(".result") }) else { continue }
            let got = obj(server.handle(line: line(req)))
            XCTAssertEqual(got["type"] as? String, want["type"] as? String, id)
            XCTAssertEqual(Set(got.keys).subtracting(["focused"]), Set(want.keys).subtracting(["focused"]), id)
            if id == "c1" {
                XCTAssertEqual(NSDictionary(dictionary: got["root"] as! [String: Any]), NSDictionary(dictionary: want["root"] as! [String: Any]))
                XCTAssertEqual(got["focused"] as? String, "e:2")
            }
        }
    }

    func testTheFixedActionSetParsesStrictly() {
        let ok: [(String, CtrlAction)] = [
            (#"{"action":"press","handle":"e:3"}"#, .press(handle: "e:3")),
            (#"{"action":"set_value","handle":"e:2","text":"hi","submit":false}"#, .setValue(handle: "e:2", text: "hi", submit: false)),
            (#"{"action":"scroll","handle":"e:3","dir":"up"}"#, .scroll(handle: "e:3", down: false)),
            (#"{"action":"key_combo","app":"a","combo":"cmd+t"}"#, .keyCombo(app: "a", combo: "cmd+t")),
            (#"{"action":"open_app","app":"a"}"#, .openApp(app: "a")),
        ]
        for (s, a) in ok { XCTAssertEqual(CtrlAction.parse(obj(s)), a, s) }
        for s in [#"{"action":"click","x":1,"y":2}"#, #"{"action":"applescript","source":"tell"}"#,
                  #"{"action":"set_value","handle":"e:2","text":"hi","submit":1}"#, #"{"action":"scroll","handle":"e:3","dir":"left"}"#,
                  #"{"action":"type","text":"hi"}"#] {
            XCTAssertNil(CtrlAction.parse(obj(s)), s)
        }
    }

    func testThePolicyRefusesBeforeTheBackend() {
        let fake = Fake()
        let server = CtrlServer(backend: fake)
        func act(_ a: String, sig: String? = nil) -> [String: Any] {
            var o: [String: Any] = ["type": "ctrl.ax.act", "id": "x", "runId": "r", "action": obj(a)]
            if let sig { o["sig"] = sig }
            return obj(server.handle(line: line(o)))
        }
        for bad in [#"{"action":"key_combo","app":"a","combo":"escape"}"#,
                    #"{"action":"key_combo","app":"a","combo":"cmd+tab"}"#,
                    #"{"action":"key_combo","app":"a","combo":"shift+cmd+t"}"#,
                    #"{"action":"key_combo","app":"a","combo":"t"}"#,
                    #"{"action":"open_url","app":"a","url":"file:///etc/passwd"}"#,
                    #"{"action":"open_url","app":"a","url":"javascript:alert(1)"}"#,
                    #"{"action":"press","handle":"AXButton"}"#,
                    #"{"action":"set_value","handle":"e:2","text":"","submit":false}"#,
                    #"{"action":"set_value","handle":"e:2","text":"\#(String(repeating: "x", count: 501))","submit":false}"#] {
            let r = act(bad)
            XCTAssertEqual(r["ok"] as? Bool, false, bad)
            XCTAssertEqual(r["err"] as? String, "act.refused", bad)
        }
        XCTAssertTrue(fake.acted.isEmpty)
        XCTAssertEqual(act(#"{"action":"key_combo","app":"a","combo":"cmd+shift+t"}"#)["ok"] as? Bool, true)
        XCTAssertEqual(act(#"{"action":"key_combo","app":"a","combo":"return"}"#)["ok"] as? Bool, true)
        XCTAssertEqual(act(#"{"action":"open_url","app":"a","url":"https://youtube.com"}"#)["ok"] as? Bool, true)
        XCTAssertEqual(act(#"{"action":"press","handle":"m:2"}"#, sig: "abc")["ok"] as? Bool, true)
        XCTAssertEqual(fake.sigs.last ?? nil, "abc")
        XCTAssertEqual(act(#"{"action":"press","handle":"e:2"}"#, sig: "stale")["err"] as? String, "act.stale")
    }

    func testBadRequestsAndTheLayaModelPath() {
        let fake = Fake()
        let server = CtrlServer(backend: fake)
        XCTAssertEqual(obj(server.handle(line: #"{"type":"ctrl.snapshot","id":"old","app":"a"}"#))["code"] as? String, "helper.bad_request")
        XCTAssertEqual(obj(server.handle(line: #"{"type":"ctrl.act","id":"old","route":{"route":"scripting"}}"#))["code"] as? String, "helper.bad_request")
        XCTAssertEqual(obj(server.handle(line: "not json"))["code"] as? String, "helper.bad_request")
        XCTAssertEqual(obj(server.handle(line: #"{"type":"ctrl.tree","id":"t","app":"com.apple.Notes"}"#))["code"] as? String, "app.not_running")
        _ = server.handle(line: #"{"type":"ctrl.laya","id":"l","model":"/m.mlmodelc","state":"s","question":"q","options":["a","b"]}"#)
        _ = server.handle(line: #"{"type":"ctrl.laya","id":"l","state":"s","question":"q","options":["a","b"]}"#)
        XCTAssertEqual(fake.layaModels, ["/m.mlmodelc", nil])
    }
}
