// ADR 0187: the Mac probe that decides whether ctrl may ever write `AXMinimized`. Foundation only, so the verdict
// rules are tested on Linux; the Mac driver is mac/CtrlMinimizeProbeMac.swift (opt-in, QRTZ_CTRL_MAC_PROBES=1, on a
// TextEdit document it opens itself, never one of the user's windows). One round minimizes the probe window and
// brings it back, each write between two guard snapshots (ADR 0100). Until enough rounds come back clean on a real
// Mac, ctrl's policy refuses the write both ways and the minimize / un-minimize intents are held.
import Foundation

public enum CtrlMinimizeProbe {
    /// The attribute under test. Refused by CtrlPolicy until an ADR 0187 amendment records a clean probe.
    public static let attribute = "AXMinimized"
    /// Clean rounds in a row a probe needs before an amendment may lift the hold (one is luck, a few is the OS).
    public static let roundsNeeded = 3

    /// What one round measured.
    public struct Round: Equatable, Sendable {
        /// The guard around `AXMinimized = true` and around `AXMinimized = false`.
        public var minimize: GuardOutcome
        public var restore: GuardOutcome
        /// The probe window read back as minimized after the first write, and as not minimized after the second.
        public var minimizedAfterWrite: Bool
        public var restoredAfterWrite: Bool
        /// The owner pid of the front-most normal-layer window on screen before the round and right after the restore.
        public var frontOwnerBefore: Int32?
        public var frontOwnerAfterRestore: Int32?
        /// The probe app's pid.
        public var probePid: Int32
        public init(minimize: GuardOutcome, restore: GuardOutcome, minimizedAfterWrite: Bool, restoredAfterWrite: Bool,
                    frontOwnerBefore: Int32?, frontOwnerAfterRestore: Int32?, probePid: Int32) {
            self.minimize = minimize
            self.restore = restore
            self.minimizedAfterWrite = minimizedAfterWrite
            self.restoredAfterWrite = restoredAfterWrite
            self.frontOwnerBefore = frontOwnerBefore
            self.frontOwnerAfterRestore = frontOwnerAfterRestore
            self.probePid = probePid
        }
    }

    public enum Verdict: String, Equatable, Sendable {
        /// Both writes took and neither changed the frontmost app, focus, cursor or the front window.
        case clean
        /// The restored window came up over the user's front window (it owns the front window now).
        case raised
        /// A write changed the frontmost app.
        case activated
        /// A write moved the focused element or the cursor.
        case focusMoved = "focus_moved"
        /// A write changed the front window some other way.
        case frontWindowChanged = "front_window_changed"
        /// A write didn't take (the window never minimized, or never came back).
        case noEffect = "no_effect"
        /// The user typed or clicked during a write, or a field couldn't be read: no verdict.
        case inconclusive
    }

    /// The worst thing a round shows, in order: a takeover beats an unreadable field, which beats a write that didn't
    /// take. Only a round that shows nothing at all is clean.
    public static func verdict(_ r: Round) -> Verdict {
        let v = r.minimize.violations + r.restore.violations
        if v.contains(.frontmostApp) { return .activated }
        // The probe window was the front window already: a raise couldn't show. Not a verdict.
        if r.frontOwnerBefore == r.probePid { return .inconclusive }
        let raisedNow = r.frontOwnerAfterRestore == r.probePid && r.frontOwnerBefore != r.probePid
        if raisedNow { return .raised }
        if v.contains(.focusedElement) || v.contains(.cursor) { return .focusMoved }
        if v.contains(.keyWindow) { return .frontWindowChanged }
        if r.minimize.userInput || r.restore.userInput { return .inconclusive }
        // Frontmost and the front window must be readable; focus may be nil (no AX trust), but then no AX write ran.
        let unreadable = (r.minimize.inconclusive + r.restore.inconclusive).contains { $0 == .frontmostApp || $0 == .keyWindow }
        if unreadable || r.frontOwnerAfterRestore == nil { return .inconclusive }
        if !r.minimizedAfterWrite || !r.restoredAfterWrite { return .noEffect }
        return .clean
    }

    /// Whether a probe's rounds are enough to propose lifting the hold: at least `roundsNeeded`, every one clean. Any
    /// takeover in any round keeps the hold for good on that macOS (an amendment records it).
    public static func liftsHold(_ verdicts: [Verdict]) -> Bool {
        verdicts.count >= roundsNeeded && verdicts.allSatisfy { $0 == .clean }
    }

    /// The probe's one-line report (no titles, no names: verdicts, counts, the OS version).
    public static func report(_ verdicts: [Verdict], os: String) -> String {
        let list = verdicts.map(\.rawValue).joined(separator: ",")
        return "[ctrl-minimize] os=\(os) rounds=\(verdicts.count) verdicts=\(list) lifts_hold=\(liftsHold(verdicts))"
    }
}
