// ctrl UI guard (ADR 0100): what the user owns, snapshotted before and after every ctrl action, and the comparison
// that turns a difference into a violation. Mirrors crates/qrtz-ctrl/src/guard.rs; both test against
// Fixtures/ctrl/guard-cases.json. Foundation-only, so it builds and tests on Linux.
import Foundation

public struct CtrlPoint: Codable, Equatable, Sendable {
    public var x: Double
    public var y: Double
    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }
}

/// Ids and coordinates only: never a label, title or value (ADR 0083). nil = the probe couldn't read the field.
public struct UiSnapshot: Codable, Equatable, Sendable {
    public var frontmostPid: Int32?
    public var cursor: CtrlPoint?
    /// Opaque id of the system-wide focused element ("e:<pid>:<hash>"). Needs Accessibility trust.
    public var focusedElement: String?
    /// Window number of the front-most normal-layer window on screen.
    public var keyWindow: UInt32?
    /// Milliseconds since the last physical (HID) input event.
    public var hidIdleMs: UInt64?

    public init(frontmostPid: Int32? = nil, cursor: CtrlPoint? = nil, focusedElement: String? = nil, keyWindow: UInt32? = nil, hidIdleMs: UInt64? = nil) {
        self.frontmostPid = frontmostPid
        self.cursor = cursor
        self.focusedElement = focusedElement
        self.keyWindow = keyWindow
        self.hidIdleMs = hidIdleMs
    }

    enum CodingKeys: String, CodingKey {
        case frontmostPid = "frontmost_pid"
        case cursor
        case focusedElement = "focused_element"
        case keyWindow = "key_window"
        case hidIdleMs = "hid_idle_ms"
    }
}

public enum GuardField: String, Codable, CaseIterable, Sendable {
    case frontmostApp = "frontmost_app"
    case cursor
    case focusedElement = "focused_element"
    case keyWindow = "key_window"
}

public struct GuardOutcome: Codable, Equatable, Sendable {
    public var violations: [GuardField] = []
    public var inconclusive: [GuardField] = []
    public var userInput = false
    public var clean: Bool { violations.isEmpty }

    enum CodingKeys: String, CodingKey {
        case violations, inconclusive
        case userInput = "user_input"
    }
}

public enum CtrlGuard {
    private enum Cmp { case same, changed, unknown }

    private static func cmp<T: Equatable>(_ a: T?, _ b: T?) -> Cmp {
        guard let a, let b else { return .unknown }
        return a == b ? .same : .changed
    }

    private static func cmpCursor(_ a: CtrlPoint?, _ b: CtrlPoint?) -> Cmp {
        guard let a, let b else { return .unknown }
        let t = CtrlPolicy.cursorTolerancePt
        return abs(a.x - b.x) <= t && abs(a.y - b.y) <= t ? .same : .changed
    }

    /// Compare two snapshots around an action that took `elapsedMs`. A change while the user was physically active
    /// (HID idle < elapsed) is theirs: inconclusive, not a violation.
    public static func compare(_ before: UiSnapshot, _ after: UiSnapshot, elapsedMs: UInt64) -> GuardOutcome {
        var out = GuardOutcome()
        if let idle = after.hidIdleMs, idle < elapsedMs { out.userInput = true }
        for f in GuardField.allCases {
            let c: Cmp
            switch f {
            case .frontmostApp: c = cmp(before.frontmostPid, after.frontmostPid)
            case .cursor: c = cmpCursor(before.cursor, after.cursor)
            case .focusedElement: c = cmp(before.focusedElement, after.focusedElement)
            case .keyWindow: c = cmp(before.keyWindow, after.keyWindow)
            }
            switch c {
            case .same: break
            case .unknown: out.inconclusive.append(f)
            case .changed: if out.userInput { out.inconclusive.append(f) } else { out.violations.append(f) }
            }
        }
        return out
    }

    /// Run `body` between two snapshots from `probe`. The body's error is returned, never swallowed.
    public static func guarded(probe: () -> UiSnapshot, _ body: () throws -> Void) -> (before: UiSnapshot, after: UiSnapshot, outcome: GuardOutcome, error: Error?) {
        let before = probe()
        let t0 = DispatchTime.now().uptimeNanoseconds
        var err: Error?
        do { try body() } catch { err = error }
        let elapsed = (DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
        let after = probe()
        return (before, after, compare(before, after, elapsedMs: elapsed), err)
    }
}
