// Run 12 (ADR 0119): the plain-data half of the Mac tabs backend. Pinned only when the AX tab bar says so and lines
// up with the scripted window; unsaved only from the page script; anything unknown is never closed unasked.
import XCTest
@testable import QrtzCtrl

final class CtrlTabsTests: XCTestCase {
    let us = "\u{1F}", rs = "\u{1E}"
    func rec(_ w: Int, _ i: Int, _ t: String, _ u: String, _ js: String) -> String { [String(w), String(i), t, u, js].joined(separator: us) + rs }

    func testEnumerationScriptsNeverActivateAndOnlyReadThePage() {
        for (id, flavor) in [("com.apple.Safari", CtrlTabFlavor.safari), ("com.google.Chrome", .chromium)] {
            let s = CtrlTabs.enumerateSource(bundleId: id, flavor: flavor)!
            XCTAssertTrue(s.hasPrefix("tell application id \"\(id)\"\n"))
            for word in ["activate", "launch", "reopen", "set current tab", "set active tab", "set index", "keystroke", "do shell script", "close"] {
                XCTAssertFalse(s.contains(word), "\(id): \(word)")
            }
            XCTAssertTrue(s.contains(flavor == .safari ? "do JavaScript" : "execute t javascript"))
            XCTAssertTrue(s.contains(flavor == .safari ? "(name of t)" : "(title of t)"))
        }
        // The page script returns one character and never writes a field or navigates.
        for word in [".value=", ".value =", "location", "submit(", "click(", "fetch(", "XMLHttpRequest", "\""] {
            XCTAssertFalse(CtrlTabs.unsavedJS.contains(word), word)
        }
        XCTAssertNil(CtrlTabs.enumerateSource(bundleId: "com.apple.Safari\" to activate --", flavor: .safari))
        XCTAssertEqual(CtrlTabFlavor.of("com.brave.Browser"), .chromium)
        XCTAssertNil(CtrlTabFlavor.of("company.thebrowser.Browser"))
    }

    func testParseIsAllOrNothing() {
        let out = rec(7, 1, "mail", "https://mail.example", "0") + rec(7, 2, "form", "https://f.example", "1") + rec(9, 1, "x", "", "?")
        let rows = CtrlTabs.parse(out)!
        XCTAssertEqual(rows.map(\.unsaved), [false, true, nil])
        XCTAssertEqual(rows[2].windowId, 9)
        XCTAssertNil(CtrlTabs.parse(out + "7\u{1F}x\u{1E}"))
        XCTAssertEqual(CtrlTabs.parse("")!, [])
    }

    func testSafariPinnedFromRoleDescriptionAndChromiumFromItsLabel() {
        XCTAssertEqual(CtrlTabs.pinned(.init(name: "a", roleDescription: "pinned tab"), title: "a", flavor: .safari, english: true), true)
        XCTAssertEqual(CtrlTabs.pinned(.init(name: "a", roleDescription: "tab"), title: "a", flavor: .safari, english: true), false)
        XCTAssertNil(CtrlTabs.pinned(.init(name: "a", roleDescription: "onglet épinglé"), title: "a", flavor: .safari, english: false))
        XCTAssertEqual(CtrlTabs.pinned(.init(name: "inbox", roleDescription: "tab"), title: "inbox", flavor: .chromium, english: true), false)
        XCTAssertEqual(CtrlTabs.pinned(.init(name: "inbox - Pinned", roleDescription: "tab"), title: "inbox", flavor: .chromium, english: true), true)
        XCTAssertEqual(CtrlTabs.pinned(.init(name: "song - Audio playing", roleDescription: "tab"), title: "song", flavor: .chromium, english: true), false)
        // Another UI language: an unknown suffix could be "pinned" in that language.
        XCTAssertNil(CtrlTabs.pinned(.init(name: "inbox - Angeheftet", roleDescription: "Tab"), title: "inbox", flavor: .chromium, english: false))
        XCTAssertNil(CtrlTabs.pinned(.init(name: "other", roleDescription: "tab"), title: "inbox", flavor: .chromium, english: true))
    }

    func rows() -> [CtrlTabRow] {
        CtrlTabs.parse(rec(7, 1, "cal", "https://cal", "0") + rec(7, 2, "news", "https://news", "0") + rec(7, 3, "form", "https://form", "1")
            + rec(7, 4, "slow", "https://slow", "?") + rec(7, 5, "", "favorites://", "0"))!
    }
    let strip: [CtrlAXTab] = [.init(name: "cal", roleDescription: "pinned tab"), .init(name: "news", roleDescription: "tab"),
                              .init(name: "form", roleDescription: "tab"), .init(name: "slow", roleDescription: "tab"),
                              .init(name: "", roleDescription: "tab")]

