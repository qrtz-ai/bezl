// ADR 0166: "open my downloads", helper side. The folder is the closed enum (a path or another spelling never parses),
// resolved from the system's own location, opened through the backend only as the enum, and verified from the
// Finder's window documents / titles without reading the folder.
import Foundation
import XCTest
@testable import QrtzCtrl

private final class FolderFake: CtrlBackend {
    var acted: [CtrlRoute] = []
    var measured: [(String, String)] = []
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], nil) }
    func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw CtrlFailure("app.not_running") }
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { [] }
    func act(_ route: CtrlRoute, content: [String: String]) throws { acted.append(route) }
    func measure(check: String, app: String) -> Int? {
        measured.append((check, app))
        return CtrlFolders.measured(check) == .downloads ? 1 : nil
    }
    func restore(frontmostPid: Int32?) -> Bool { false }
    func laya(state: String, question: String, options: [String]) throws -> [Double] { [] }
    func frontmostPid() -> Int32? { 100 }
    func pid(of app: String) -> Int32? { app == CtrlFolders.finder ? 50 : nil }
}

private func json(_ s: String) -> [String: Any] { try! JSONSerialization.jsonObject(with: Data(s.utf8)) as! [String: Any] }

final class CtrlFoldersTests: XCTestCase {
    func testOnlyTheEnumParses() {
        for f in CtrlFolder.allCases { XCTAssertEqual(CtrlFolders.folder(f.rawValue), f) }
        for bad in ["Downloads", " downloads", "downloads/", "../downloads", "/Users/jared/Downloads", "~/Downloads",
                    "screenshots", "trash", ""] {
            XCTAssertNil(CtrlFolders.folder(bad), bad)
            XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.act","id":"x","actionId":"a","route":{"route":"workspace","op":"open_folder","folder":"# +
                String(decoding: try! JSONSerialization.data(withJSONObject: [bad]), as: UTF8.self).dropFirst().dropLast() + #"},"content":{}}"#), bad)
        }
        XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.act","id":"x","actionId":"a","route":{"route":"workspace","op":"open_folder"},"content":{}}"#))
        XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.act","id":"x","actionId":"a","route":{"route":"workspace","op":"open_folder","folder":1},"content":{}}"#))
    }

    func testOpenFolderReachesTheBackendAsTheEnumOnly() {
        let line = #"{"type":"ctrl.act","id":"c21","actionId":"a7","route":{"route":"workspace","op":"open_folder","folder":"downloads"},"content":{}}"#
        guard case let .act(_, _, route, content)? = CtrlRequest.parse(line) else { return XCTFail("open_folder") }
        XCTAssertEqual(route, .openFolder(folder: .downloads))
        XCTAssertTrue(content.isEmpty)
        XCTAssertEqual(CtrlServer.appOf(route), CtrlFolders.finder)
        XCTAssertNoThrow(try CtrlServer.recheck(route, frontmostPid: 100, targetPid: 50))
        let fake = FolderFake()
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.openFolder(folder: .downloads)])
    }

    func testFolderOpenMeasureIsForTheFinderAndNeedsAFolder() {
        guard case let .measure(_, check, app)? = CtrlRequest.parse(#"{"type":"ctrl.measure","id":"c22","check":{"check":"folder_open","folder":"downloads"}}"#)
        else { return XCTFail("folder_open") }
        XCTAssertEqual(check, "folder_open.downloads")
        XCTAssertEqual(app, CtrlFolders.finder)
        XCTAssertEqual(CtrlFolders.measured(check), .downloads)
        XCTAssertNil(CtrlFolders.measured("folder_open./etc"))
        XCTAssertNil(CtrlFolders.measured("tab_count"))
        XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.measure","id":"x","check":{"check":"folder_open"}}"#))
        XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.measure","id":"x","check":{"check":"folder_open","folder":"/tmp"}}"#))
        XCTAssertNil(CtrlRequest.parse(#"{"type":"ctrl.measure","id":"x","check":{"check":"folder_open","app":"com.apple.finder"}}"#))
        let fake = FolderFake()
        let r = json(CtrlServer(backend: fake).handle(line: #"{"type":"ctrl.measure","id":"c22","check":{"check":"folder_open","folder":"downloads"}}"#))
        XCTAssertEqual(r["value"] as? Int, 1)
        XCTAssertEqual(fake.measured.map { $0.1 }, [CtrlFolders.finder])
    }

    func testTheLocationComesFromTheSystemNotTheWire() {
        for f in CtrlFolder.allCases {
            guard let u = CtrlFolders.url(f) else { return XCTFail("\(f) has a system location") }
            XCTAssertTrue(u.isFileURL)
            XCTAssertEqual(u, FileManager.default.urls(for: CtrlFolders.directory(f), in: .userDomainMask).first!.standardizedFileURL)
        }
        XCTAssertNotEqual(CtrlFolders.url(.downloads), CtrlFolders.url(.documents))
    }

    func testShownFromWindowDocumentsAndTitles() {
        let p = "/Users/me/Downloads"
        XCTAssertEqual(CtrlFolders.shown(windows: 0, documents: [], titles: [], path: p), 0, "no windows at all: not shown")
        XCTAssertEqual(CtrlFolders.shown(windows: 1, documents: ["file:///Users/me/Downloads/"], titles: ["x"], path: p), 1)
        XCTAssertEqual(CtrlFolders.shown(windows: 2, documents: [], titles: ["Documents", "Downloads"], path: p), 1)
        XCTAssertNil(CtrlFolders.shown(windows: 1, documents: ["file:///Users/me/Documents/"], titles: ["Documents"], path: p),
                     "another folder in front: can't tell (a tab behind may show it)")
        XCTAssertNil(CtrlFolders.shown(windows: 1, documents: [], titles: ["Téléchargements"], path: p), "localized title: can't tell")
        XCTAssertNil(CtrlFolders.shown(windows: 1, documents: ["https://example.com/Users/me/Downloads"], titles: [], path: p))
    }
}
