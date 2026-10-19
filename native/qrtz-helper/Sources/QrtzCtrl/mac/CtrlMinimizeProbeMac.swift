#if os(macOS)
// ADR 0187: the macOS half of the minimize probe (CtrlMinimizeProbe holds the rules). Two parts:
// - `minimizedCount`: read-only, the number of an app's windows that read `AXMinimized == true` (nil without
//   Accessibility trust). The read-only Mac check sums it over the running apps; it is what a later pre-check would
//   read. Nothing is written.
// - `rounds`: the opt-in active probe (QrtzCtrlTests, QRTZ_CTRL_MAC_PROBES=1, never in CI) on a window of a TextEdit
//   document the test opened itself. It writes `AXMinimized` raw, not through CtrlActions.set: the policy refuses
//   that write (ADR 0187), and measuring the bare primitive is the point. Nothing in the helper calls this.
import AppKit
import ApplicationServices
import Foundation

public enum CtrlMinimizeProbeMac {
    public static func isMinimized(_ window: AXUIElement) -> Bool? {
        CtrlActions.raw(window, kAXMinimizedAttribute) as? Bool
    }

    /// Read-only: how many of `pid`'s windows are minimized. nil without Accessibility trust (never prompts).
    public static func minimizedCount(_ pid: Int32) -> Int? {
        guard CtrlProbe.axTrusted else { return nil }
        return CtrlActions.windows(pid).filter { isMinimized($0) == true }.count
    }

    private static func write(_ window: AXUIElement, _ minimized: Bool) {
        if minimized {
            _ = AXUIElementSetAttributeValue(window, CtrlMinimizeProbe.attribute as CFString, kCFBooleanTrue)
        } else {
            _ = AXUIElementSetAttributeValue(window, CtrlMinimizeProbe.attribute as CFString, kCFBooleanFalse)
        }
    }

    /// Minimize `window` (of the probe app `pid`) and bring it back, `count` times, each write between guard
    /// snapshots from `probe`. `spin(seconds, done)` runs the main run loop until `done` or the time is up.
    public static func rounds(_ count: Int, window: AXUIElement, pid: Int32, probe: () -> UiSnapshot,
                              spin: (Double, () -> Bool) -> Void) -> [CtrlMinimizeProbe.Verdict] {
        var verdicts: [CtrlMinimizeProbe.Verdict] = []
        for _ in 0..<count {
            let frontBefore = CtrlProbe.frontWindow()?.pid
            let m = CtrlGuard.guarded(probe: probe) {
                write(window, true)
                spin(3) { isMinimized(window) == true }
            }
            let minimized = isMinimized(window) == true
            spin(0.5) { false }
            let r = CtrlGuard.guarded(probe: probe) {
                write(window, false)
                spin(3) { isMinimized(window) == false }
            }
            spin(0.3) { false }
            let round = CtrlMinimizeProbe.Round(
                minimize: m.outcome, restore: r.outcome, minimizedAfterWrite: minimized,
                restoredAfterWrite: isMinimized(window) == false, frontOwnerBefore: frontBefore,
                frontOwnerAfterRestore: CtrlProbe.frontWindow()?.pid, probePid: pid)
            verdicts.append(CtrlMinimizeProbe.verdict(round))
            // A takeover ends the probe: once is enough to keep the hold, and it must not repeat on the user's screen.
            if verdicts.last != .clean && verdicts.last != .inconclusive && verdicts.last != .noEffect { break }
        }
        return verdicts
    }
}
#endif
