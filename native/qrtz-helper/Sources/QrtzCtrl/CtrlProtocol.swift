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
    /// ADR 0134: hide each running app (no activation, nothing raised). Reversible, so no gate; never the frontmost.
    case hideApps(apps: [String])
    /// ADR 0152: unhide each running app (no activation: the app the user is in stays frontmost). Reversible, no gate.
    case unhideApps(apps: [String])
    /// ADR 0135: mute the sound output (in the helper's own process; no app, nothing shown). Reversible, no gate.
    case muteOutput
    /// ADR 0139: the reverse (same process, same rules).
    case unmuteOutput
    /// ADR 0140: set the output volume to `level` percent (0–100) and unmute (same process, same rules).
    case setVolume(level: Int)
    /// ADR 0158: run the one "qrtz Set Focus" shortcut through Shortcuts Events with `mode` as its input. A mode off
    /// the enum is a bad request (never reaches the backend). Reversible, no gate.
    case setFocus(mode: CtrlFocusMode)
    /// ADR 0166: show one of the user's folders (a closed set, resolved here from the system's own location) in the
    /// running Finder with `activates = false`. No path ever comes over the wire. Reversible, no gate.
    case openFolder(folder: CtrlFolder)
    /// ADR 0168: empty the general pasteboard in the helper's own process (no app, nothing shown, nothing read from
    /// the clipboard). Not undoable; no gate (the task asked for exactly this); the host defers it while the user is
    /// typing or clicking.
    case clearClipboard
    /// ADR 0176: unmount and eject the volumes the helper lists as ejectable (never forced; nothing about them comes
    /// over the wire). `single`: refuse unless exactly one would go. No app, no gate.
    case ejectVolumes(single: Bool)
    /// ADR 0180: put the displays to sleep now, in the helper's own process (IOKit's display wrangler, what `pmset
    /// displaysleepnow` does, without spawning it). The Mac stays awake, nothing is locked. No app, no content, no gate.
    case sleepDisplay
    /// ADR 0183: put the whole Mac to sleep now, in the helper's own process (IOKit's `IOPMSleepSystem`, the request
    /// `pmset sleepnow` makes, without spawning it). Nothing quit or closed, no key event. No app, no content, no gate
    /// (the host defers it while the user is typing or clicking).
    case sleepSystem
    /// ADR 0191: turn the Wi-Fi radio on or off, in the helper's own process (CoreWLAN `setPower` on the default
    /// interface, the menu bar switch). No network is joined, left or scanned and nothing about one comes over the
    /// wire: only `on`. No app, no content, no gate (the host defers an off while the user is typing or clicking).
    case setWifi(on: Bool)
    case scripting(app: String, verb: String)
    case axPerform(app: String, role: String, element: String, action: String)
    case axSet(app: String, role: String, element: String, attribute: String)
    case pidKey(app: String, combo: String)
    /// Click, global key or activate: named so a planner can say it, always refused.
    case takesOver(String)

    /// The only Apple Event verbs (Rust `ScriptVerb`): none of them can activate, launch, open or reopen.
    /// These have a fixed script (`CtrlScripts.source`).
    public static let scriptVerbs: Set<String> = ["count_tabs", "close_all_tabs", "close_tab", "count_windows", "close_window",
                                                  "set_dark_mode", "set_light_mode", "set_dock_autohide", "set_dock_shown",
                                                  "start_screen_saver", "stop_screen_saver"]
    /// ADR 0137: the appearance verbs, only ever to System Events (a faceless agent, never brought forward).
    public static let appearanceVerbs: Set<String> = ["set_dark_mode", "set_light_mode"]
    /// ADR 0173: the Dock's auto-hide verbs, likewise only ever to System Events.
    public static let dockVerbs: Set<String> = ["set_dock_autohide", "set_dock_shown"]
    /// ADR 0178: start the current screen saver (amendment 1: or stop it), likewise only ever to System Events.
    public static let screensaverVerbs: Set<String> = ["start_screen_saver", "stop_screen_saver"]
    /// The verbs that go to System Events, and the only ones that may (ADR 0137, ADR 0173, ADR 0178).
    public static let systemEventsVerbs: Set<String> = appearanceVerbs.union(dockVerbs).union(screensaverVerbs)
    public static let systemEvents = "com.apple.systemevents"
    /// ADR 0135 / 0137 / 0140: the measures that read the Mac itself, not an app.
    /// ADR 0158: `focus_on` too (the Mac backend answers nil: reading a Focus needs its own prompt).
    /// ADR 0168: `clipboard_cleared` too (the pasteboard's change count, never its contents).
    /// ADR 0173: `dock_autohide` too (the Dock's own defaults, read in this process).
    /// ADR 0178: `screensaver_running` too (ScreenSaverEngine in the process list, read in this process).
    /// ADR 0180: `display_asleep` too (the main display's sleep state, read in this process).
    /// ADR 0183: `last_sleep` too (when the Mac last slept, `kern.sleeptime`, read in this process).
    /// ADR 0191: `wifi_on` too (the Wi-Fi radio's power, read in this process; never a network's name).
    public static let applessChecks: Set<String> = ["output_muted", "dark_mode", "output_volume", "focus_on", CtrlClipboard.check,
                                                    CtrlDock.check, CtrlVolumes.check, CtrlScreensaver.check, CtrlDisplay.check,
                                                    CtrlSystemSleep.check, CtrlWifi.check]
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
            case "hide_apps": if let a = o["apps"] as? [String] { return .hideApps(apps: a) }
            case "unhide_apps": if let a = o["apps"] as? [String] { return .unhideApps(apps: a) }
            case "mute_output": return .muteOutput
            case "unmute_output": return .unmuteOutput
            case "clear_clipboard": return .clearClipboard
            // ADR 0180: the bare op; nothing else is read from the request.
            case CtrlDisplay.op: return .sleepDisplay
            // ADR 0183: the bare op likewise.
            case CtrlSystemSleep.op: return .sleepSystem
            // ADR 0191: `on` must be a JSON true/false (objCType "c", ADR 0141), and nothing else is read from the
            // request (a network name riding along is ignored, never used).
            case CtrlWifi.op:
                guard let n = o["on"] as? NSNumber, String(cString: n.objCType) == "c", let b = o["on"] as? Bool
                else { return nil }
                return .setWifi(on: b)
            // ADR 0176: `single` must be a JSON true/false (objCType "c", ADR 0141); a number, a string or no field is
            // a bad request.
            case "eject_volumes":
                guard let n = o["single"] as? NSNumber, String(cString: n.objCType) == "c", let b = o["single"] as? Bool
                else { return nil }
                return .ejectVolumes(single: b)
            // ADR 0140: a whole number 0–100 only; anything else is a bad request, never clamped.
            case "set_volume":
                // ADR 0141: `is Bool` is true for a JSON 0 or 1 too (NSNumber bridging, Linux and macOS), so a JSON
                // true/false is told apart by its objCType "c" instead; 0 and 1 are levels, true and false are not.
                guard let n = o["level"] as? NSNumber, String(cString: n.objCType) != "c", let l = o["level"] as? Int,
                      (0...100).contains(l) else { return nil }
                return .setVolume(level: l)
            // ADR 0158 amendment 3: the strict enum, checked before any event is built.
            case "set_focus": return s("mode").flatMap(CtrlFocus.mode).map { .setFocus(mode: $0) }
            // ADR 0166: the strict enum; a path, another spelling or a missing folder is a bad request.
            case "open_folder": return s("folder").flatMap(CtrlFolders.folder).map { .openFolder(folder: $0) }
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
    /// ADR 0133: warm the consented, installed model at `model` in the background; the reply comes at once.
    case layaWarm(id: String, model: String)
    /// The confirm gate's preview (ADR 0111): `what` is `windows` or `tabs` of `app`.
    case preview(id: String, app: String, what: String)
    /// ADR 0130: the running apps with the last-used record (`CtrlLastUsed`).
    case apps(id: String)

    public var id: String {
        switch self {
        case let .catalog(id), let .probe(id), let .snapshot(id, _, _), let .act(id, _, _, _), let .measure(id, _, _),
             let .restore(id, _), let .laya(id, _, _, _, _), let .preview(id, _, _), let .apps(id),
             let .layaWarm(id, _): return id
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
            // ADR 0135 / 0137 / 0140: `output_muted`, `dark_mode` and `output_volume` have no app.
            // ADR 0166: `folder_open` names a folder of the closed set instead; its app is always the Finder.
            if let c = o["check"] as? [String: Any], c["check"] as? String == "folder_open" {
                guard let f = (c["folder"] as? String).flatMap(CtrlFolders.folder) else { return nil }
                return .measure(id: id, check: CtrlFolders.measurePrefix + f.rawValue, app: CtrlFolders.finder)
            }
            guard let c = o["check"] as? [String: Any], let k = c["check"] as? String,
                  let a = (c["app"] as? String) ?? (CtrlRoute.applessChecks.contains(k) ? "" : nil) else { return nil }
            return .measure(id: id, check: k, app: a)
        case "ctrl.restore": return .restore(id: id, frontmostPid: (o["frontmostPid"] as? NSNumber).map { $0.int32Value })
        case "ctrl.laya":
            guard let st = s("state"), let q = s("question"), let opts = o["options"] as? [String] else { return nil }
            return .laya(id: id, model: s("model"), state: st, question: q, options: opts)
        case "ctrl.laya.warm": return s("model").map { .layaWarm(id: id, model: $0) }
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
    /// ADR 0134: `isHidden`; nil leaves the field off the wire (the host then can't verify a hide).
    public let hidden: Bool?
    public init(bundleId: String, name: String, lastActiveMs: UInt64?, frontmost: Bool, unsaved: Bool?, hidden: Bool? = nil) {
        self.bundleId = bundleId; self.name = name; self.lastActiveMs = lastActiveMs; self.frontmost = frontmost; self.unsaved = unsaved
        self.hidden = hidden
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
    /// ADR 0133: start warming the model at `model` off the request queue and return at once.
    func warmLaya(model: String) throws -> CtrlLayaWarmer.State
}

public extension CtrlBackend {
    func laya(model: String?, state: String, question: String, options: [String]) throws -> [Double] {
        try laya(state: state, question: question, options: options)
    }
    /// Until a backend enumerates them, the host falls back to a count (`ctrl.measure`) for the preview.
    func preview(app: String, what: String) throws -> [CtrlPreviewItem] { throw CtrlFailure("preview.unsupported") }
    /// A backend without the activation watch: the host then quits nothing.
    func runningApps() throws -> (apps: [CtrlRunningApp], sinceMs: UInt64?, dayStartMs: UInt64) { throw CtrlFailure("apps.not_watching") }
    /// A backend without a Laya runtime has nothing to warm.
    func warmLaya(model: String) throws -> CtrlLayaWarmer.State { throw CtrlFailure("laya.no_runtime") }
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
        case .files, .openURL, .launchHidden, .quitApps, .hideApps, .unhideApps, .muteOutput, .unmuteOutput, .setVolume,
             .setFocus, .openFolder, .clearClipboard, .ejectVolumes, .sleepDisplay, .sleepSystem, .setWifi: break
        }
    }

    static func appOf(_ r: CtrlRoute) -> String? {
        switch r {
        case let .openURL(a, _), let .launchHidden(a), let .scripting(a, _), let .axPerform(a, _, _, _), let .axSet(a, _, _, _),
             let .pidKey(a, _): return a
        case .openFolder: return CtrlFolders.finder
        case .files, .takesOver, .quitApps, .hideApps, .unhideApps, .muteOutput, .unmuteOutput, .setVolume, .setFocus,
             .clearClipboard, .ejectVolumes, .sleepDisplay, .sleepSystem, .setWifi:
            return nil
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
                // ADR 0134: the app the user is in is never hidden under them; it drops out of the list (the user may
                // have switched to it since the host planned), the rest are hidden.
                var route = route
                if case let .hideApps(apps) = route, let front = backend.frontmostPid() {
                    route = .hideApps(apps: apps.filter { backend.pid(of: $0) != front })
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
            case let .layaWarm(_, model):
                return ["type": "ctrl.laya.warm.result", "id": id, "state": try backend.warmLaya(model: model).rawValue]
            case .apps:
                let r = try backend.runningApps()
                return ["type": "ctrl.apps.result", "id": id, "sinceMs": r.sinceMs.map { NSNumber(value: $0) } as Any? ?? NSNull(),
                        "dayStartMs": NSNumber(value: r.dayStartMs),
                        "apps": r.apps.map { a -> [String: Any] in
                            var o: [String: Any] = ["bundleId": a.bundleId, "name": a.name, "frontmost": a.frontmost,
                                                    "lastActiveMs": a.lastActiveMs.map { NSNumber(value: $0) } as Any? ?? NSNull(),
                                                    "unsaved": a.unsaved as Any? ?? NSNull()]
                            if let h = a.hidden { o["hidden"] = h }
                            return o
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
