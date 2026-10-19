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
    public static let secureRoles: Set<String> = ["AXSecureTextField"]
    public static let secureSubroles: Set<String> = ["AXSecureTextField"]
    public static let textRoles: Set<String> = ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"]
    /// Words that mark a password or payment field (whole words of the lowercased label, or these phrases).
    public static let sensitivePhrases = ["password", "passcode", "passphrase", "card number", "credit card", "debit card",
                                          "security code", "cvv", "cvc", "csc", "expiry", "expiration", "iban",
                                          "routing number", "account number", "pin", "one-time code", "2fa", "otp"]
    /// The only actions the host may ask for (Rust `ax::KNOWN_AX_ACTIONS`); anything else an element offers is dropped.
    public static let actions: Set<String> = ["AXPress", "AXIncrement", "AXDecrement", "AXShowMenu"]
    public static let maxValue = 500
    public static let maxTitle = 200
    public static let maxNodes = 600

    /// True when the element must not be read at all past its role and subrole.
    public static func secure(role: String, subrole: String?) -> Bool {
        secureRoles.contains(role) || subrole.map(secureSubroles.contains) == true
    }

    /// True for a text input labelled as a password or payment field.
    public static func sensitive(role: String, labels: [String]) -> Bool {
        guard textRoles.contains(role) else { return false }
        let text = labels.joined(separator: " ").lowercased()
        let words = Set(text.split(whereSeparator: { !$0.isLetter && !$0.isNumber && $0 != "-" }).map(String.init))
        return sensitivePhrases.contains { p in p.contains(" ") ? text.contains(p) : words.contains(p) }
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
        if map.count >= Self.limit { map.removeAll() }
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

    /// The element's fingerprint: role, subrole, title and offered actions (no value).
    public func sig(_ e: S.Element) -> String? {
        guard let role = source.role(e) else { return nil }
        let sub = source.subrole(e)
        if CtrlTreeRules.secure(role: role, subrole: sub) { return ctrlSig([role, sub ?? ""]) }
        return ctrlSig([role, sub ?? "", source.title(e), source.actions(e).sorted().joined(separator: ",")])
    }

    /// Read `root` down to `depth` levels. Returns nil if the root itself is secure or unreadable.
    public func read(_ root: S.Element, app: String, depth: Int, focused: S.Element?) -> (root: CtrlTreeNode, focused: String?)? {
        var budget = CtrlTreeRules.maxNodes
        var focusedId: String?
        func walk(_ e: S.Element, _ level: Int) -> CtrlTreeNode? {
            guard budget > 0, let role = source.role(e) else { return nil }
            let sub = source.subrole(e)
            if CtrlTreeRules.secure(role: role, subrole: sub) { return nil }
            let title = CtrlTreeRules.cap(source.title(e), CtrlTreeRules.maxTitle)
            let placeholder = CtrlTreeRules.cap(source.placeholder(e), CtrlTreeRules.maxTitle)
            if CtrlTreeRules.sensitive(role: role, labels: [title, placeholder, source.help(e)]) { return nil }
            budget -= 1
            let actions = source.actions(e).filter(CtrlTreeRules.actions.contains)
            var value: String?
            if CtrlTreeRules.textRoles.contains(role) || role == "AXWebArea" {
                value = source.value(e).map { CtrlTreeRules.cap($0, CtrlTreeRules.maxValue) }
            }
            let sig = ctrlSig([role, sub ?? "", source.title(e), source.actions(e).sorted().joined(separator: ",")])
            let id = handles.remember(e, app: app, sig: sig)
            let isFocused = focused.map { source.same($0, e) } ?? false
            if isFocused { focusedId = id }
            var kids: [CtrlTreeNode] = []
            var more = 0
            if level < depth {
                for c in source.children(e) { if let n = walk(c, level + 1) { kids.append(n) } }
            } else {
                more = source.childCount(e)
            }
            return CtrlTreeNode(id: id, role: role, subrole: sub, title: title, placeholder: placeholder, value: value,
                                actions: actions, enabled: source.enabled(e), hidden: false, size: source.size(e),
                                focused: isFocused, children: kids, more: more, sig: sig)
        }
        guard let n = walk(root, 0) else { return nil }
        return (n, focusedId)
    }
}
