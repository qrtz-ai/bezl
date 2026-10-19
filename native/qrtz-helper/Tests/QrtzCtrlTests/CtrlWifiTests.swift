// ADR 0191: "turn off my wifi", helper side. The act and its app-less measure parse from the fixture and reach the
// backend with no app and no content; `on` must be a JSON bool and nothing else in the request is read; CoreWLAN's
// not-permitted error maps to wifi.needs_admin; the Mac side flips one radio, reads its power, reads nothing about a
// network and spawns nothing.
import Foundation
import XCTest
@testable import QrtzCtrl

private final class WifiFake: CtrlBackend {
    var acted: [CtrlRoute] = []
    var measured: [(String, String)] = []
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], nil) }
    func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw CtrlFailure("app.not_running") }
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { [] }
    func act(_ route: CtrlRoute, content: [String: String]) throws { acted.append(route) }
    func measure(check: String, app: String) -> Int? {
        measured.append((check, app))
        return check == CtrlWifi.check ? 0 : nil
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

final class CtrlWifiTests: XCTestCase {
    func testSetWifiReachesTheBackendWithNoAppAndNoContent() throws {
        let line = try fixtureLine { $0.contains(#""op":"set_wifi""#) }
        guard case let .act(_, _, route, content)? = CtrlRequest.parse(line) else { return XCTFail("set_wifi") }
        XCTAssertEqual(route, .setWifi(on: false))
        XCTAssertTrue(content.isEmpty)
        XCTAssertNil(CtrlServer.appOf(route))
        XCTAssertNoThrow(try CtrlServer.recheck(route, frontmostPid: 100, targetPid: nil))
        let fake = WifiFake()
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.setWifi(on: false)])
    }

    func testItsMeasureHasNoAppAndReachesTheBackend() throws {
        let line = try fixtureLine { $0.contains(#""check":"wifi_on""#) }
        guard case let .measure(_, check, app)? = CtrlRequest.parse(line) else { return XCTFail("wifi_on") }
        XCTAssertEqual(check, CtrlWifi.check)
        XCTAssertEqual(app, "")
        XCTAssertTrue(CtrlRoute.applessChecks.contains(CtrlWifi.check))
        let fake = WifiFake()
        let r = json(CtrlServer(backend: fake).handle(line: line))
        XCTAssertEqual(r["value"] as? Int, 0, "\(r)")
        XCTAssertEqual(fake.measured.map(\.0), [CtrlWifi.check])
    }

    func testOnIsAStrictBoolAndNothingElseIsRead() {
        func parse(_ route: String) -> CtrlRoute? {
            let line = #"{"type":"ctrl.act","id":"x","actionId":"a","route":"# + route + #","content":{}}"#
            guard case let .act(_, _, r, _)? = CtrlRequest.parse(line) else { return nil }
            return r
        }
        XCTAssertEqual(parse(#"{"route":"workspace","op":"set_wifi","on":true}"#), .setWifi(on: true))
        XCTAssertEqual(parse(#"{"route":"workspace","op":"set_wifi","on":false}"#), .setWifi(on: false))
        // A network name riding along is never read: the route is still only the bool.
        XCTAssertEqual(parse(#"{"route":"workspace","op":"set_wifi","on":false,"ssid":"home","password":"x"}"#),
                       .setWifi(on: false))
        for bad in [#"{"route":"workspace","op":"set_wifi","on":1}"#, #"{"route":"workspace","op":"set_wifi","on":0}"#,
                    #"{"route":"workspace","op":"set_wifi","on":"true"}"#, #"{"route":"workspace","op":"set_wifi"}"#,
                    #"{"route":"workspace","op":"set_wifis","on":true}"#, #"{"route":"files","op":"set_wifi","on":true}"#] {
            if let r = parse(bad) {
                if case .setWifi = r { XCTFail(bad) }
            }
        }
    }

    func testErrorsMapWithoutAPromptAndTheMeasureNeverMakesUpAValue() {
        XCTAssertEqual(CtrlWifi.failure(code: CtrlWifi.notPermittedCode), "wifi.needs_admin")
        XCTAssertEqual(CtrlWifi.notPermittedCode, -3930)
        for other in [-3900, -3902, -3906, 0, 1] { XCTAssertEqual(CtrlWifi.failure(code: other), "wifi.refused") }
        XCTAssertEqual(CtrlWifi.value(powerOn: true), 1)
        XCTAssertEqual(CtrlWifi.value(powerOn: false), 0)
        XCTAssertNil(CtrlWifi.value(powerOn: nil))
    }

    /// The Mac side flips one radio's power and reads it back; it never touches a network (no scan, join, leave, name)
    /// and spawns nothing.
    func testTheMacSideFlipsOnlyThePowerAndSpawnsNothing() throws {
        let src = try String(contentsOf: root.appendingPathComponent("Sources/QrtzCtrl/mac/CtrlWifiMac.swift"), encoding: .utf8)
        let code = src.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }.joined(separator: "\n")
        XCTAssertEqual(code.components(separatedBy: ".setPower(").count - 1, 1)
        XCTAssertEqual(code.components(separatedBy: ".powerOn()").count - 1, 1)
        for banned in ["Process(", "posix_spawn", "networksetup", "NSTask", "system(", "NSAppleScript", "/bin/", "/sbin/",
                       "ssid", "bssid", "scanForNetworks", "associate", "disassociate", "cachedScanResults", "configuration",
                       "setPairwiseMasterKey", "setWEPKey", "startIBSSMode", "CLLocationManager", "Keychain", "SecItem"] {
            XCTAssertFalse(code.contains(banned), banned)
        }
        // Nothing else in QrtzCtrl talks to CoreWLAN.
        let dir = root.appendingPathComponent("Sources/QrtzCtrl")
        let files = try XCTUnwrap(FileManager.default.enumerator(atPath: dir.path)).compactMap { $0 as? String }
            .filter { $0.hasSuffix(".swift") }
        for f in files where !f.hasSuffix("CtrlWifiMac.swift") {
            let s = try String(contentsOf: dir.appendingPathComponent(f), encoding: .utf8)
            XCTAssertFalse(s.contains("import CoreWLAN") || s.contains("CWWiFiClient"), f)
        }
    }
}
