// ctrl do's tree reader, platform-neutral (ADR 0214). The macOS backend plugs the accessibility API in as a
// `CtrlAXSource`; the walk, the exclusions, the handles and the fingerprints live here, so they build and test on
// Linux against an in-memory source.
//
// Security rules, enforced while reading (never after):
// - an `AXSecureTextField` (or a text field with the secure subrole) is skipped before any attribute but its role is
//   read: its value is never read and it never reaches the host;
// - a text input whose title, placeholder or description names a password or payment field (card number, cvc,
//   iban…) is skipped the same way;
// - a value is read only for plain text inputs and a web area's address, capped, and only to send to the host on the
//   wire (the host hashes it before anything is logged).
import Foundation
import QrtzShared

/// One element on the wire (Rust `ax::AxNodeWire`).
public struct CtrlTreeNode: Equatable, Sendable {
    public var id: String
    public var role: String
    public var subrole: String?
    public var title: String
    public var placeholder: String
    public var value: String?
    public var actions: [String]
    public var enabled: Bool
    public var hidden: Bool
    public var size: [Double]?
    public var focused: Bool
    public var children: [CtrlTreeNode]
    public var more: Int
    public var sig: String

    public init(id: String, role: String, subrole: String? = nil, title: String = "", placeholder: String = "", value: String? = nil,
                actions: [String] = [], enabled: Bool = true, hidden: Bool = false, size: [Double]? = nil, focused: Bool = false,
                children: [CtrlTreeNode] = [], more: Int = 0, sig: String = "") {
        self.id = id; self.role = role; self.subrole = subrole; self.title = title; self.placeholder = placeholder
        self.value = value; self.actions = actions; self.enabled = enabled; self.hidden = hidden; self.size = size
        self.focused = focused; self.children = children; self.more = more; self.sig = sig
    }

    public var json: [String: Any] {
        var o: [String: Any] = ["id": id, "role": role, "title": title, "placeholder": placeholder, "actions": actions,
                                "enabled": enabled, "hidden": hidden, "focused": focused, "children": children.map(\.json),
                                "more": more, "sig": sig]
        if let subrole { o["subrole"] = subrole }
        if let value { o["value"] = value }
        if let size { o["size"] = size }
        return o
    }

    /// Every node, depth first.
    public var all: [CtrlTreeNode] { [self] + children.flatMap(\.all) }
}

public enum CtrlTreeRules {
    /// ADR 0224: the secure-field rule lives in QrtzShared (`SecureField`), shared with echo's insertion.
    public static var secureRoles: Set<String> { SecureField.roles }
    public static var secureSubroles: Set<String> { SecureField.subroles }
    public static let textRoles: Set<String> = ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"]
    /// Review H4: the words that mark a password, identity or payment field come from spec/ctrl/secure-words.json
    /// (`CtrlSecureWords`, generated; the Rust host uses the same list).
    /// The only actions the host may ask for (Rust `ax::KNOWN_AX_ACTIONS`); anything else an element offers is dropped.
    public static let actions: Set<String> = ["AXPress", "AXIncrement", "AXDecrement", "AXShowMenu"]
    public static let maxValue = 500
    public static let maxTitle = 200
    public static let maxNodes = 600

    /// True when the element must not be read at all past its role and subrole.
    public static func secure(role: String, subrole: String?) -> Bool {
        SecureField.isSecure(role: role, subrole: subrole)
    }

    /// True for a text input labelled as a password or payment field.
    public static func sensitive(role: String, labels: [String]) -> Bool {
        guard textRoles.contains(role) else { return false }
        let text = labels.joined(separator: " ").lowercased()
        let list = text.split(whereSeparator: { !$0.isLetter && !$0.isNumber }).map(String.init)
        if list.contains(where: CtrlSecureWords.words.contains) { return true }
        return zip(list, list.dropFirst()).contains { p in CtrlSecureWords.phrases.contains { $0 == p } }
    }

