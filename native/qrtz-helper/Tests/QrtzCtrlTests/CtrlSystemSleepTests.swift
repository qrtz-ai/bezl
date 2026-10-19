// ADR 0183: "put my mac to sleep", helper side. The act and its app-less measure parse from the fixture and reach the
// backend with no app and no content; only the bare op parses; the read after the act waits (bounded) for the last
// sleep time to move and never makes one up; the Mac side makes one power-manager sleep request, reads one sysctl and
// spawns nothing.
import Foundation
import XCTest
@testable import QrtzCtrl

private final class SleepFake: CtrlBackend {
    var acted: [CtrlRoute] = []
    var measured: [(String, String)] = []
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], nil) }
    func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw CtrlFailure("app.not_running") }
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { [] }
    func act(_ route: CtrlRoute, content: [String: String]) throws { acted.append(route) }
    func measure(check: String, app: String) -> Int? {
        measured.append((check, app))
        return check == CtrlSystemSleep.check ? 1_760_000_060 : nil
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

final class CtrlSystemSleepTests: XCTestCase {
    func testSleepSystemReachesTheBackendWithNoAppAndNoContent() throws {
        let line = try fixtureLine { $0.contains(#""op":"sleep_system""#) }
        guard case let .act(_, _, route, content)? = CtrlRequest.parse(line) else { return XCTFail("sleep_system") }
        XCTAssertEqual(route, .sleepSystem)
        XCTAssertTrue(content.isEmpty)
        XCTAssertNil(CtrlServer.appOf(route))
        XCTAssertNoThrow(try CtrlServer.recheck(route, frontmostPid: 100, targetPid: nil))
        let fake = SleepFake()
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.sleepSystem])
    }

    func testItsMeasureHasNoAppAndReachesTheBackend() throws {
        let line = try fixtureLine { $0.contains(#""check":"last_sleep""#) }
        guard case let .measure(_, check, app)? = CtrlRequest.parse(line) else { return XCTFail("last_sleep") }
        XCTAssertEqual(check, CtrlSystemSleep.check)
        XCTAssertEqual(app, "")
        XCTAssertTrue(CtrlRoute.applessChecks.contains(CtrlSystemSleep.check))
        let fake = SleepFake()
        let r = json(CtrlServer(backend: fake).handle(line: line))
        XCTAssertEqual(r["value"] as? Int, 1_760_000_060, "\(r)")
        XCTAssertEqual(fake.measured.map(\.0), [CtrlSystemSleep.check])
    }

    func testOnlyTheBareOpParses() {
        for bad in [#"{"route":"workspace","op":"sleep_systems"}"#, #"{"route":"files","op":"sleep_system"}"#,
                    #"{"route":"scripting","app":"com.apple.systemevents","verb":"sleep"}"#,
                    #"{"route":"scripting","app":"com.apple.systemevents","verb":"sleep_system"}"#] {
            let line = #"{"type":"ctrl.act","id":"x","actionId":"a","route":"# + bad + #","content":{}}"#
            if case let .act(_, _, route, _)? = CtrlRequest.parse(line) {
                XCTAssertNotEqual(route, .sleepSystem, bad)
            }
        }
        XCTAssertFalse(CtrlRoute.scriptVerbs.contains("sleep"), "System Events' sleep is not a ctrl verb")
    }

    func testTheReadAfterTheActWaitsForTheSleepAndNeverMakesOneUp() {
        // No act pending: one read, no wait.
        var waits: [Double] = []
        XCTAssertEqual(CtrlSystemSleep.awaitSleep(baseline: nil, read: { 5 }, wait: { waits.append($0) }), 5)
        XCTAssertTrue(waits.isEmpty)
        // The sleep shows on the third read.
        var reads = [100, 100, 160]
        waits = []
        XCTAssertEqual(CtrlSystemSleep.awaitSleep(baseline: 100, read: { reads.removeFirst() }, wait: { waits.append($0) }), 160)
        XCTAssertEqual(waits.count, 2)
        // It never comes: the unchanged value after the settle (the host's verify fails), within the helper timeout.
        waits = []
        XCTAssertEqual(CtrlSystemSleep.awaitSleep(baseline: 100, read: { 100 }, wait: { waits.append($0) }), 100)
        XCTAssertEqual(waits.reduce(0, +), CtrlSystemSleep.settleSeconds, accuracy: 0.001)
        XCTAssertLessThan(CtrlSystemSleep.settleSeconds, 10, "under the host's helper timeout")
        // Unreadable stays unreadable.
        XCTAssertNil(CtrlSystemSleep.awaitSleep(baseline: 100, settle: 1, read: { nil }, wait: { _ in }))
    }

    /// The Mac side makes one sleep request and reads one sysctl; it spawns nothing and touches nothing else.
    func testTheMacSideMakesOneSleepRequestAndSpawnsNothing() throws {
        let src = try String(contentsOf: root.appendingPathComponent("Sources/QrtzCtrl/mac/CtrlSystemSleepMac.swift"), encoding: .utf8)
        let code = src.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }.joined(separator: "\n")
        XCTAssertEqual(CtrlSystemSleep.sleepTimeName, "kern.sleeptime")
        XCTAssertEqual(code.components(separatedBy: "IOPMSleepSystem(").count - 1, 1)
        XCTAssertEqual(code.components(separatedBy: "sysctlbyname(").count - 1, 1)
        XCTAssertTrue(code.contains("CtrlSystemSleep.sleepTimeName"))
        for banned in ["Process(", "posix_spawn", "pmset", "NSTask", "system(", "NSAppleScript", "SACLockScreen", "/bin/",
                       "IORegistryEntrySetCFProperty", "IOPMAssertion", "terminate", "shutdown", "restart"] {
            XCTAssertFalse(code.contains(banned), banned)
        }
    }
}
