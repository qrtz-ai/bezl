// ctrl guard and policy against the shared fixtures (ADR 0100). Pure; runs on Linux and macOS.
import Foundation
import XCTest
@testable import QrtzCtrl

private let fixtures = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Fixtures/ctrl").standardized

private struct GuardCase: Decodable {
    let name: String
    let elapsed_ms: UInt64
    let before: UiSnapshot
    let after: UiSnapshot
    let violations: [GuardField]
    let inconclusive: [GuardField]
    let user_input: Bool
}

private struct GuardCases: Decodable { let cases: [GuardCase] }

private struct Policy: Decodable {
    let forbidden_ax_actions: [String]
    let forbidden_ax_set_attributes: [String]
    let menu_opening_roles: [String]
    let secure_roles: [String]
    let focus_intent_shortcuts: [String]
    let user_active_window_ms: UInt64
    let cursor_tolerance_pt: Double
}

final class CtrlGuardTests: XCTestCase {
    func testSharedGuardCases() throws {
        let data = try Data(contentsOf: fixtures.appendingPathComponent("guard-cases.json"))
        let cases = try JSONDecoder().decode(GuardCases.self, from: data).cases
        XCTAssertGreaterThanOrEqual(cases.count, 8)
        for c in cases {
            let got = CtrlGuard.compare(c.before, c.after, elapsedMs: c.elapsed_ms)
            XCTAssertEqual(got.violations, c.violations, "\(c.name): violations")
            XCTAssertEqual(got.inconclusive, c.inconclusive, "\(c.name): inconclusive")
            XCTAssertEqual(got.userInput, c.user_input, "\(c.name): user_input")
        }
    }

    func testSnapshotJSONMatchesTheRustShape() throws {
        let s = UiSnapshot(frontmostPid: 501, cursor: CtrlPoint(x: 1, y: 2), focusedElement: "e:1:ff", keyWindow: 9, hidIdleMs: 30)
        let json = String(decoding: try JSONEncoder().encode(s), as: UTF8.self)
        for key in ["\"frontmost_pid\":501", "\"focused_element\":\"e:1:ff\"", "\"key_window\":9", "\"hid_idle_ms\":30"] {
            XCTAssertTrue(json.contains(key), json)
        }
        XCTAssertEqual(try JSONDecoder().decode(UiSnapshot.self, from: Data(json.utf8)), s)
        // A probe that couldn't read a field leaves it out; the Rust side reads that as None.
        XCTAssertEqual(String(decoding: try JSONEncoder().encode(UiSnapshot()), as: UTF8.self), "{}")
    }

    func testGuardedRecordsTheBodysChangeAndError() {
        var ui = UiSnapshot(frontmostPid: 501, cursor: CtrlPoint(x: 5, y: 5), keyWindow: 7, hidIdleMs: 60_000)
        struct Boom: Error {}
        let r = CtrlGuard.guarded(probe: { ui }) {
            ui.frontmostPid = 733
            throw Boom()
        }
        XCTAssertEqual(r.outcome.violations, [.frontmostApp])
        XCTAssertTrue(r.error is Boom)
    }

    func testPolicyTablesMatchTheSharedFixture() throws {
        let p = try JSONDecoder().decode(Policy.self, from: Data(contentsOf: fixtures.appendingPathComponent("policy.json")))
        XCTAssertEqual(p.forbidden_ax_actions, CtrlPolicy.forbiddenAXActions)
        XCTAssertEqual(p.forbidden_ax_set_attributes, CtrlPolicy.forbiddenAXSetAttributes)
        XCTAssertEqual(p.menu_opening_roles, CtrlPolicy.menuOpeningRoles)
        XCTAssertEqual(p.secure_roles, CtrlPolicy.secureRoles)
        XCTAssertEqual(p.focus_intent_shortcuts, CtrlPolicy.focusIntentShortcuts)
        XCTAssertEqual(p.user_active_window_ms, CtrlPolicy.userActiveWindowMs)
        XCTAssertEqual(p.cursor_tolerance_pt, CtrlPolicy.cursorTolerancePt)
    }

    func testPolicyChecks() {
        XCTAssertEqual(CtrlPolicy.normalizeCombo("⌘⇧G"), "cmd+shift+g")
        XCTAssertEqual(CtrlPolicy.normalizeCombo("Shift+Cmd+]"), "cmd+shift+]")
        XCTAssertTrue(CtrlPolicy.isFocusIntent("⌘L"))
        XCTAssertFalse(CtrlPolicy.isFocusIntent("cmd+w"))
        XCTAssertThrowsError(try CtrlPolicy.checkAXAction("AXRaise", role: "AXWindow"))
        XCTAssertThrowsError(try CtrlPolicy.checkAXAction("AXPress", role: "AXMenuItem"))
        XCTAssertThrowsError(try CtrlPolicy.checkAXAction("AXPress", role: "AXSecureTextField"))
        XCTAssertNoThrow(try CtrlPolicy.checkAXAction("AXPress", role: "AXButton"))
        XCTAssertThrowsError(try CtrlPolicy.checkAXSet("AXFrontmost", role: "AXApplication"))
        XCTAssertNoThrow(try CtrlPolicy.checkAXSet("AXValue", role: "AXTextField"))
        XCTAssertThrowsError(try CtrlPolicy.checkKey("cmd+w", targetPid: 5, frontmostPid: 5)) { e in
            XCTAssertEqual(e as? CtrlPolicy.Refusal, .targetIsFrontmost)
        }
        XCTAssertNoThrow(try CtrlPolicy.checkKey("cmd+w", targetPid: 5, frontmostPid: 6))
    }
}

final class CtrlFilesTests: XCTestCase {
    func testDownloadsCleanupMovesAndTrashesWithoutOverwriting() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-ctrl-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let downloads = root.appendingPathComponent("Downloads")
        let archive = downloads.appendingPathComponent("old")
        try FileManager.default.createDirectory(at: archive, withIntermediateDirectories: true)
        for n in ["a.dmg", "b.pdf"] { try Data(n.utf8).write(to: downloads.appendingPathComponent(n)) }

        let moved = try CtrlFiles.moveInto(downloads.appendingPathComponent("a.dmg"), archive)
        XCTAssertEqual(moved.lastPathComponent, "a.dmg")
        XCTAssertTrue(FileManager.default.fileExists(atPath: archive.appendingPathComponent("a.dmg").path))
        try CtrlFiles.trash(downloads.appendingPathComponent("b.pdf"), into: root.appendingPathComponent(".Trash"))
        XCTAssertTrue(FileManager.default.fileExists(atPath: root.appendingPathComponent(".Trash/b.pdf").path))

        try Data("new".utf8).write(to: downloads.appendingPathComponent("a.dmg"))
        XCTAssertThrowsError(try CtrlFiles.moveInto(downloads.appendingPathComponent("a.dmg"), archive)) { e in
            XCTAssertEqual(e as? CtrlFiles.Failure, .exists)
        }
        XCTAssertEqual(try String(contentsOf: archive.appendingPathComponent("a.dmg"), encoding: .utf8), "a.dmg")
        XCTAssertThrowsError(try CtrlFiles.moveInto(downloads.appendingPathComponent("a.dmg"), root.appendingPathComponent("nope")))
    }
}
