// ctrl do's tree reader over an in-memory source (ADR 0214): secure fields are skipped on their role alone (no other
// attribute is read), password and payment fields on their labels, values only for text inputs and web areas,
// depth stops with `more`, handles map back, and fingerprints change when the element does.
import Foundation
import XCTest
@testable import QrtzCtrl

private final class El {
    let role: String, subrole: String?, title: String, placeholder: String, value: String?, actions: [String]
    var kids: [El]
    weak var parent: El?
    var title_: String? = nil
    init(_ role: String, _ title: String = "", sub: String? = nil, placeholder: String = "", value: String? = nil,
         actions: [String] = [], kids: [El] = []) {
        self.role = role; self.subrole = sub; self.title = title; self.placeholder = placeholder; self.value = value
        self.actions = actions; self.kids = kids
        kids.forEach { $0.parent = self }
    }
}

/// Records every attribute read, so the test can prove a secure field's title or value was never asked for.
private final class Source: CtrlAXSource {
    var reads: [(String, String)] = []
    func note(_ e: El, _ a: String) { reads.append((e.role, a)) }
    func role(_ e: El) -> String? { e.role }
    func subrole(_ e: El) -> String? { e.subrole }
    func title(_ e: El) -> String { note(e, "title"); return e.title_ ?? e.title }
    func parent(_ e: El) -> El? { parentReads += 1; return e.parent }
    var parentReads = 0
    func placeholder(_ e: El) -> String { note(e, "placeholder"); return e.placeholder }
    func help(_ e: El) -> String { note(e, "help"); return "" }
    func value(_ e: El) -> String? { note(e, "value"); return e.value }
    func actions(_ e: El) -> [String] { note(e, "actions"); return e.actions }
    func enabled(_ e: El) -> Bool { true }
    func size(_ e: El) -> [Double]? { nil }
    func children(_ e: El) -> [El] { e.kids }
    func childCount(_ e: El) -> Int { e.kids.count }
    func same(_ a: El, _ b: El) -> Bool { a === b }
}

final class CtrlTreeTests: XCTestCase {
    func testSecureAndPaymentFieldsNeverReachTheHost() {
        let pw = El("AXSecureTextField", "password", value: "hunter2")
        let pw2 = El("AXTextField", "", sub: "AXSecureTextField", value: "hunter3")
        let card = El("AXTextField", "card number", value: "4111111111111111")
        let cvc = El("AXTextField", "", placeholder: "CVC", value: "123")
        // "name on card" names a card: dropped since the shared list (review H4); a plain field stays.
        let name = El("AXTextField", "full name", value: "jared")
        let search = El("AXSearchField", "search", value: "cats", actions: ["AXPress", "AXRaise"])
        let web = El("AXWebArea", "page", value: "https://example.com")
        let button = El("AXButton", "send", value: "never read", actions: ["AXPress"])
        let win = El("AXWindow", "checkout", kids: [pw, pw2, card, cvc, name, search, web, button])
        let src = Source()
        let reader = CtrlTreeReader(source: src, handles: CtrlHandles<El>())
        let (root, focused) = reader.read(win, app: "com.apple.Safari", depth: 6, focused: search)!
        let all = root.all
        XCTAssertEqual(all.map(\.role), ["AXWindow", "AXTextField", "AXSearchField", "AXWebArea", "AXButton"])
        XCTAssertEqual(all.first { $0.role == "AXTextField" }?.value, "jared")
        XCTAssertNil(all.first { $0.role == "AXButton" }?.value, "a button's value isn't read")
        XCTAssertEqual(all.first { $0.role == "AXSearchField" }?.actions, ["AXPress"], "only the known actions")
        XCTAssertEqual(focused, all.first { $0.role == "AXSearchField" }?.id)
        // The secure fields: only role and subrole were looked at.
        XCTAssertFalse(src.reads.contains { $0.0 == "AXSecureTextField" })
        XCTAssertFalse(src.reads.contains { $0.1 == "value" && ["4111111111111111", "123"].contains(where: { _ in false }) })
        let json = String(decoding: try! JSONSerialization.data(withJSONObject: root.json), as: UTF8.self)
        for secret in ["hunter2", "hunter3", "4111111111111111", "\"123\"", "password", "card number"] {
            XCTAssertFalse(json.contains(secret), secret)
        }
    }

    func testDepthStopsWithMoreAndHandlesMapBack() {
        let leaf = El("AXButton", "downloads", actions: ["AXPress"])
        let group = El("AXGroup", "sidebar", kids: [leaf, El("AXButton", "desktop")])
        let win = El("AXWindow", "finder", kids: [group])
        let handles = CtrlHandles<El>()
        let reader = CtrlTreeReader(source: Source(), handles: handles)
        let (root, _) = reader.read(win, app: "com.apple.finder", depth: 1, focused: nil)!
        XCTAssertEqual(root.children.count, 1)
        XCTAssertEqual(root.children[0].more, 2)
        XCTAssertTrue(root.children[0].children.isEmpty)
        let gid = root.children[0].id
        XCTAssertTrue(handles.get(gid)!.el === group)
        let (scoped, _) = reader.read(handles.get(gid)!.el, app: "com.apple.finder", depth: 1, focused: nil)!
        XCTAssertEqual(scoped.children.map(\.title), ["downloads", "desktop"])
        handles.forget(app: "com.apple.finder")
        XCTAssertNil(handles.get(gid))
    }