    /// Review H4: a value or title as it may leave the helper. Digit runs that pass the Luhn check (card numbers,
    /// spaces and dashes allowed inside) are dropped, and a page URL loses its query and fragment.
    public static func scrub(_ s: String, url: Bool = false) -> String {
        var t = s
        if url, var c = URLComponents(string: t) { c.query = nil; c.fragment = nil; t = c.string ?? t }
        var out = "", run = ""
        func flush() {
            let digits = run.filter(\.isNumber)
            if digits.count >= 13 && digits.count <= 19 && luhn(digits) { out += " " } else { out += run }
            run = ""
        }
        for ch in t {
            if ch.isASCII && ch.isNumber || (!run.isEmpty && (ch == " " || ch == "-")) { run.append(ch) } else { flush(); out.append(ch) }
        }
        flush()
        return out
    }

    static func luhn(_ d: String) -> Bool {
        var sum = 0
        for (i, c) in d.reversed().enumerated() {
            guard var n = c.wholeNumberValue else { return false }
            if i % 2 == 1 { n *= 2; if n > 9 { n -= 9 } }
            sum += n
        }
        return sum % 10 == 0
    }

    static func cap(_ s: String, _ n: Int) -> String { s.count > n ? String(s.prefix(n)) : s }
}

/// 64-bit FNV-1a, hex: a stable fingerprint of an element's identity (not a secret, not content in the log).
public func ctrlSig(_ parts: [String]) -> String {
    var h: UInt64 = 0xcbf2_9ce4_8422_2325
    for b in parts.joined(separator: "\u{1f}").utf8 {
        h ^= UInt64(b)
        h = h &* 0x0000_0100_0000_01b3
    }
    return String(format: "%016llx", h)
}

/// What a source can tell about one element. Only `role` and `subrole` are read before the secure check.
public protocol CtrlAXSource {
    associatedtype Element
    func role(_ e: Element) -> String?
    func subrole(_ e: Element) -> String?
    func title(_ e: Element) -> String
    func placeholder(_ e: Element) -> String
    func help(_ e: Element) -> String
    /// Only called for a non-secure, non-sensitive text input or a web area.
    func value(_ e: Element) -> String?
    func actions(_ e: Element) -> [String]
    func enabled(_ e: Element) -> Bool
    func size(_ e: Element) -> [Double]?
    func children(_ e: Element) -> [Element]
    func childCount(_ e: Element) -> Int
    func same(_ a: Element, _ b: Element) -> Bool
    /// Review H4: more labels that can name a secure field (the AXTitleUIElement's title, AXDOMIdentifier,
    /// AXDOMClassList).
    func extraLabels(_ e: Element) -> [String]
    /// Review M2: the fingerprint's identity parts: AXIdentifier, the process, and the titles of the ancestors
    /// (window included).
    func identifier(_ e: Element) -> String
    func pid(_ e: Element) -> Int32
    func parent(_ e: Element) -> Element?
}

public extension CtrlAXSource {
    func extraLabels(_ e: Element) -> [String] { [] }
    func identifier(_ e: Element) -> String { "" }
    func pid(_ e: Element) -> Int32 { 0 }
    func parent(_ e: Element) -> Element? { nil }
}

/// Handles this helper created → elements, per app. A new top-level read of an app drops that app's old handles.
public final class CtrlHandles<Element>: @unchecked Sendable {
    private var map: [String: (app: String, el: Element, sig: String)] = [:]
    private var next = 0
    private let lock = NSLock()
    public static var limit: Int { 4096 }
    public init() {}

    public func remember(_ el: Element, app: String, sig: String, prefix: String = "e") -> String {
        lock.lock(); defer { lock.unlock() }
        if map.count >= Self.limit {
            // Review (low): say so; every older handle is now stale.
            FileHandle.standardError.write(Data("ctrl: handle table reset at \(Self.limit)\n".utf8))
            map.removeAll()
        }
        next += 1
        let id = "\(prefix):\(next)"
        map[id] = (app, el, sig)
        return id
    }

    public func get(_ id: String) -> (app: String, el: Element, sig: String)? {
        lock.lock(); defer { lock.unlock() }
        return map[id]
    }

    public func forget(app: String, prefix: String = "e") {
        lock.lock(); defer { lock.unlock() }
        map = map.filter { $0.value.app != app || !$0.key.hasPrefix(prefix + ":") }
    }
}

public struct CtrlTreeReader<S: CtrlAXSource> {
    public let source: S
    public let handles: CtrlHandles<S.Element>
    public init(source: S, handles: CtrlHandles<S.Element>) { self.source = source; self.handles = handles }

