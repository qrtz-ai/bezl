// The ctrl helper protocol, helper side (slice 5, ADR 0108). Mirrors crates/qrtz-ctrl/src/helper.rs (the source of
// truth); both sides test against Fixtures/ctrl/protocol.jsonl. One request line in, one reply line out, all under
// the `ctrl.` prefix, so it rides next to the echo/nano messages without touching them.
//
// Foundation-only: parsing, the policy re-check and the dispatch build and test on Linux. The macOS backend (the
// LaunchServices catalog, CtrlProbe, AX snapshots, CtrlActions) implements `CtrlBackend` and is wired into the
// helper's stdio in a later slice. The server re-checks every route against CtrlPolicy before the backend sees it
// (ADR 0100 §6): a route the host should never have sent is refused here too.
import Foundation

/// A ladder route as the host sends it (`{"route": "scripting", "app": …, "verb": …}`).
public enum CtrlRoute: Equatable, Sendable {
    case files(op: String)
    case openURL(app: String, urlRef: String)
    case launchHidden(app: String)
    /// ADR 0130: terminate each running app (no activation). Only behind the host's approved confirm gate.
    case quitApps(apps: [String])
    case scripting(app: String, verb: String)
    case axPerform(app: String, role: String, element: String, action: String)
    case axSet(app: String, role: String, element: String, attribute: String)
    case pidKey(app: String, combo: String)
    /// Click, global key or activate: named so a planner can say it, always refused.
    case takesOver(String)

    /// The only Apple Event verbs (Rust `ScriptVerb`): none of them can activate, launch, open or reopen.
    /// These have a fixed script (`CtrlScripts.source`).
    public static let scriptVerbs: Set<String> = ["count_tabs", "close_all_tabs", "close_tab", "count_windows", "close_window"]
    /// Slice 7b's selective closes (ADR 0114 / 0119) have no fixed script: the backend reads the tabs first
    /// (`CtrlTabs`) and closes only the ones it can tell about; a backend that can't tell refuses, and the host asks.
    public static let tabVerbs: Set<String> = ["close_saved_tabs", "close_unpinned_tabs"]

    static func parse(_ o: [String: Any]) -> CtrlRoute? {
        let s = { (k: String) in o[k] as? String }
        switch s("route") {
        case "files": return s("op").map { .files(op: $0) }
        case "workspace":
            switch s("op") {
            case "open_url": if let a = s("app"), let u = s("url") { return .openURL(app: a, urlRef: u) }
            case "launch_hidden": if let a = s("app") { return .launchHidden(app: a) }
            case "quit_apps": if let a = o["apps"] as? [String] { return .quitApps(apps: a) }
            default: return nil
            }
            return nil
        case "scripting": if let a = s("app"), let v = s("verb") { return .scripting(app: a, verb: v) }
        case "ax":
            guard let a = s("app"), let r = s("role"), let e = s("element"), let op = o["op"] as? [String: Any] else { return nil }
            switch op["op"] as? String {
            case "perform": if let x = op["action"] as? String { return .axPerform(app: a, role: r, element: e, action: x) }
            case "set_attribute": if let x = op["attribute"] as? String { return .axSet(app: a, role: r, element: e, attribute: x) }
            default: return nil
            }
        case "pid_key": if let a = s("app"), let c = s("combo") { return .pidKey(app: a, combo: c) }
        case "click", "global_key", "activate": return .takesOver(s("route")!)
        default: return nil
        }
        return nil
    }
}

public enum CtrlRequest: Equatable, Sendable {
    case catalog(id: String)
    case probe(id: String)
    case snapshot(id: String, app: String, scope: String?)
    case act(id: String, actionId: String, route: CtrlRoute, content: [String: String])
    /// `check`: tab_count / window_count / app_running / ax_scope, for `app`.
    case measure(id: String, check: String, app: String)
    case restore(id: String, frontmostPid: Int32?)
    /// `model`: the consented, installed model the host hands over (ADR 0116), nil when it sent none.
    case laya(id: String, model: String?, state: String, question: String, options: [String])
    /// The confirm gate's preview (ADR 0111): `what` is `windows` or `tabs` of `app`.
    case preview(id: String, app: String, what: String)
    /// ADR 0130: the running apps with the last-used record (`CtrlLastUsed`).
    case apps(id: String)

    public var id: String {
        switch self {
        case let .catalog(id), let .probe(id), let .snapshot(id, _, _), let .act(id, _, _, _), let .measure(id, _, _),
             let .restore(id, _), let .laya(id, _, _, _, _), let .preview(id, _, _), let .apps(id): return id
        }
    }

