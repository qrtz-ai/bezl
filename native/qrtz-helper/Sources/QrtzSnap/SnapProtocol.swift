// qrtz snap's wire (ADR 0159): newline-delimited JSON on stdio with the host, like ctrl's (ADR 0110).
//   host → snap: {"type":"snap.configure","shortcuts":{"leftHalf":"alt+cmd+left",…},"disabledApps":[…],"statusItem":true}
//                {"type":"snap.act","action":"leftHalf"}      (tests and the status menu's own path)
//                {"type":"shutdown"}
//   snap → host: {"type":"hello","features":["module:snap"],…}   first line, so the supervisor counts it running
//                {"type":"snap.status","axTrusted":…,"registered":{action: binding},"refused":{action: reason}}
//                {"type":"snap.disable-app","bundleId":"…","disabled":true}   the status menu's "disable for <app>"
//                {"type":"snap.moved","action":"…"} / {"type":"snap.failed","action":"…","reason":"…"}
// Foundation-only; window titles and app content never leave the helper (only bundle ids the user chose to disable).
import Foundation

public struct SnapConfig: Equatable {
    public var overrides: [String: String] = [:]
    public var disabledApps: Set<String> = []
    public var statusItem = true
    public init() {}

    /// Parses a `snap.configure` line's fields; anything missing keeps its default.
    public init(json o: [String: Any]) {
        if let s = o["shortcuts"] as? [String: Any] {
            for (k, v) in s { if let b = v as? String { overrides[k] = b } }
        }
        if let d = o["disabledApps"] as? [Any] { disabledApps = Set(d.compactMap { $0 as? String }) }
        if let b = o["statusItem"] as? Bool { statusItem = b }
    }
}

public enum SnapRequest: Equatable {
    case configure(SnapConfig)
    case act(SnapAction)
    case shutdown
    case ignored

    public static func parse(_ line: String) -> SnapRequest {
        guard let d = line.data(using: .utf8),
              let o = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any],
              let t = o["type"] as? String else { return .ignored }
        switch t {
        case "snap.configure": return .configure(SnapConfig(json: o))
        case "snap.act":
            guard let a = (o["action"] as? String).flatMap(SnapAction.init(rawValue:)) else { return .ignored }
            return .act(a)
        case "shutdown": return .shutdown
        default: return .ignored
        }
    }
}

public enum SnapLines {
    static func encode(_ o: [String: Any]) -> String {
        guard let d = try? JSONSerialization.data(withJSONObject: o, options: [.sortedKeys]),
              let s = String(data: d, encoding: .utf8) else { return #"{"type":"snap.error"}"# }
        return s
    }

    public static func hello(version: String) -> String {
        encode(["type": "hello", "protocol": 1, "helperVersion": version, "features": ["module:snap"]])
    }

    public static func status(axTrusted: Bool, registered: [SnapAction: SnapShortcut], refused: [SnapAction: String]) -> String {
        var r: [String: String] = [:]
        for (a, s) in registered { r[a.rawValue] = s.description }
        var f: [String: String] = [:]
        for (a, why) in refused { f[a.rawValue] = why }
        return encode(["type": "snap.status", "axTrusted": axTrusted, "registered": r, "refused": f])
    }

    public static func disableApp(_ bundleId: String, disabled: Bool) -> String {
        encode(["type": "snap.disable-app", "bundleId": bundleId, "disabled": disabled])
    }

    public static func outcome(_ action: SnapAction, _ o: SnapOutcome) -> String {
        switch o {
        case .moved: return encode(["type": "snap.moved", "action": action.rawValue])
        case .failed(let why): return encode(["type": "snap.failed", "action": action.rawValue, "reason": why])
        }
    }
}

/// What the helper's hotkeys and menu drive. The macOS one registers Carbon hotkeys and moves windows through AX.
public protocol SnapBackend: AnyObject {
    var axTrusted: Bool { get }
    /// Register exactly these shortcuts (dropping the old ones). Answers the ones the system refused, with why
    /// (another app holds the keys), which snap reports instead of silently missing.
    func register(_ shortcuts: [SnapAction: SnapShortcut]) -> [SnapAction: String]
    func perform(_ action: SnapAction) -> SnapOutcome
    func setDisabledApps(_ apps: Set<String>)
    func setStatusItem(_ shown: Bool)
}

/// The helper's request loop, platform-free: configure → resolve shortcuts → register → report status.
public final class SnapServer {
    let backend: SnapBackend
    let write: (String) -> Void
    public private(set) var config = SnapConfig()
    /// The last status sent, so a change of the Accessibility grant re-sends it (nil before the first configure).
    var last: (axTrusted: Bool, registered: [SnapAction: SnapShortcut], refused: [SnapAction: String])?

    public init(backend: SnapBackend, write: @escaping (String) -> Void) {
        self.backend = backend
        self.write = write
    }

    /// Returns false on shutdown.
    @discardableResult
    public func handle(_ line: String) -> Bool {
        switch SnapRequest.parse(line) {
        case .configure(let c): configure(c)
        case .act(let a): write(SnapLines.outcome(a, backend.perform(a)))
        case .shutdown: return false
        case .ignored: break
        }
        return true
    }

    public func configure(_ c: SnapConfig) {
        config = c
        backend.setDisabledApps(c.disabledApps)
        backend.setStatusItem(c.statusItem)
        let resolved = SnapShortcuts.resolve(overrides: c.overrides)
        var refused = resolved.refused
        var registered = resolved.shortcuts
        for (a, why) in backend.register(resolved.shortcuts) {
            registered[a] = nil
            refused[a] = why
        }
        last = (backend.axTrusted, registered, refused)
        write(SnapLines.status(axTrusted: backend.axTrusted, registered: registered, refused: refused))
    }

    /// Called every second by the helper: when the user switches qrtz on (or off) in System Settings the status
    /// goes out again, so snap's view drops its prompt without a relaunch. Answers whether a line was sent.
    @discardableResult
    public func refreshTrust() -> Bool {
        guard let l = last else { return false }
        let now = backend.axTrusted
        guard now != l.axTrusted else { return false }
        last = (now, l.registered, l.refused)
        write(SnapLines.status(axTrusted: now, registered: l.registered, refused: l.refused))
        return true
    }
}

/// Off macOS: nothing registers and nothing moves (Linux builds and tests).
public final class UnsupportedSnapBackend: SnapBackend {
    public init() {}
    public var axTrusted: Bool { false }
    public func register(_ shortcuts: [SnapAction: SnapShortcut]) -> [SnapAction: String] {
        shortcuts.mapValues { _ in "not on macos" }
    }
    public func perform(_ action: SnapAction) -> SnapOutcome { .failed("not on macos") }
    public func setDisabledApps(_ apps: Set<String>) {}
    public func setStatusItem(_ shown: Bool) {}
}