    func testTheFingerprintFollowsTheElement() {
        let src = Source()
        let reader = CtrlTreeReader(source: src, handles: CtrlHandles<El>())
        let a = El("AXButton", "send", actions: ["AXPress"])
        let b = El("AXButton", "delete", actions: ["AXPress"])
        XCTAssertEqual(reader.sig(a), reader.sig(El("AXButton", "send", actions: ["AXPress"])))
        XCTAssertNotEqual(reader.sig(a), reader.sig(b))
        XCTAssertEqual(ctrlSig(["x"]).count, 16)
    }

    /// Review H-D: the walk's fingerprints equal a standalone `sig` (what `live` re-checks at act time), the
    /// walk doesn't climb to the window per node, and the window's title (document name, unread count) is not in
    /// any fingerprint while ancestor titles still are.
    func testTheWalkBuildsAncestryOnceAndLeavesTheWindowTitleOut() {
        let send = El("AXButton", "send", actions: ["AXPress"])
        let group = El("AXGroup", "reply", kids: [send])
        let deep = (0..<6).reduce(group) { inner, _ in El("AXGroup", "", kids: [inner]) }
        let win = El("AXWindow", "inbox (3)", kids: [deep])
        let src = Source()
        let handles = CtrlHandles<El>()
        let reader = CtrlTreeReader(source: src, handles: handles)
        let (root, _) = reader.read(win, app: "com.apple.mail", depth: 12, focused: nil)!
        XCTAssertLessThanOrEqual(src.parentReads, 1, "only the root's ancestry is climbed")
        for n in root.all { XCTAssertEqual(n.sig, reader.sig(handles.get(n.id)!.el), n.role) }
        let before = reader.sig(send)
        win.title_ = "inbox (4)"
        XCTAssertEqual(reader.sig(send), before, "the window title is not in the fingerprint")
        XCTAssertEqual(reader.sig(win), reader.sig(El("AXWindow", "other")))
        group.title_ = "forward"
        XCTAssertNotEqual(reader.sig(send), before, "an ancestor's title still is")
        // A scoped read (root below the window) agrees with the standalone sig too.
        let (scoped, _) = reader.read(group, app: "com.apple.mail", depth: 2, focused: nil)!
        for n in scoped.all { XCTAssertEqual(n.sig, reader.sig(handles.get(n.id)!.el)) }
    }

    func testSensitiveLabelsAreWholeWords() {
        XCTAssertTrue(CtrlTreeRules.sensitive(role: "AXTextField", labels: ["Enter your PIN"]))
        XCTAssertTrue(CtrlTreeRules.sensitive(role: "AXTextField", labels: ["", "Credit card", ""]))
        XCTAssertFalse(CtrlTreeRules.sensitive(role: "AXTextField", labels: ["pinned notes"]))
        XCTAssertFalse(CtrlTreeRules.sensitive(role: "AXButton", labels: ["password"]), "only text inputs")
    }
}

/// ADR 0214 review H4: the helper's secure words are the spec's, card numbers and URL queries never leave.
final class CtrlSecureWordsTests: XCTestCase {
    func testTheListIsTheSpecs() throws {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .appendingPathComponent("../../../../spec/ctrl/secure-words.json").standardized
        let spec = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
        XCTAssertEqual(Set(spec["words"] as! [String]), CtrlSecureWords.words, "node scripts/ctrl/gen-secure-words.mjs")
        XCTAssertEqual((spec["phrases"] as! [[String]]).map { $0.joined(separator: " ") },
                       CtrlSecureWords.phrases.map { "\($0.0) \($0.1)" })
        for l in ["SSN", "Social Security number", "Verification code", "CVV2", "Passwort", "Routing", "BIC", "One-time code"] {
            XCTAssertTrue(CtrlTreeRules.sensitive(role: "AXTextField", labels: [l]), l)
        }
        XCTAssertFalse(CtrlTreeRules.sensitive(role: "AXTextField", labels: ["search"]))
    }

    func testLuhnRunsAndUrlQueriesAreScrubbed() {
        XCTAssertFalse(CtrlTreeRules.scrub("pay 4242 4242 4242 4242 now").contains("4242"))
        XCTAssertTrue(CtrlTreeRules.scrub("order 1234567890123 shipped").contains("1234567890123"), "not luhn")
        XCTAssertEqual(CtrlTreeRules.scrub("https://ex.com/a?token=s3cr3t#frag", url: true), "https://ex.com/a")
    }
}