    func testCloseSavedTouchesOnlyKnownPlainTabsAndUnpinnedNeverPinned() {
        let tabs = CtrlTabs.join(rows(), strips: [[.init(name: "x", roleDescription: "tab")], strip], flavor: .safari, english: true)
        XCTAssertEqual(tabs.map(\.pinned), [true, false, false, false, false])
        XCTAssertEqual(CtrlTabs.targets(tabs, verb: "close_saved_tabs").map(\.row.title), ["news", ""])
        XCTAssertEqual(CtrlTabs.targets(tabs, verb: "close_unpinned_tabs").map(\.row.title), ["news", "form", "slow", ""])
        XCTAssertEqual(CtrlTabs.count(tabs, check: "saved_tabs"), 1) // the blank favorites page isn't counted
        XCTAssertEqual(CtrlTabs.count(tabs, check: "unpinned_tabs"), 3)
        XCTAssertEqual(CtrlTabs.preview(tabs).map(\.pinned), [true, false, false, false, false])
        XCTAssertEqual(CtrlTabs.preview(tabs)[2].unsaved, true)
        XCTAssertNil(CtrlTabs.preview(tabs)[3].unsaved)
        XCTAssertTrue(CtrlTabs.targets(tabs, verb: "close_all_tabs").isEmpty)
    }

    func testNoAXOrAStripThatDoesNotLineUpClosesNothing() {
        // No AX grant: no strips.
        let blind = CtrlTabs.join(rows(), strips: [], flavor: .safari, english: true)
        XCTAssertTrue(blind.allSatisfy { $0.pinned == nil })
        XCTAssertTrue(CtrlTabs.targets(blind, verb: "close_saved_tabs").isEmpty)
        XCTAssertTrue(CtrlTabs.targets(blind, verb: "close_unpinned_tabs").isEmpty)
        // …and an approved close that closed nothing doesn't verify.
        XCTAssertEqual(CtrlTabs.count(blind, check: "unpinned_tabs"), 4)
        // Two strips that both line up: ambiguous.
        XCTAssertTrue(CtrlTabs.join(rows(), strips: [strip, strip], flavor: .safari, english: true).allSatisfy { $0.pinned == nil })
        // Titles out of order.
        var swapped = strip; swapped.swapAt(1, 2)
        XCTAssertTrue(CtrlTabs.join(rows(), strips: [swapped], flavor: .safari, english: true).allSatisfy { $0.pinned == nil })
        // A "pinned" tab after a plain one: misread, so nothing is known.
        var late = strip; late[3] = .init(name: "slow", roleDescription: "pinned tab")
        XCTAssertTrue(CtrlTabs.join(rows(), strips: [late], flavor: .safari, english: true).allSatisfy { $0.pinned == nil })
    }

