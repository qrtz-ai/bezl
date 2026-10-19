// The ctrl helper protocol, helper side, version 2 (ctrl do, ADR 0214). Mirrors crates/qrtz-ctrl/src/helper.rs (the
// source of truth); both sides test against Fixtures/ctrl/protocol.jsonl. One request line in, one reply line out,
// all under the `ctrl.` prefix:
//
// - `ctrl.catalog`: installed and running apps, the default browser.
// - `ctrl.tree {app, scope?, depth}`: the front window of `app` (or one element's children). Secure text fields and
//   password / payment fields are left out here: their values are never read and they never reach the host.
// - `ctrl.menu.shortcuts {app}`: the menu-bar items that have a key equivalent.
// - `ctrl.ax.act {runId, action, sig?}`: one action of the fixed set (`CtrlAction`) on a handle this helper created.
// - `ctrl.observe {runId, arm?}`: the stop-key watcher (Esc stops, any other physical input stops too).
// - `ctrl.laya`, `ctrl.laya.warm`: the consented Laya model (ADR 0106, 0133).
//
// Foundation-only: parsing, the policy re-check and the dispatch build and test on Linux. The server re-checks every
// action before the backend sees it (defense in depth next to the host's own check): no AppleScript, no coordinate
// click, no free typing, no combo outside the allowed shape, no non-http(s) address.
import Foundation

public let ctrlProtocolVersion = 2

/// One action of ctrl do's fixed set (Rust `act::Action`), with its text resolved for this call only.
public enum CtrlAction: Equatable, Sendable {
    case press(handle: String)
    case setValue(handle: String, text: String, submit: Bool)
    case increment(handle: String)
    case decrement(handle: String)
    case showMenu(handle: String)
    case focus(handle: String)
    case scroll(handle: String, down: Bool)
    case keyCombo(app: String, combo: String)
    case openURL(app: String, url: String)
    case openApp(app: String)

    public var kind: String {
        switch self {
        case .press: return "press"
        case .setValue: return "set_value"
        case .increment: return "increment"
        case .decrement: return "decrement"
        case .showMenu: return "show_menu"
        case .focus: return "focus"
        case .scroll: return "scroll"
        case .keyCombo: return "key_combo"
        case .openURL: return "open_url"
        case .openApp: return "open_app"
        }
    }

    public var handle: String? {
        switch self {
        case let .press(h), let .setValue(h, _, _), let .increment(h), let .decrement(h), let .showMenu(h), let .focus(h),
             let .scroll(h, _): return h
        case .keyCombo, .openURL, .openApp: return nil
        }
    }

    static func parse(_ o: [String: Any]) -> CtrlAction? {
        let s = { (k: String) in o[k] as? String }
        let h = s("handle")
        switch s("action") {
        case "press": return h.map { .press(handle: $0) }
        case "set_value":
            guard let h, let t = s("text"), let n = o["submit"] as? NSNumber, String(cString: n.objCType) == "c" else { return nil }
            return .setValue(handle: h, text: t, submit: n.boolValue)
        case "increment": return h.map { .increment(handle: $0) }
        case "decrement": return h.map { .decrement(handle: $0) }
        case "show_menu": return h.map { .showMenu(handle: $0) }
        case "focus": return h.map { .focus(handle: $0) }
        case "scroll":
            guard let h, let d = s("dir"), d == "up" || d == "down" else { return nil }
            return .scroll(handle: h, down: d == "down")
        case "key_combo":
            guard let a = s("app"), let c = s("combo") else { return nil }
            return .keyCombo(app: a, combo: c)
        case "open_url":
            guard let a = s("app"), let u = s("url") else { return nil }
            return .openURL(app: a, url: u)
        case "open_app": return s("app").map { .openApp(app: $0) }
        default: return nil
        }
    }
}

/// The helper's own re-check of an action (Rust `act::check` is the host's). Refusals are short codes.
public enum CtrlDoPolicy {
    /// The stop key is never sent; Esc alone and the app switchers are never ctrl's to press.
    /// Rust `RESERVED_COMBOS` plus the system's app and input switchers.
    public static let reservedCombos: Set<String> = ["escape", "cmd+option+escape", "cmd+tab", "cmd+shift+tab", "cmd+space"]
    /// Rust `normalize_combo`'s order.
    public static let modifiers = ["cmd", "ctrl", "option", "shift"]
    /// Refused while the focused element is secure (review, low).
    public static let clipboardCombos: Set<String> = ["cmd+v", "cmd+c", "cmd+x", "cmd+a", "cmd+shift+v", "cmd+option+shift+v"]
    /// Rust `act::TEXT_MAX`.
    public static let maxText = 500