    public static func parse(_ line: String) -> CtrlRequest? {
        guard let data = line.data(using: .utf8),
              let o = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let type = o["type"] as? String, let id = o["id"] as? String else { return nil }
        let s = { (k: String) in o[k] as? String }
        switch type {
        case "ctrl.catalog": return .catalog(id: id)
        case "ctrl.probe": return .probe(id: id)
        case "ctrl.apps": return .apps(id: id)
        case "ctrl.snapshot": return s("app").map { .snapshot(id: id, app: $0, scope: s("scope")) }
        case "ctrl.act":
            guard let a = s("actionId"), let r = (o["route"] as? [String: Any]).flatMap(CtrlRoute.parse),
                  let c = o["content"] as? [String: String] else { return nil }
            return .act(id: id, actionId: a, route: r, content: c)
        case "ctrl.measure":
            guard let c = o["check"] as? [String: Any], let k = c["check"] as? String, let a = c["app"] as? String else { return nil }
            return .measure(id: id, check: k, app: a)
        case "ctrl.restore": return .restore(id: id, frontmostPid: (o["frontmostPid"] as? NSNumber).map { $0.int32Value })
        case "ctrl.laya":
            guard let st = s("state"), let q = s("question"), let opts = o["options"] as? [String] else { return nil }
            return .laya(id: id, model: s("model"), state: st, question: q, options: opts)
        case "ctrl.preview":
            guard let a = s("app"), let w = s("what"), w == "windows" || w == "tabs" else { return nil }
            return .preview(id: id, app: a, what: w)
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

/// One AX element of a snapshot. `title` goes to the host, which hashes it before anything is logged.
public struct CtrlAXNode: Equatable, Sendable {
    public let id: String, role: String, subrole: String?, title: String, actions: [String], children: Int
    public init(id: String, role: String, subrole: String?, title: String, actions: [String], children: Int) {
        self.id = id; self.role = role; self.subrole = subrole; self.title = title; self.actions = actions; self.children = children
    }
}

/// A short code, never content (`ax.not_trusted`, `app.not_running`, `laya.no_model`).
public struct CtrlFailure: Error, Equatable { public let code: String; public init(_ c: String) { code = c } }

/// One item of a confirm-gate preview. `title` goes to the host, which hashes it before anything is logged;
/// `unsaved: nil` = can't tell (the host counts it as unsaved).
/// `pinned` (tabs only, slice 7b, ADR 0114): a pinned tab is never closed by "close all my tabs"; nil = can't tell
/// (the host then holds the tab for the confirm gate) and is left off the wire.
public struct CtrlPreviewItem: Equatable, Sendable {
    public let title: String, unsaved: Bool?, pinned: Bool?
    public init(title: String, unsaved: Bool?, pinned: Bool? = nil) {
        self.title = title; self.unsaved = unsaved; self.pinned = pinned
    }
}

/// One running app for "quit unused apps" (ADR 0130). `name` labels the gate's card (the host hashes it before the
/// log); `lastActiveMs` is the last activation the helper saw, nil if none since it began watching; `unsaved` is
/// false only when the app has no windows, nil when the helper can't tell.
public struct CtrlRunningApp: Equatable, Sendable {
    public let bundleId: String, name: String, lastActiveMs: UInt64?, frontmost: Bool, unsaved: Bool?
    public init(bundleId: String, name: String, lastActiveMs: UInt64?, frontmost: Bool, unsaved: Bool?) {
        self.bundleId = bundleId; self.name = name; self.lastActiveMs = lastActiveMs; self.frontmost = frontmost; self.unsaved = unsaved
    }
}

/// What the macOS side implements (and the tests fake).
public protocol CtrlBackend: AnyObject {
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?)
    /// The ladder's `Env` and the guard's `UiSnapshot`, as the Rust shapes' JSON objects.
    func probe() throws -> (env: [String: Any], ui: [String: Any])
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode]
    func act(_ route: CtrlRoute, content: [String: String]) throws
    func measure(check: String, app: String) -> Int?
    func restore(frontmostPid: Int32?) -> Bool
    func laya(state: String, question: String, options: [String]) throws -> [Double]
    /// The same pass on the model at `model` (ADR 0116); defaults to the model-less call.
    func laya(model: String?, state: String, question: String, options: [String]) throws -> [Double]
    func frontmostPid() -> Int32?
    func pid(of app: String) -> Int32?
    /// The windows or tabs an irreversible action would touch (read-only, never prompts).
    func preview(app: String, what: String) throws -> [CtrlPreviewItem]
    /// ADR 0130: running regular apps, when the last-used watch began (nil: not watching) and local midnight.
    func runningApps() throws -> (apps: [CtrlRunningApp], sinceMs: UInt64?, dayStartMs: UInt64)
}

public extension CtrlBackend {
    func laya(model: String?, state: String, question: String, options: [String]) throws -> [Double] {
        try laya(state: state, question: question, options: options)
    }
    /// Until a backend enumerates them, the host falls back to a count (`ctrl.measure`) for the preview.
    func preview(app: String, what: String) throws -> [CtrlPreviewItem] { throw CtrlFailure("preview.unsupported") }
    /// A backend without the activation watch: the host then quits nothing.
    func runningApps() throws -> (apps: [CtrlRunningApp], sinceMs: UInt64?, dayStartMs: UInt64) { throw CtrlFailure("apps.not_watching") }
}

public final class CtrlServer {
    let backend: CtrlBackend
    public init(backend: CtrlBackend) { self.backend = backend }

