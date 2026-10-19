// ADR 0180: "turn off my display", helper side. The act and its app-less measure parse from the fixture and reach
// the backend with no app and no content; only the bare op parses; the Mac side writes exactly pmset's registry
// property and spawns nothing.
import Foundation
import XCTest
@testable import QrtzCtrl

private final class DisplayFake: CtrlBackend {
    var acted: [CtrlRoute] = []
    var measured: [(String, String)] = []
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], nil) }
    func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw CtrlFailure("app.not_running") }
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { [] }
    func act(_ route: CtrlRoute, content: [String: String]) throws { acted.append(route) }
    func measure(check: String, app: String) -> Int? {
        measured.append((check, app))
        return check == CtrlDisplay.check ? 1 : nil
    }
    func restore(frontmostPid: Int32?) -> Bool { false }
    func laya(state: String, question: String, options: [String]) throws -> [Double] { [] }
    func frontmostPid() -> Int32? { 100 }
    func pid(of app: String) -> Int32? { nil }
}

private func json(_ s: String) -> [String: Any] { try! JSONSerialization.jsonObject(with: Data(s.utf8)) as! [String: Any] }

private let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

private func fixtureLine(_ match: (String) -> Bool) throws -> String {
    let raw = try String(contentsOf: root.appendingPathComponent("Fixtures/ctrl/protocol.jsonl"), encoding: .utf8)
    return try XCTUnwrap(raw.split(separator: "\n").map(String.init).first(where: match))
}

final class CtrlDisplayTests: XCTestCase {
    func testSleepDisplayReachesTheBackendWithNoAppAndNoContent() throws {
        let line = try fixtureLine { $0.contains(#""op":"sleep_display""#) }
        guard case let .act(_, _, route, content)? = CtrlRequest.parse(line) else { return XCTFail("sleep_display") }
        XCTAssertEqual(route, .sleepDisplay)
        XCTAssertTrue(content.isEmpty)
        XCTAssertNil(CtrlServer.appOf(route))
        XCTAssertNoThrow(try CtrlServer.recheck(route, frontmostPid: 100, targetPid: nil))
        let fake = DisplayFake()
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.sleepDisplay])
    }

    func testItsMeasureHasNoAppAndReachesTheBackend() throws {
        let line = try fixtureLine { $0.contains(#""check":"display_asleep""#) }
        guard case let .measure(_, check, app)? = CtrlRequest.parse(line) else { return XCTFail("display_asleep") }
        XCTAssertEqual(check, CtrlDisplay.check)
        XCTAssertEqual(app, "")
        XCTAssertTrue(CtrlRoute.applessChecks.contains(CtrlDisplay.check))
        let fake = DisplayFake()
        let r = json(CtrlServer(backend: fake).handle(line: line))
        XCTAssertEqual(r["value"] as? Int, 1, "\(r)")
        XCTAssertEqual(fake.measured.map(\.0), [CtrlDisplay.check])
    }

    func testOnlyTheBareOpParses() {
        for bad in [#"{"route":"workspace","op":"sleep_displays"}"#, #"{"route":"files","op":"sleep_display"}"#,
                    #"{"route":"scripting","app":"com.apple.systemevents","verb":"sleep_display"}"#] {
            let line = #"{"type":"ctrl.act","id":"x","actionId":"a","route":"# + bad + #","content":{}}"#
            if case let .act(_, _, route, _)? = CtrlRequest.parse(line) {
                XCTAssertNotEqual(route, .sleepDisplay, bad)
            }
        }
    }

    /// The Mac side writes only pmset's property on the display wrangler and spawns nothing (no pmset, no shell).
    func testTheMacSideWritesOnlyTheWranglersIdleRequestAndSpawnsNothing() throws {
        let src = try String(contentsOf: root.appendingPathComponent("Sources/QrtzCtrl/mac/CtrlDisplayMac.swift"), encoding: .utf8)
        XCTAssertEqual(CtrlDisplay.wranglerPath, "IOService:/IOResources/IODisplayWrangler")
        XCTAssertEqual(CtrlDisplay.requestIdle, "IORequestIdle")
        XCTAssertEqual(src.components(separatedBy: "IORegistryEntrySetCFProperty(").count - 1, 1)
        XCTAssertTrue(src.contains("CtrlDisplay.wranglerPath") && src.contains("CtrlDisplay.requestIdle"))
        for banned in ["Process(", "posix_spawn", "pmset", "NSTask", "system(", "IOPMSleepSystem", "SACLockScreen", "/bin/"] {
            let code = src.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }.joined(separator: "\n")
            XCTAssertFalse(code.contains(banned), banned)
        }
    }
}