    /// `cmd+shift+t`: modifiers in canonical order, then one known key; at least one modifier unless the key is Return.
    public static func comboAllowed(_ combo: String) -> Bool {
        if reservedCombos.contains(combo) { return false }
        let parts = combo.split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        guard let key = parts.last, !key.isEmpty, !modifiers.contains(key) else { return false }
        let mods = parts.dropLast()
        var last = -1
        for m in mods {
            guard let i = modifiers.firstIndex(of: m), i > last else { return false }
            last = i
        }
        if mods.isEmpty { return key == "return" }
        return CtrlKeys.code(key) != nil
    }

    public static func check(_ a: CtrlAction) throws {
        if let h = a.handle, !(h.hasPrefix("e:") || h.hasPrefix("m:")) { throw CtrlFailure("act.refused") }
        switch a {
        case let .setValue(_, text, _):
            if text.isEmpty || text.count > maxText { throw CtrlFailure("act.refused") }
        case let .keyCombo(_, combo):
            if !comboAllowed(combo) { throw CtrlFailure("act.refused") }
        case let .openURL(_, url):
            guard let u = URL(string: url), let s = u.scheme?.lowercased(), s == "http" || s == "https", u.host != nil
            else { throw CtrlFailure("act.refused") }
        case .press, .increment, .decrement, .showMenu, .focus, .scroll, .openApp: break
        }
    }
}

/// Virtual key codes for the keys a combo may name (US layout, Carbon's kVK_*).
public enum CtrlKeys {
    static let letters: [String: UInt16] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13,
        "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "equal": 24, "9": 25,
        "7": 26, "minus": 27, "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "return": 36,
        "l": 37, "j": 38, "k": 40, ";": 41, "comma": 43, "/": 44, "n": 45, "m": 46, "period": 47, "tab": 48,
        "space": 49, "grave": 50, "delete": 51, "escape": 53, "left": 123, "right": 124, "down": 125, "up": 126,
    ]
    public static func code(_ key: String) -> UInt16? { letters[key] }
    public static let escape: UInt16 = 53
}

public enum CtrlRequest: Equatable, Sendable {
    case catalog(id: String)
    case tree(id: String, app: String, scope: String?, depth: Int)
    case menuShortcuts(id: String, app: String)
    case act(id: String, runId: String, action: CtrlAction, sig: String?)
    case observe(id: String, runId: String, arm: Bool?)
    case laya(id: String, model: String?, state: String, question: String, options: [String])
    case layaWarm(id: String, model: String)

    public var id: String {
        switch self {
        case let .catalog(id), let .tree(id, _, _, _), let .menuShortcuts(id, _), let .act(id, _, _, _),
             let .observe(id, _, _), let .laya(id, _, _, _, _), let .layaWarm(id, _): return id
        }
    }

    public static func parse(_ line: String) -> CtrlRequest? {
        guard let data = line.data(using: .utf8),
              let o = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let type = o["type"] as? String, let id = o["id"] as? String else { return nil }
        let s = { (k: String) in o[k] as? String }
        switch type {
        case "ctrl.catalog": return .catalog(id: id)
        case "ctrl.tree":
            guard let a = s("app") else { return nil }
            let depth = (o["depth"] as? Int).map { min(max($0, 1), 8) } ?? 6
            return .tree(id: id, app: a, scope: s("scope"), depth: depth)
        case "ctrl.menu.shortcuts": return s("app").map { .menuShortcuts(id: id, app: $0) }
        case "ctrl.ax.act":
            guard let r = s("runId"), let a = (o["action"] as? [String: Any]).flatMap(CtrlAction.parse) else { return nil }
            return .act(id: id, runId: r, action: a, sig: s("sig"))
        case "ctrl.observe":
            guard let r = s("runId") else { return nil }
            return .observe(id: id, runId: r, arm: o["arm"] as? Bool)
        case "ctrl.laya":
            guard let st = s("state"), let q = s("question"), let opts = o["options"] as? [String] else { return nil }
            return .laya(id: id, model: s("model"), state: st, question: q, options: opts)
        case "ctrl.laya.warm": return s("model").map { .layaWarm(id: id, model: $0) }
        default: return nil
        }
    }
}

public struct CtrlCatalogApp: Equatable, Sendable {
    public let bundleId: String, name: String, browser: Bool, running: Bool
    public init(bundleId: String, name: String, browser: Bool, running: Bool) {
        self.bundleId = bundleId; self.name = name; self.browser = browser; self.running = running
    }
}

public struct CtrlMenuShortcut: Equatable, Sendable {
    public let id: String, path: [String], combo: String, enabled: Bool
    public init(id: String, path: [String], combo: String, enabled: Bool = true) {
        self.id = id; self.path = path; self.combo = combo; self.enabled = enabled
    }
}