    func testCloseScriptGoesHighestIndexFirstAndRechecksEachURL() {
        let tabs = CtrlTabs.join(rows(), strips: [strip], flavor: .safari, english: true)
        let s = CtrlTabs.closeSource(bundleId: "com.apple.Safari", targets: CtrlTabs.targets(tabs, verb: "close_unpinned_tabs"))!
        let i5 = s.range(of: "close tab 5 of window id 7")!, i2 = s.range(of: "close tab 2 of window id 7")!
        XCTAssertLessThan(i5.lowerBound, i2.lowerBound)
        XCTAssertTrue(s.contains("if ((URL of tab 3 of window id 7) as text) is \"https://form\" then close tab 3 of window id 7"))
        XCTAssertFalse(s.contains("tab 1 of"), "the pinned tab is never named")
        for word in ["activate", "saving no", "close every", "launch"] { XCTAssertFalse(s.contains(word), word) }
        XCTAssertNil(CtrlTabs.closeSource(bundleId: "com.apple.Safari", targets: []))
        XCTAssertEqual(CtrlTabs.literal(#"a"b\c"#), #""a\"b\\c""#)
        XCTAssertNil(CtrlTabs.literal("a\nb"))
    }

    func testServerLetsTheTabVerbsThroughToTheBackendOnly() throws {
        XCTAssertNoThrow(try CtrlServer.recheck(.scripting(app: "com.apple.Safari", verb: "close_saved_tabs"), frontmostPid: 1, targetPid: 2))
        XCTAssertNoThrow(try CtrlServer.recheck(.scripting(app: "com.apple.Safari", verb: "close_unpinned_tabs"), frontmostPid: 1, targetPid: 2))
        XCTAssertThrowsError(try CtrlServer.recheck(.scripting(app: "com.apple.Safari", verb: "close_everything"), frontmostPid: 1, targetPid: 2))
        XCTAssertNil(CtrlScripts.source(verb: "close_saved_tabs", bundleId: "com.apple.Safari"))
    }

    /// A browser as the backend sees it: scripted rows + the AX strip; closing removes rows and re-indexes.
    final class Browser: CtrlBackend {
        var rows: [CtrlTabRow], strip: [CtrlAXTab]?, front: Int32 = 100, scripts: [String] = []
        init(rows: [CtrlTabRow], strip: [CtrlAXTab]?) { self.rows = rows; self.strip = strip }
        var tabs: [CtrlTab] { CtrlTabs.join(rows, strips: strip.map { [$0] } ?? [], flavor: .safari, english: true) }
        func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], nil) }
        func probe() throws -> (env: [String: Any], ui: [String: Any]) { ([:], [:]) }
        func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { [] }
        func act(_ route: CtrlRoute, content: [String: String]) throws {
            guard case let .scripting(app, verb) = route, CtrlRoute.tabVerbs.contains(verb) else { throw CtrlFailure("policy.refused") }
            let t = CtrlTabs.targets(tabs, verb: verb)
            if let s = CtrlTabs.closeSource(bundleId: app, targets: t) { scripts.append(s) }
            let gone = Set(t.map(\.row.index))
            var keep: [CtrlTabRow] = [], keepAX: [CtrlAXTab] = []
            for (i, r) in rows.enumerated() where !gone.contains(r.index) {
                keep.append(CtrlTabRow(windowId: r.windowId, index: keep.count + 1, title: r.title, url: r.url, unsaved: r.unsaved))
                if let s = strip { keepAX.append(s[i]) }
            }
            rows = keep
            if strip != nil { strip = keepAX }
        }
        func measure(check: String, app: String) -> Int? { CtrlTabs.count(tabs, check: check) }
        func restore(frontmostPid: Int32?) -> Bool { false }
        func laya(state: String, question: String, options: [String]) throws -> [Double] { [] }
        func frontmostPid() -> Int32? { front }
        func pid(of app: String) -> Int32? { 200 }
        func preview(app: String, what: String) throws -> [CtrlPreviewItem] { CtrlTabs.preview(tabs) }
    }

    func obj(_ s: String) -> [String: Any] { try! JSONSerialization.jsonObject(with: Data(s.utf8)) as! [String: Any] }

    /// ADR 0114's sequence over the wire: preview → close_saved_tabs → saved_tabs 0 → (gate approved)
    /// close_unpinned_tabs → unpinned_tabs 0; the pinned tab stays, the browser is never brought forward.
    func testCloseAllMyTabsOverTheWire() {
        let b = Browser(rows: rows(), strip: strip)
        let server = CtrlServer(backend: b)
        let pv = obj(server.handle(line: #"{"type":"ctrl.preview","id":"p","app":"com.apple.Safari","what":"tabs"}"#))
        let items = pv["items"] as! [[String: Any]]
        XCTAssertEqual(items.map { $0["pinned"] as? Bool }, [true, false, false, false, false])
        XCTAssertEqual(items.map { $0["unsaved"] as? Bool }, [false, false, true, nil, false])
        let act = { (v: String) in self.obj(server.handle(line: #"{"type":"ctrl.act","id":"a","actionId":"x","route":{"route":"scripting","app":"com.apple.Safari","verb":"\#(v)"},"content":{}}"#)) }
        let measure = { (c: String) in self.obj(server.handle(line: #"{"type":"ctrl.measure","id":"m","check":{"check":"\#(c)","app":"com.apple.Safari"}}"#))["value"] as? Int }
        XCTAssertEqual(act("close_saved_tabs")["ok"] as? Bool, true)
        XCTAssertEqual(b.rows.map(\.title), ["cal", "form", "slow"])
        XCTAssertEqual(measure("saved_tabs"), 0)
        XCTAssertEqual(measure("unpinned_tabs"), 2)
        XCTAssertEqual(act("close_unpinned_tabs")["ok"] as? Bool, true)
        XCTAssertEqual(b.rows.map(\.title), ["cal"])
        XCTAssertEqual(measure("unpinned_tabs"), 0)
        XCTAssertEqual(b.front, 100)
        for s in b.scripts { XCTAssertFalse(s.contains("tab 1 of"), "the pinned tab is never named"); XCTAssertFalse(s.contains("activate")) }
    }

    /// No AX grant: nothing closes, and the approved close doesn't verify (the host asks).
    func testWithoutTheTabBarNothingClosesAndNothingVerifies() {
        let b = Browser(rows: rows(), strip: nil)
        let server = CtrlServer(backend: b)
        for v in ["close_saved_tabs", "close_unpinned_tabs"] {
            _ = server.handle(line: #"{"type":"ctrl.act","id":"a","actionId":"x","route":{"route":"scripting","app":"com.apple.Safari","verb":"\#(v)"},"content":{}}"#)
        }
        XCTAssertEqual(b.rows.count, 5)
        XCTAssertTrue(b.scripts.isEmpty)
        let m = obj(server.handle(line: #"{"type":"ctrl.measure","id":"m","check":{"check":"unpinned_tabs","app":"com.apple.Safari"}}"#))
        XCTAssertEqual(m["value"] as? Int, 4)
    }
}
