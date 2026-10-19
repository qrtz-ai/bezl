// ADR 0194: "keep my mac awake for 2 hours", helper side. The acts and the app-less measure parse from the fixture and
// reach the backend with no app and no content; `minutes` must be a whole JSON number 1–720 and `display` a JSON bool,
// nothing else is read; the measure reads only the assertion's level; the Mac side makes one assertion with a
// timeout that releases it, lets go only its own, reads no other app's, declares no user activity and spawns nothing.
import Foundation
import XCTest
@testable import QrtzCtrl

private final class AwakeFake: CtrlBackend {
    var acted: [CtrlRoute] = []
    var measured: [(String, String)] = []
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], nil) }
    func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw CtrlFailure("app.not_running") }
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { [] }
    func act(_ route: CtrlRoute, content: [String: String]) throws { acted.append(route) }
    func measure(check: String, app: String) -> Int? {
        measured.append((check, app))
        return check == CtrlKeepAwake.check ? 1 : nil
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

final class CtrlKeepAwakeTests: XCTestCase {
    func testKeepAwakeAndAllowSleepReachTheBackendWithNoAppAndNoContent() throws {
        for (op, want) in [("keep_awake", CtrlRoute.keepAwake(minutes: 120, display: false)), ("allow_sleep", .allowSleep)] {
            let line = try fixtureLine { $0.contains(#""op":"\#(op)""#) }
            guard case let .act(_, _, route, content)? = CtrlRequest.parse(line) else { return XCTFail(op) }
            XCTAssertEqual(route, want)
            XCTAssertTrue(content.isEmpty)
            XCTAssertNil(CtrlServer.appOf(route))
            XCTAssertNoThrow(try CtrlServer.recheck(route, frontmostPid: 100, targetPid: nil))
            let fake = AwakeFake()
            XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
            XCTAssertEqual(fake.acted, [want])
        }
    }

    func testItsMeasureHasNoAppAndReachesTheBackend() throws {
        let line = try fixtureLine { $0.contains(#""check":"kept_awake""#) }
        guard case let .measure(_, check, app)? = CtrlRequest.parse(line) else { return XCTFail("kept_awake") }
        XCTAssertEqual(check, CtrlKeepAwake.check)
        XCTAssertEqual(app, "")
        XCTAssertTrue(CtrlRoute.applessChecks.contains(CtrlKeepAwake.check))
        let fake = AwakeFake()
        let r = json(CtrlServer(backend: fake).handle(line: line))
        XCTAssertEqual(r["value"] as? Int, 1, "\(r)")
        XCTAssertEqual(fake.measured.map(\.0), [CtrlKeepAwake.check])
    }

    func testMinutesAreAWholeNumberInRangeAndDisplayAStrictBool() {
        func parse(_ route: String) -> CtrlRoute? {
            let line = #"{"type":"ctrl.act","id":"x","actionId":"a","route":"# + route + #","content":{}}"#
            guard case let .act(_, _, r, _)? = CtrlRequest.parse(line) else { return nil }
            return r
        }
        XCTAssertEqual(parse(#"{"route":"workspace","op":"keep_awake","minutes":1,"display":false}"#),
                       .keepAwake(minutes: 1, display: false))
        XCTAssertEqual(parse(#"{"route":"workspace","op":"keep_awake","minutes":720,"display":true}"#),
                       .keepAwake(minutes: 720, display: true))
        // A reason or an app riding along is never read.
        XCTAssertEqual(parse(#"{"route":"workspace","op":"keep_awake","minutes":60,"display":false,"name":"x","app":"y"}"#),
                       .keepAwake(minutes: 60, display: false))
        XCTAssertEqual(parse(#"{"route":"workspace","op":"allow_sleep"}"#), .allowSleep)
        for bad in [#"{"route":"workspace","op":"keep_awake","minutes":0,"display":false}"#,
                    #"{"route":"workspace","op":"keep_awake","minutes":721,"display":false}"#,
                    #"{"route":"workspace","op":"keep_awake","minutes":-5,"display":false}"#,
                    #"{"route":"workspace","op":"keep_awake","minutes":60.5,"display":false}"#,
                    #"{"route":"workspace","op":"keep_awake","minutes":true,"display":false}"#,
                    #"{"route":"workspace","op":"keep_awake","minutes":"60","display":false}"#,
                    #"{"route":"workspace","op":"keep_awake","minutes":60,"display":0}"#,
                    #"{"route":"workspace","op":"keep_awake","minutes":60,"display":"true"}"#,
                    #"{"route":"workspace","op":"keep_awake","minutes":60}"#,
                    #"{"route":"workspace","op":"keep_awake","display":false}"#,
                    #"{"route":"files","op":"keep_awake","minutes":60,"display":false}"#] {
            if let r = parse(bad) {
                if case .keepAwake = r { XCTFail(bad) }
            }
        }
    }

    func testTheTimeoutTypeAndMeasureNeverMakeUpAValue() {
        XCTAssertEqual(CtrlKeepAwake.seconds(minutes: 1), 60)
        XCTAssertEqual(CtrlKeepAwake.seconds(minutes: 720), 43_200)
        XCTAssertNil(CtrlKeepAwake.seconds(minutes: 0))
        XCTAssertNil(CtrlKeepAwake.seconds(minutes: 721))
        XCTAssertEqual(CtrlKeepAwake.type(display: false), "PreventUserIdleSystemSleep")
        XCTAssertEqual(CtrlKeepAwake.type(display: true), "PreventUserIdleDisplaySleep")
        XCTAssertEqual(CtrlKeepAwake.value(properties: nil), 0)
        XCTAssertEqual(CtrlKeepAwake.value(properties: [:]), 0)
        XCTAssertEqual(CtrlKeepAwake.value(properties: ["AssertLevel": NSNumber(value: 255)]), 1)
        XCTAssertEqual(CtrlKeepAwake.value(properties: ["AssertLevel": NSNumber(value: 0)]), 0)
        XCTAssertEqual(CtrlKeepAwake.name, CtrlKeepAwake.name.lowercased())
    }

    /// The Mac side makes one assertion and sets its release-on-timeout before anything else can happen, releases only
    /// the id it made, never lists other processes' assertions or declares user activity, and spawns nothing.
    func testTheMacSideHoldsOneTimedAssertionAndSpawnsNothing() throws {
        let src = try String(contentsOf: root.appendingPathComponent("Sources/QrtzCtrl/mac/CtrlKeepAwakeMac.swift"), encoding: .utf8)
        let code = src.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }.joined(separator: "\n")
        XCTAssertEqual(code.components(separatedBy: "IOPMAssertionCreateWithName(").count - 1, 1)
        XCTAssertEqual(code.components(separatedBy: "IOPMAssertionSetProperty(").count - 1, 2)
        XCTAssertTrue(code.contains("CtrlKeepAwake.timeoutActionRelease"))
        let action = try XCTUnwrap(code.range(of: "CtrlKeepAwake.timeoutActionKey"))
        let timeout = try XCTUnwrap(code.range(of: "CtrlKeepAwake.timeoutKey"))
        XCTAssertLessThan(action.lowerBound, timeout.lowerBound, "release-on-timeout is set before the timeout starts")
        for banned in ["Process(", "posix_spawn", "caffeinate", "pmset", "NSTask", "system(", "NSAppleScript", "/bin/",
                       "/usr/", "IOPMAssertionDeclareUserActivity", "IOPMCopyAssertionsByProcess", "IOPMCopyAssertionsStatus",
                       "IOPMAssertionCreateWithDescription", "IOPMSleepSystem", "IOPMAssertionRetain", "beginActivity"] {
            XCTAssertFalse(code.contains(banned), banned)
        }
        // Nothing else in QrtzCtrl makes or releases a power assertion.
        let dir = root.appendingPathComponent("Sources/QrtzCtrl")
        let files = try XCTUnwrap(FileManager.default.enumerator(atPath: dir.path)).compactMap { $0 as? String }
            .filter { $0.hasSuffix(".swift") }
        for f in files where !f.hasSuffix("CtrlKeepAwakeMac.swift") {
            let s = try String(contentsOf: dir.appendingPathComponent(f), encoding: .utf8)
            XCTAssertFalse(s.contains("IOPMAssertionCreate") || s.contains("IOPMAssertionRelease("), f)
        }
    }
}