/// What `ctrl.observe` reports: ids and flags only.
public struct CtrlObserved: Equatable, Sendable {
    public var frontmost: String?, focused: String?
    public var stopped = false, userInput = false, armed = false
    /// Review (d): why the watcher isn't armed: `accessibility`, `input_monitoring` or `secure_input`.
    public var missing: String?
    public init(frontmost: String? = nil, focused: String? = nil, stopped: Bool = false, userInput: Bool = false, armed: Bool = false,
                missing: String? = nil) {
        self.frontmost = frontmost; self.focused = focused; self.stopped = stopped; self.userInput = userInput; self.armed = armed
        self.missing = missing
    }
}

public struct CtrlFailure: Error, Equatable { public let code: String; public init(_ c: String) { code = c } }

public protocol CtrlBackend: AnyObject {
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?)
    func tree(app: String, scope: String?, depth: Int) throws -> (root: CtrlTreeNode, focused: String?)
    func menuShortcuts(app: String) throws -> [CtrlMenuShortcut]
    /// After `CtrlDoPolicy.check`; `sig` is the fingerprint the host saw (`act.stale` when the element changed).
    func act(_ action: CtrlAction, sig: String?) throws
    func observe(arm: Bool?) -> CtrlObserved
    /// The stop watcher is armed and nothing tripped it (checked before every act, review C2).
    func mayAct() -> Bool
    func laya(model: String?, state: String, question: String, options: [String]) throws -> [Double]
    func warmLaya(model: String) throws -> CtrlLayaWarmer.State
}

public extension CtrlBackend {
    func warmLaya(model: String) throws -> CtrlLayaWarmer.State { throw CtrlFailure("laya.no_runtime") }
}

public final class CtrlServer {
    let backend: CtrlBackend
    public init(backend: CtrlBackend) { self.backend = backend }

    func reply(_ req: CtrlRequest) -> [String: Any] {
        let id = req.id
        do {
            switch req {
            case .catalog:
                let c = try backend.catalog()
                return ["type": "ctrl.catalog.result", "id": id, "defaultBrowser": c.defaultBrowser as Any? ?? NSNull(),
                        "apps": c.apps.map { ["bundleId": $0.bundleId, "name": $0.name, "browser": $0.browser, "running": $0.running] }]
            case let .tree(_, app, scope, depth):
                let t = try backend.tree(app: app, scope: scope, depth: depth)
                return ["type": "ctrl.tree.result", "id": id, "root": t.root.json, "focused": t.focused as Any? ?? NSNull()]
            case let .menuShortcuts(_, app):
                let items = try backend.menuShortcuts(app: app)
                return ["type": "ctrl.menu.shortcuts.result", "id": id,
                        "items": items.map { ["id": $0.id, "path": $0.path, "combo": $0.combo, "enabled": $0.enabled] }]
            case let .act(_, _, action, sig):
                // Review C2: the watcher is checked first; a stop (or no watcher) refuses with `guard.stopped`.
                guard backend.mayAct() else {
                    return ["type": "ctrl.ax.act.result", "id": id, "ok": false, "err": "guard.stopped"]
                }
                do {
                    try CtrlDoPolicy.check(action)
                    try backend.act(action, sig: sig)
                    return ["type": "ctrl.ax.act.result", "id": id, "ok": true, "err": NSNull()]
                } catch let f as CtrlFailure {
                    return ["type": "ctrl.ax.act.result", "id": id, "ok": false, "err": f.code]
                }
            case let .observe(_, _, arm):
                let o = backend.observe(arm: arm)
                var r: [String: Any] = ["type": "ctrl.observe.result", "id": id, "stopped": o.stopped,
                                        "userInput": o.userInput, "armed": o.armed]
                if let f = o.frontmost { r["frontmost"] = f }
                if let f = o.focused { r["focused"] = f }
                if !o.armed, let m = o.missing { r["missing"] = m }
                return r
            case let .laya(_, model, state, question, options):
                return ["type": "ctrl.laya.result", "id": id, "p": try backend.laya(model: model, state: state, question: question, options: options)]
            case let .layaWarm(_, model):
                return ["type": "ctrl.laya.warm.result", "id": id, "state": try backend.warmLaya(model: model).rawValue]
            }
        } catch let f as CtrlFailure {
            return ["type": "ctrl.error", "id": id, "code": f.code]
        } catch {
            return ["type": "ctrl.error", "id": id, "code": "helper.internal"]
        }
    }

    public func handle(line: String) -> String {
        let o: [String: Any]
        if let req = CtrlRequest.parse(line) {
            o = reply(req)
        } else {
            let id = (line.data(using: .utf8).flatMap { try? JSONSerialization.jsonObject(with: $0) } as? [String: Any])?["id"] as? String
            o = ["type": "ctrl.error", "id": id ?? "", "code": "helper.bad_request"]
        }
        let data = (try? JSONSerialization.data(withJSONObject: o, options: [.sortedKeys])) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }
}
