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
    var layaModels: [String?] = []
    func laya(state: String, question: String, options: [String]) throws -> [Double] { [0.75, 0.25] }
    func laya(model: String?, state: String, question: String, options: [String]) throws -> [Double] {
        layaModels.append(model)
        return try laya(state: state, question: question, options: options)
    }
    func frontmostPid() -> Int32? { front }
    func pid(of app: String) -> Int32? { app == "com.apple.Notes" ? 100 : 200 }
    func preview(app: String, what: String) throws -> [CtrlPreviewItem] {
        [CtrlPreviewItem(title: "notes.txt", unsaved: false), CtrlPreviewItem(title: "draft.txt", unsaved: true)]
    }
    func runningApps() throws -> (apps: [CtrlRunningApp], sinceMs: UInt64?, dayStartMs: UInt64) {
        ([CtrlRunningApp(bundleId: "com.tinyspeck.slackmacgap", name: "Slack", lastActiveMs: 1_789_990_000_000, frontmost: false, unsaved: nil, hidden: false),
          CtrlRunningApp(bundleId: "com.apple.Notes", name: "Notes", lastActiveMs: nil, frontmost: true, unsaved: false)],
         1_789_950_000_000, 1_789_989_200_000)
    }
    var warms: [String] = []
    func warmLaya(model: String) throws -> CtrlLayaWarmer.State { warms.append(model); return .started }
}

