// ADR 0168: "clear my clipboard", helper side. The act and its app-less measure parse from the fixture and reach
// the backend with no app and no content; the verify compares the pasteboard's change count only.
import Foundation
import XCTest
@testable import QrtzCtrl

private final class ClipboardFake: CtrlBackend {
    var acted: [CtrlRoute] = []
    var measured: [(String, String)] = []
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], nil) }
    func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw CtrlFailure("app.not_running") }
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { [] }
    func act(_ route: CtrlRoute, content: [String: String]) throws { acted.append(route) }
    func measure(check: String, app: String) -> Int? {
        measured.append((check, app))
        return check == CtrlClipboard.check ? 1 : nil
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

final class CtrlClipboardTests: XCTestCase {
    func testClearClipboardReachesTheBackendWithNoAppAndNoContent() throws {
        let line = try fixtureLine { $0.contains(#""op":"clear_clipboard""#) }
        guard case let .act(_, _, route, content)? = CtrlRequest.parse(line) else { return XCTFail("clear_clipboard") }
        XCTAssertEqual(route, .clearClipboard)
        XCTAssertTrue(content.isEmpty)
        XCTAssertNil(CtrlServer.appOf(route))
        XCTAssertNoThrow(try CtrlServer.recheck(route, frontmostPid: 100, targetPid: nil))
        let fake = ClipboardFake()
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.clearClipboard])
    }

    func testItsMeasureHasNoAppAndReachesTheBackend() throws {
        let line = try fixtureLine { $0.contains(#""check":"clipboard_cleared""#) }
        guard case let .measure(_, check, app)? = CtrlRequest.parse(line) else { return XCTFail("clipboard_cleared") }
        XCTAssertEqual(check, CtrlClipboard.check)
        XCTAssertEqual(app, "")
        let fake = ClipboardFake()
        let r = json(CtrlServer(backend: fake).handle(line: line))
        XCTAssertEqual(r["value"] as? Int, 1, "\(r)")
        XCTAssertEqual(fake.measured.map(\.0), [CtrlClipboard.check])
    }

    func testClearedComparesTheChangeCountOnly() {
        XCTAssertEqual(CtrlClipboard.cleared(changeCount: 42, lastClear: 42), 1)
        XCTAssertEqual(CtrlClipboard.cleared(changeCount: 43, lastClear: 42), 0, "copied since")
        XCTAssertEqual(CtrlClipboard.cleared(changeCount: 42, lastClear: nil), 0, "never cleared")
    }

    func testOnlyTheBareOpParses() {
        XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.act","id":"x","actionId":"a","route":{"route":"workspace","op":"clear_clipboards"},"content":{}}"#))
        XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.act","id":"x","actionId":"a","route":{"route":"files","op":"clear_clipboard"},"content":{}}"#).flatMap { r -> CtrlRoute? in
            if case let .act(_, _, route, _) = r, route == .clearClipboard { return route }
            return nil
        })
    }
}
