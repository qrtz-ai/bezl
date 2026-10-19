// ADR 0158: the focus route's boundary (Bozniak's amendments 1–5): one shortcut name, a closed set of modes, one
// Apple Event target by bundle id, no process spawn, and the grant + usage text on qrtz.app (ADR 0154).
import Foundation
import XCTest
@testable import QrtzCtrl

final class CtrlFocusTests: XCTestCase {
    static let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent("../../").standardized
    static func text(_ rel: String) throws -> String {
        try String(contentsOf: root.appendingPathComponent(rel), encoding: .utf8)
    }

    func testOnlyTheOneShortcutNameBuildsAnEvent() {
        XCTAssertEqual(CtrlFocus.shortcutName, "qrtz Set Focus")
        XCTAssertEqual(CtrlFocus.allowedShortcuts, ["qrtz Set Focus"])
        let s = CtrlFocus.source(shortcut: "qrtz Set Focus", mode: .doNotDisturb)!
        XCTAssertEqual(s, "tell application id \"com.apple.shortcuts.events\" to run shortcut named \"qrtz Set Focus\" with input \"do_not_disturb\"")
        for n in ["qrtz set focus", "Set Focus", "qrtz Set Focus ", "", "x\" & (do shell script \"id\") & \""] {
            XCTAssertNil(CtrlFocus.source(shortcut: n, mode: .off), n)
        }
    }

    func testModesAreAClosedSet() {
        XCTAssertEqual(CtrlFocusMode.allCases.map(\.rawValue),
                       ["do_not_disturb", "work", "personal", "sleep", "driving", "fitness", "gaming", "mindfulness", "reading", "off"])
        for bad in ["", "dnd", "do not disturb", "Do_Not_Disturb", "party", "work ", "off\n", "\"; quit"] {
            XCTAssertNil(CtrlFocus.mode(bad), bad)
            let line = "{\"type\":\"ctrl.act\",\"id\":\"f1\",\"actionId\":\"a1\",\"route\":{\"route\":\"workspace\",\"op\":\"set_focus\",\"mode\":\(String(reflecting: bad))},\"content\":{}}"
            XCTAssertNil(CtrlRequest.parse(line), bad)
        }
        let ok = "{\"type\":\"ctrl.act\",\"id\":\"f1\",\"actionId\":\"a1\",\"route\":{\"route\":\"workspace\",\"op\":\"set_focus\",\"mode\":\"sleep\"},\"content\":{}}"
        XCTAssertEqual(CtrlRequest.parse(ok), .act(id: "f1", actionId: "a1", route: .setFocus(mode: .sleep), content: [:]))
        XCTAssertNil(CtrlRequest.parse(ok.replacingOccurrences(of: "\"sleep\"", with: "7")))
    }

    func testFailuresSayWhatToDoAndNothingElse() {
        XCTAssertEqual(CtrlFocus.failure(errorNumber: -1743).code, "automation.denied")
        XCTAssertEqual(CtrlFocus.failure(errorNumber: -1744).code, "automation.denied")
        XCTAssertEqual(CtrlFocus.failure(errorNumber: -1728).code, "focus.no_shortcut")
        XCTAssertEqual(CtrlFocus.failure(errorNumber: nil).code, "focus.failed")
    }

    /// Amendment 5: the bundle ids named in QrtzCtrl are exactly this set: the two faceless agents (System Events for
    /// the appearance, Shortcuts Events for focus) and the scriptable apps of the tab/window verbs (Safari, Safari
    /// Technology Preview, TextEdit; other browsers come from the catalog at run time). Mail is named only on the Rust
    /// side, as an app id; QrtzCtrl sends it nothing. ADR 0166 names the Finder, for the LaunchServices open
    /// (`activates = false`) of a folder and the AX read of its windows; no new Apple Event or script comes with it.
    /// ADR 0173 names `com.apple.dock` only as the defaults domain the Dock's auto-hide is read from (no Apple Event,
    /// no LaunchServices: the setting goes through System Events, already here). Nothing in QrtzCtrl spawns a process.
    func testTargetsAreExactAndNothingSpawns() throws {
        let dir = Self.root.appendingPathComponent("Sources/QrtzCtrl")
        let files = FileManager.default.enumerator(at: dir, includingPropertiesForKeys: nil)!
            .compactMap { $0 as? URL }.filter { $0.pathExtension == "swift" }
        XCTAssertFalse(files.isEmpty)
        var ids = Set<String>()
        let re = try NSRegularExpression(pattern: "\"(com\\.apple\\.[A-Za-z0-9.\\-]+)\"")
        for f in files {
            let src = try String(contentsOf: f, encoding: .utf8)
            for m in re.matches(in: src, range: NSRange(src.startIndex..., in: src)) {
                ids.insert(String(src[Range(m.range(at: 1), in: src)!]))
            }
            for banned in ["Process(", "Process.run", "posix_spawn", "NSTask", "/usr/bin/shortcuts", "osascript", "system("] {
                XCTAssertFalse(src.contains(banned), "\(f.lastPathComponent) contains \(banned)")
            }
        }
        XCTAssertEqual(ids, ["com.apple.systemevents", "com.apple.shortcuts.events", "com.apple.Safari",
                             "com.apple.SafariTechnologyPreview", "com.apple.TextEdit", "com.apple.finder",
                             "com.apple.dock"])
    }

    /// Amendment 5 + ADR 0154: qrtz.app (the one identity) holds the grant and names the focus shortcut in its text.
    func testQrtzHoldsTheGrantAndNamesTheShortcut() throws {
        let ent = try Self.text("../../apps/desktop/src-tauri/entitlements.mac.plist")
        XCTAssertTrue(ent.contains("<key>com.apple.security.automation.apple-events</key><true/>"))
        let helper = try Self.text("entitlements/ai.qrtz.ctrl.entitlements")
        XCTAssertTrue(helper.contains("com.apple.security.automation.apple-events"))
        let info = try Self.text("../../apps/desktop/src-tauri/Info.plist")
        guard let k = info.range(of: "<key>NSAppleEventsUsageDescription</key>"),
              let open = info.range(of: "<string>", range: k.upperBound..<info.endIndex),
              let close = info.range(of: "</string>", range: open.upperBound..<info.endIndex) else {
            return XCTFail("no NSAppleEventsUsageDescription")
        }
        XCTAssertTrue(info[open.upperBound..<close.lowerBound].contains("qrtz Set Focus"))
    }
}