final class CtrlProtocolTests: XCTestCase {
    func testEveryHostLineOfTheFixtureParses() throws {
        let all = try lines()
        let host = all.filter { !(($0["type"] as! String).hasSuffix(".result")) && $0["type"] as! String != "ctrl.error" }
        XCTAssertEqual(host.count, 25)
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
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.laya.warm","id":"c12","model":"/models/ctrl/laya/laya.mlmodelc"}"#)),
                       want("ctrl.laya.warm.result"))
    }

    func testTheConsentedModelPathReachesTheBackend() throws {
        // ADR 0116: the host hands the consented model over with the question; a line without one still parses.
        let m = try lines().first { $0["type"] as? String == "ctrl.laya" && $0["model"] != nil }!
        let line = String(decoding: try JSONSerialization.data(withJSONObject: m), as: UTF8.self)
        guard case let .laya(_, model, _, _, opts)? = CtrlRequest.parse(line) else { return XCTFail("laya") }
        XCTAssertEqual(model, "/models/ctrl/laya/laya.mlmodelc")
        XCTAssertEqual(opts.count, 2)
        guard case let .laya(_, none, _, _, _)? = CtrlRequest.parse(#"{"type":"ctrl.laya","id":"n","state":"s","question":"q","options":["a"]}"#)
        else { return XCTFail("laya without a model") }
        XCTAssertNil(none)
        let fake = Fake()
        _ = CtrlServer(backend: fake).handle(line: line)
        XCTAssertEqual(fake.layaModels, ["/models/ctrl/laya/laya.mlmodelc"])
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

    func testQuitUnusedAppsWireAndTheFrontmostAppIsNeverQuit() throws {
        // ADR 0130: the apps report has the fixture's shape; quit_apps parses; a list naming the frontmost app is
        // refused before the backend sees it; a backend without the watch says so.
        let all = try lines()
        let want = NSDictionary(dictionary: all.first { $0["type"] as? String == "ctrl.apps.result" }!)
        let fake = Fake()
        let server = CtrlServer(backend: fake)
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.apps","id":"c10"}"#)), want)
        let act = all.first { $0["type"] as? String == "ctrl.act" && ($0["route"] as? [String: Any])?["op"] as? String == "quit_apps" }!
        let line = String(decoding: try JSONSerialization.data(withJSONObject: act), as: UTF8.self)
        guard case let .act(_, _, route, _)? = CtrlRequest.parse(line) else { return XCTFail("quit_apps") }
        XCTAssertEqual(route, .quitApps(apps: ["com.tinyspeck.slackmacgap"]))
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.act","id":"x","actionId":"a0","route":{"route":"workspace","op":"quit_apps","apps":["com.apple.Safari","com.apple.Notes"]},"content":{}}"#)),
                       ["type": "ctrl.error", "id": "x", "code": "policy.refused"] as NSDictionary)
        XCTAssertTrue(fake.acted.isEmpty)
        XCTAssertEqual(json(server.handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.quitApps(apps: ["com.tinyspeck.slackmacgap"])])
        XCTAssertEqual(json(CtrlServer(backend: UnsupportedCtrlBackend()).handle(line: #"{"type":"ctrl.apps","id":"u"}"#))["code"] as? String,
                       "apps.not_watching")
    }

    func testHideAppsWireAndTheFrontmostAppIsNeverHidden() throws {
        // ADR 0134: hide_apps parses from the fixture; the frontmost app (Notes, pid 100) drops out of the list before
        // the backend sees it and the rest are hidden; `hidden` rides the apps report only when the backend knows it.
        let all = try lines()
        let act = all.first { $0["type"] as? String == "ctrl.act" && ($0["route"] as? [String: Any])?["op"] as? String == "hide_apps" }!
        let line = String(decoding: try JSONSerialization.data(withJSONObject: act), as: UTF8.self)
        guard case let .act(_, _, route, _)? = CtrlRequest.parse(line) else { return XCTFail("hide_apps") }
        XCTAssertEqual(route, .hideApps(apps: ["com.tinyspeck.slackmacgap", "com.apple.finder"]))
        let fake = Fake()
        let server = CtrlServer(backend: fake)
        XCTAssertEqual(json(server.handle(line: #"{"type":"ctrl.act","id":"x","actionId":"a0","route":{"route":"workspace","op":"hide_apps","apps":["com.apple.Safari","com.apple.Notes"]},"content":{}}"#))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.hideApps(apps: ["com.apple.Safari"])])
        let apps = json(server.handle(line: #"{"type":"ctrl.apps","id":"h"}"#))["apps"] as! [[String: Any]]
        XCTAssertEqual(apps[0]["hidden"] as? Bool, false)
        XCTAssertNil(apps[1]["hidden"], "unknown stays off the wire")
    }

    func testUnhideAppsWireReachesTheBackendWithNoFrontmostFilter() throws {
        // ADR 0152: unhide_apps parses and reaches the backend as sent (unhiding activates nothing, so the app the
        // user is in needs no filtering); a malformed list is a bad request.
        let line = #"{"type":"ctrl.act","id":"u","actionId":"a0","route":{"route":"workspace","op":"unhide_apps","apps":["com.apple.TextEdit"]},"content":{}}"#
        guard case let .act(_, _, route, _)? = CtrlRequest.parse(line) else { return XCTFail("unhide_apps") }
        XCTAssertEqual(route, .unhideApps(apps: ["com.apple.TextEdit"]))
        let fake = Fake()
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.unhideApps(apps: ["com.apple.TextEdit"])])
        XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.act","id":"u","actionId":"a0","route":{"route":"workspace","op":"unhide_apps","apps":"x"},"content":{}}"#))
    }

    func testMuteOutputWireAndItsMeasureHasNoApp() throws {
        // ADR 0135: mute_output and the app-less output_muted measure parse from the fixture and reach the backend.
        let all = try lines()
        let act = all.first { $0["type"] as? String == "ctrl.act" && ($0["route"] as? [String: Any])?["op"] as? String == "mute_output" }!
        let line = String(decoding: try JSONSerialization.data(withJSONObject: act), as: UTF8.self)
        guard case let .act(_, _, route, _)? = CtrlRequest.parse(line) else { return XCTFail("mute_output") }
        XCTAssertEqual(route, .muteOutput)
        let fake = Fake()
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.muteOutput])
        // ADR 0139: unmute_output, the reverse.
        let un = all.first { ($0["route"] as? [String: Any])?["op"] as? String == "unmute_output" }!
        let unLine = String(decoding: try JSONSerialization.data(withJSONObject: un), as: UTF8.self)
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: unLine))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.muteOutput, .unmuteOutput])
        // ADR 0140: set_volume with a whole level 0–100 from the fixture; anything else is a bad request.
        let sv = all.first { ($0["route"] as? [String: Any])?["op"] as? String == "set_volume" }!
        let svLine = String(decoding: try JSONSerialization.data(withJSONObject: sv), as: UTF8.self)
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: svLine))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.muteOutput, .unmuteOutput, .setVolume(level: 30)])
        // ADR 0141: 0 and 1 are levels (a JSON 0 or 1 also bridges `is Bool`), true and false are not.
        for (level, want) in [("0", 0), ("1", 1), ("100", 100)] {
            guard case let .act(_, _, route, _)? = CtrlRequest.parse(#"{"type":"ctrl.act","id":"x","actionId":"a","route":{"route":"workspace","op":"set_volume","level":"# + level + #"},"content":{}}"#)
            else { return XCTFail("level \(level)") }
            XCTAssertEqual(route, .setVolume(level: want))
        }
        for bad in [#"101"#, #"-1"#, #""30""#, #"30.5"#, #"null"#, #"true"#, #"false"#] {
            XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.act","id":"x","actionId":"a","route":{"route":"workspace","op":"set_volume","level":"# + bad + #"},"content":{}}"#), bad)
        }
        guard case let .measure(_, vcheck, vapp)? = CtrlRequest.parse(#"{"type":"ctrl.measure","id":"c20","check":{"check":"output_volume"}}"#)
        else { return XCTFail("output_volume") }
        XCTAssertEqual(vcheck, "output_volume")
        XCTAssertEqual(vapp, "")
        guard case let .measure(_, check, app)? = CtrlRequest.parse(#"{"type":"ctrl.measure","id":"c15","check":{"check":"output_muted"}}"#)
        else { return XCTFail("output_muted") }
        XCTAssertEqual(check, "output_muted")
        XCTAssertEqual(app, "")
        XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.measure","id":"x","check":{"check":"tab_count"}}"#), "other checks still need an app")
    }

    func testDarkModeGoesOnlyToSystemEventsAndItsMeasureHasNoApp() throws {
        // ADR 0137: set_dark_mode parses from the fixture and reaches the backend; its script goes to System Events
        // only, never another app, and no other verb goes to System Events; dark_mode is measured without an app.
        let all = try lines()
        let act = all.first { $0["type"] as? String == "ctrl.act" && ($0["route"] as? [String: Any])?["verb"] as? String == "set_dark_mode" }!
        let line = String(decoding: try JSONSerialization.data(withJSONObject: act), as: UTF8.self)
        guard case let .act(_, _, route, _)? = CtrlRequest.parse(line) else { return XCTFail("set_dark_mode") }
        XCTAssertEqual(route, .scripting(app: "com.apple.systemevents", verb: "set_dark_mode"))
        let fake = Fake()
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [route])
        let dark = CtrlScripts.source(verb: "set_dark_mode", bundleId: "com.apple.systemevents")!
        XCTAssertTrue(dark.contains("tell application id \"com.apple.systemevents\""))
        XCTAssertTrue(dark.contains("set dark mode to true"))
        XCTAssertFalse(dark.contains("activate") || dark.contains("launch") || dark.contains("reopen"))
        XCTAssertTrue(CtrlScripts.source(verb: "set_light_mode", bundleId: "com.apple.systemevents")!.contains("set dark mode to false"))
        XCTAssertNil(CtrlScripts.source(verb: "set_dark_mode", bundleId: "com.apple.Safari"))
        XCTAssertNil(CtrlScripts.source(verb: "close_all_tabs", bundleId: "com.apple.systemevents"))
        guard case let .measure(_, check, app)? = CtrlRequest.parse(#"{"type":"ctrl.measure","id":"c17","check":{"check":"dark_mode"}}"#)
        else { return XCTFail("dark_mode") }
        XCTAssertEqual(check, "dark_mode")
        XCTAssertEqual(app, "")
        XCTAssertTrue(CtrlAppearance.isDark(["AppleInterfaceStyle": "Dark"]))
        XCTAssertFalse(CtrlAppearance.isDark([:]))
        XCTAssertFalse(CtrlAppearance.isDark(nil))
    }

    func testTheLastUsedRecordKeepsTheLatestActivationAndMidnightIsLocal() {
        let u = CtrlLastUsed(sinceMs: 1_000)
        XCTAssertNil(u.lastActive("com.a"))
        u.record("com.a", atMs: 5_000)
        u.record("com.a", atMs: 3_000)
        XCTAssertEqual(u.lastActive("com.a"), 5_000, "an older notification never moves it back")
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "America/Los_Angeles")!
        // 2026-10-18 09:13 PDT → 2026-10-18 00:00 PDT = 07:00 UTC.
        XCTAssertEqual(CtrlLastUsed.dayStartMs(nowMs: 1_792_339_980_000, calendar: cal), 1_792_306_800_000)
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
