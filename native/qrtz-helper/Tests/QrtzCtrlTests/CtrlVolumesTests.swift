// ADR 0176: "eject my disks", helper side. The act and its app-less count parse from the fixture and reach the backend
// with no app and no content; `single` is a strict JSON bool; the eject set and the singular refusal are pure rules.
import Foundation
import XCTest
@testable import QrtzCtrl

private final class VolumesFake: CtrlBackend {
    var acted: [CtrlRoute] = []
    var measured: [(String, String)] = []
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], nil) }
    func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw CtrlFailure("app.not_running") }
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { [] }
    func act(_ route: CtrlRoute, content: [String: String]) throws { acted.append(route) }
    func measure(check: String, app: String) -> Int? {
        measured.append((check, app))
        return check == CtrlVolumes.check ? 2 : nil
    }
    func restore(frontmostPid: Int32?) -> Bool { false }
    func laya(state: String, question: String, options: [String]) throws -> [Double] { [] }
    func frontmostPid() -> Int32? { 100 }
    func pid(of app: String) -> Int32? { nil }
}

private func json(_ s: String) -> [String: Any] { try! JSONSerialization.jsonObject(with: Data(s.utf8)) as! [String: Any] }

private func fixtureLine(_ match: (String) -> Bool) throws -> String {
    let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Fixtures/ctrl/protocol.jsonl")
    let raw = try String(contentsOf: url, encoding: .utf8)
    return try XCTUnwrap(raw.split(separator: "\n").map(String.init).first(where: match))
}

private func act(_ route: String) -> String {
    #"{"type":"ctrl.act","id":"x","actionId":"a","route":"# + route + #","content":{}}"#
}

final class CtrlVolumesTests: XCTestCase {
    func testEjectReachesTheBackendWithNoAppAndNoContent() throws {
        let line = try fixtureLine { $0.contains(#""op":"eject_volumes""#) }
        guard case let .act(_, _, route, content)? = CtrlRequest.parse(line) else { return XCTFail("eject_volumes") }
        XCTAssertEqual(route, .ejectVolumes(single: false))
        XCTAssertTrue(content.isEmpty)
        XCTAssertNil(CtrlServer.appOf(route))
        XCTAssertNoThrow(try CtrlServer.recheck(route, frontmostPid: 100, targetPid: nil))
        let fake = VolumesFake()
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.ejectVolumes(single: false)])
    }

    func testItsCountHasNoAppAndReachesTheBackend() throws {
        let line = try fixtureLine { $0.contains(#""check":"ejectable_volumes""#) }
        guard case let .measure(_, check, app)? = CtrlRequest.parse(line) else { return XCTFail("ejectable_volumes") }
        XCTAssertEqual(check, CtrlVolumes.check)
        XCTAssertEqual(app, "")
        let fake = VolumesFake()
        let r = json(CtrlServer(backend: fake).handle(line: line))
        XCTAssertEqual(r["value"] as? Int, 2, "\(r)")
        XCTAssertEqual(fake.measured.map(\.0), [CtrlVolumes.check])
    }

    func testSingleIsAStrictBool() {
        guard case let .act(_, _, one, _)? = CtrlRequest.parse(act(#"{"route":"workspace","op":"eject_volumes","single":true}"#))
        else { return XCTFail("single true") }
        XCTAssertEqual(one, .ejectVolumes(single: true))
        for bad in [#"{"route":"workspace","op":"eject_volumes"}"#,
                    #"{"route":"workspace","op":"eject_volumes","single":1}"#,
                    #"{"route":"workspace","op":"eject_volumes","single":0}"#,
                    #"{"route":"workspace","op":"eject_volumes","single":"true"}"#,
                    #"{"route":"workspace","op":"eject_volume","single":false}"#,
                    #"{"route":"workspace","op":"eject_volumes","single":false,"path":"/Volumes/x"}"#] {
            let parsed = CtrlRequest.parse(act(bad))
            if bad.contains("path") {
                // An extra field is ignored, never used: the route carries no path.
                guard case let .act(_, _, r, _)? = parsed else { return XCTFail(bad) }
                XCTAssertEqual(r, .ejectVolumes(single: false))
            } else {
                XCTAssertNil(parsed, bad)
            }
        }
    }

    func testOnlyEjectableLocalNonRootVolumesGo() {
        typealias I = CtrlVolumes.Info
        XCTAssertTrue(CtrlVolumes.ejectable(I(ejectable: true, removable: false, local: true, root: false)), "disk image / usb")
        XCTAssertTrue(CtrlVolumes.ejectable(I(ejectable: false, removable: true, local: true, root: false)), "sd card")
        XCTAssertFalse(CtrlVolumes.ejectable(I(ejectable: false, removable: false, local: true, root: false)), "internal partition")
        XCTAssertFalse(CtrlVolumes.ejectable(I(ejectable: true, removable: true, local: false, root: false)), "network share")
        XCTAssertFalse(CtrlVolumes.ejectable(I(ejectable: true, removable: true, local: true, root: true)), "startup disk")
    }

    func testASingularEjectTakesExactlyOne() throws {
        XCTAssertEqual(try CtrlVolumes.plan(["a"], single: true), ["a"])
        XCTAssertEqual(try CtrlVolumes.plan(["a", "b"], single: false), ["a", "b"])
        XCTAssertEqual(try CtrlVolumes.plan([String](), single: false), [])
        XCTAssertThrowsError(try CtrlVolumes.plan(["a", "b"], single: true)) { XCTAssertEqual($0 as? CtrlFailure, CtrlFailure("volumes.not_one")) }
        XCTAssertThrowsError(try CtrlVolumes.plan([String](), single: true)) { XCTAssertEqual($0 as? CtrlFailure, CtrlFailure("volumes.not_one")) }
    }
}