    /// ADR 0100 §6: the same refusals as the Rust ladder, checked again before the backend acts.
    public static func recheck(_ route: CtrlRoute, frontmostPid: Int32?, targetPid: Int32?) throws {
        switch route {
        case .takesOver: throw CtrlFailure("policy.refused")
        case let .scripting(_, verb):
            if !CtrlRoute.scriptVerbs.contains(verb) && !CtrlRoute.tabVerbs.contains(verb) { throw CtrlFailure("policy.refused") }
        case let .axPerform(_, role, _, action):
            do { try CtrlPolicy.checkAXAction(action, role: role) } catch { throw CtrlFailure("policy.refused") }
        case let .axSet(_, role, _, attribute):
            do { try CtrlPolicy.checkAXSet(attribute, role: role) } catch { throw CtrlFailure("policy.refused") }
        case let .pidKey(_, combo):
            guard let t = targetPid else { throw CtrlFailure("app.not_running") }
            do { try CtrlPolicy.checkKey(combo, targetPid: t, frontmostPid: frontmostPid) } catch { throw CtrlFailure("policy.refused") }
        case .files, .openURL, .launchHidden, .quitApps: break
        }
    }

    static func appOf(_ r: CtrlRoute) -> String? {
        switch r {
        case let .openURL(a, _), let .launchHidden(a), let .scripting(a, _), let .axPerform(a, _, _, _), let .axSet(a, _, _, _),
             let .pidKey(a, _): return a
        case .files, .takesOver, .quitApps: return nil
        }
    }

    func reply(_ req: CtrlRequest) -> [String: Any] {
        let id = req.id
        do {
            switch req {
            case .catalog:
                let c = try backend.catalog()
                return ["type": "ctrl.catalog.result", "id": id, "defaultBrowser": c.defaultBrowser as Any? ?? NSNull(),
                        "apps": c.apps.map { ["bundleId": $0.bundleId, "name": $0.name, "browser": $0.browser, "running": $0.running] }]
            case .probe:
                let p = try backend.probe()
                return ["type": "ctrl.probe.result", "id": id, "env": p.env, "ui": p.ui]
            case let .snapshot(_, app, scope):
                let nodes = try backend.snapshot(app: app, scope: scope)
                return ["type": "ctrl.snapshot.result", "id": id, "nodes": nodes.map { n -> [String: Any] in
                    var o: [String: Any] = ["id": n.id, "role": n.role, "title": n.title, "actions": n.actions, "children": n.children]
                    if let s = n.subrole { o["subrole"] = s }
                    return o
                }]
            case let .act(_, _, route, content):
                try Self.recheck(route, frontmostPid: backend.frontmostPid(), targetPid: Self.appOf(route).flatMap(backend.pid(of:)))
                // ADR 0130: the app the user is in is never quit under them.
                if case let .quitApps(apps) = route, let front = backend.frontmostPid(),
                   apps.contains(where: { backend.pid(of: $0) == front }) {
                    throw CtrlFailure("policy.refused")
                }
                do {
                    try backend.act(route, content: content)
                    return ["type": "ctrl.act.result", "id": id, "ok": true, "err": NSNull()]
                } catch let f as CtrlFailure {
                    return ["type": "ctrl.act.result", "id": id, "ok": false, "err": f.code]
                }
            case let .measure(_, check, app):
                return ["type": "ctrl.measure.result", "id": id, "value": backend.measure(check: check, app: app) as Any? ?? NSNull()]
            case let .restore(_, pid):
                return ["type": "ctrl.restore.result", "id": id, "ok": backend.restore(frontmostPid: pid)]
            case let .laya(_, model, state, question, options):
                return ["type": "ctrl.laya.result", "id": id, "p": try backend.laya(model: model, state: state, question: question, options: options)]
            case .apps:
                let r = try backend.runningApps()
                return ["type": "ctrl.apps.result", "id": id, "sinceMs": r.sinceMs.map { NSNumber(value: $0) } as Any? ?? NSNull(),
                        "dayStartMs": NSNumber(value: r.dayStartMs),
                        "apps": r.apps.map { a -> [String: Any] in
                            ["bundleId": a.bundleId, "name": a.name, "frontmost": a.frontmost,
                             "lastActiveMs": a.lastActiveMs.map { NSNumber(value: $0) } as Any? ?? NSNull(),
                             "unsaved": a.unsaved as Any? ?? NSNull()]
                        }]
            case let .preview(_, app, what):
                let items = try backend.preview(app: app, what: what)
                return ["type": "ctrl.preview.result", "id": id, "items": items.map { i -> [String: Any] in
                    var o: [String: Any] = ["title": i.title, "unsaved": i.unsaved as Any? ?? NSNull()]
                    if let p = i.pinned { o["pinned"] = p }
                    return o
                }]
            }
        } catch let f as CtrlFailure {
            return ["type": "ctrl.error", "id": id, "code": f.code]
        } catch {
            return ["type": "ctrl.error", "id": id, "code": "helper.internal"]
        }
    }

    /// One request line → one reply line (sorted keys). A line that isn't a ctrl request gets `ctrl.error`.
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