    /// The element's fingerprint: role, subrole, title, offered actions, and (review M2) AXIdentifier, the
    /// process, and every ancestor's role and title up to the window (no value). Review H-D: the window's own
    /// title is left out (it changes with the document, unread count, page title), and `ancestry`, when given, is
    /// the ancestor part already built by the tree walk, so a read doesn't climb to the window once per node.
    public func sig(_ e: S.Element, ancestry: [String]? = nil) -> String? {
        guard let role = source.role(e) else { return nil }
        let sub = source.subrole(e)
        var parts = ["\(source.pid(e))", role, sub ?? ""]
        if !CtrlTreeRules.secure(role: role, subrole: sub) {
            parts += [role == "AXWindow" ? "" : source.title(e), source.actions(e).sorted().joined(separator: ","),
                      source.identifier(e)]
        }
        return ctrlSig(parts + (ancestry ?? self.ancestry(e)))
    }

    /// The ancestor part of `sig`: (role, title) of each ancestor, nearest first, up to the window (its title
    /// left out), at most `maxHops` of them.
    public static var maxHops: Int { 24 }
    public func ancestry(_ e: S.Element) -> [String] {
        var out: [String] = []
        var p = source.parent(e)
        var hops = 0
        while let a = p, hops < Self.maxHops {
            out += Self.link(source.role(a) ?? "", source.title(a))
            if source.role(a) == "AXWindow" { break }
            p = source.parent(a); hops += 1
        }
        return out
    }

    /// The ancestry of a child of `e`, given `e`'s own (`parent`): `e`'s link, then `parent`'s, unless `e` is the
    /// window, capped like `ancestry`.
    func childAncestry(role: String, title: String, parent: [String]) -> [String] {
        if role == "AXWindow" { return Self.link(role, title) }
        return Array((Self.link(role, title) + parent).prefix(2 * Self.maxHops))
    }

    static func link(_ role: String, _ title: String) -> [String] { [role, role == "AXWindow" ? "" : title] }

    /// Read `root` down to `depth` levels. Returns nil if the root itself is secure or unreadable.
    public func read(_ root: S.Element, app: String, depth: Int, focused: S.Element?) -> (root: CtrlTreeNode, focused: String?)? {
        var budget = CtrlTreeRules.maxNodes
        var focusedId: String?
        func walk(_ e: S.Element, _ level: Int, _ anc: [String]) -> CtrlTreeNode? {
            guard budget > 0, let role = source.role(e) else { return nil }
            let sub = source.subrole(e)
            if CtrlTreeRules.secure(role: role, subrole: sub) { return nil }
            let title = CtrlTreeRules.cap(CtrlTreeRules.scrub(source.title(e)), CtrlTreeRules.maxTitle)
            let placeholder = CtrlTreeRules.cap(source.placeholder(e), CtrlTreeRules.maxTitle)
            if CtrlTreeRules.sensitive(role: role, labels: [title, placeholder, source.help(e)] + source.extraLabels(e)) { return nil }
            budget -= 1
            let actions = source.actions(e).filter(CtrlTreeRules.actions.contains)
            var value: String?
            if CtrlTreeRules.textRoles.contains(role) || role == "AXWebArea" {
                value = source.value(e).map { CtrlTreeRules.cap(CtrlTreeRules.scrub($0, url: role == "AXWebArea"), CtrlTreeRules.maxValue) }
            }
            let sig = self.sig(e, ancestry: anc) ?? ctrlSig([role])
            let id = handles.remember(e, app: app, sig: sig)
            let isFocused = focused.map { source.same($0, e) } ?? false
            if isFocused { focusedId = id }
            var kids: [CtrlTreeNode] = []
            var more = 0
            if level < depth {
                let down = childAncestry(role: role, title: source.title(e), parent: anc)
                for c in source.children(e) { if let n = walk(c, level + 1, down) { kids.append(n) } }
            } else {
                more = source.childCount(e)
            }
            return CtrlTreeNode(id: id, role: role, subrole: sub, title: title, placeholder: placeholder, value: value,
                                actions: actions, enabled: source.enabled(e), hidden: false, size: source.size(e),
                                focused: isFocused, children: kids, more: more, sig: sig)
        }
        guard let n = walk(root, 0, ancestry(root)) else { return nil }
        return (n, focusedId)
    }
}
