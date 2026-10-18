#if os(macOS)
// ctrl background actions (ADR 0100): the macOS primitives for rungs 2, 4 and 5 of the ladder. Every entry point
// re-checks CtrlPolicy before it touches anything, so a route the Rust ladder should have refused is refused here
// too. There is deliberately no API here that activates an app, moves the cursor or posts to the global event
// stream; the only activation is `restoreFrontmost`, which puts the user's app back after a violation.
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

public enum CtrlActions {
    public enum Failure: Error, Equatable {
        case notTrusted
        case ax(Int32)
        case noElement
        case eventSource
        case refused(CtrlPolicy.Refusal)
    }

    // MARK: rung 2: workspace without activation

    public static func backgroundConfiguration() -> NSWorkspace.OpenConfiguration {
        let c = NSWorkspace.OpenConfiguration()
        c.activates = false
        c.addsToRecentItems = false
        c.promptsUserIfNeeded = false
        return c
    }

    /// Open `urls` in the app at `appURL` without activating it. `done` runs on an arbitrary queue.
    public static func open(_ urls: [URL], withApp appURL: URL, done: @escaping (NSRunningApplication?, Error?) -> Void) {
        // AppKit's handler is @Sendable; callers already expect any queue (see above), so hand it over unchecked.
        let box = UncheckedDone(call: done)
        NSWorkspace.shared.open(urls, withApplicationAt: appURL, configuration: backgroundConfiguration()) { app, err in
            box.call(app, err)
        }
    }

    private struct UncheckedDone: @unchecked Sendable {
        let call: (NSRunningApplication?, Error?) -> Void
    }

    /// Re-activate the app the user had in front (after a guard violation). Under cooperative activation (macOS 14+)
    /// this is a request the system may deny; the caller records whether it worked.
    @discardableResult
    public static func restoreFrontmost(_ pid: Int32) -> Bool {
        guard let app = NSRunningApplication(processIdentifier: pid) else { return false }
        return app.activate(options: [])
    }

    // MARK: rung 4: background AX

    public static func appElement(_ pid: Int32) -> AXUIElement {
        let el = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(el, 1.0)
        return el
    }

    static func raw(_ el: AXUIElement, _ name: String) -> CFTypeRef? {
        var v: CFTypeRef?
        return AXUIElementCopyAttributeValue(el, name as CFString, &v) == .success ? v : nil
    }

    static func element(_ v: CFTypeRef?) -> AXUIElement? {
        guard let v, CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
        return (v as! AXUIElement)
    }

    public static func role(_ el: AXUIElement) -> String {
        guard let v = raw(el, kAXRoleAttribute), CFGetTypeID(v) == CFStringGetTypeID() else { return "AXUnknown" }
        return (v as! CFString) as String
    }

    public static func windows(_ pid: Int32) -> [AXUIElement] {
        guard let v = raw(appElement(pid), kAXWindowsAttribute), CFGetTypeID(v) == CFArrayGetTypeID() else { return [] }
        return (((v as! CFArray) as NSArray) as [AnyObject]).compactMap { element($0 as CFTypeRef) }
    }

    public static func closeButton(_ window: AXUIElement) -> AXUIElement? {
        element(raw(window, kAXCloseButtonAttribute))
    }

    /// Perform an AX action on an element of a (background) app, after the policy check.
    public static func perform(_ action: String, on el: AXUIElement) throws {
        guard CtrlProbe.axTrusted else { throw Failure.notTrusted }
        do { try CtrlPolicy.checkAXAction(action, role: role(el)) } catch let r as CtrlPolicy.Refusal { throw Failure.refused(r) }
        let e = AXUIElementPerformAction(el, action as CFString)
        guard e == .success else { throw Failure.ax(e.rawValue) }
    }

    /// Write an AX attribute (AXValue on a field, a checkbox), after the policy check.
    public static func set(_ attribute: String, _ value: CFTypeRef, on el: AXUIElement) throws {
        guard CtrlProbe.axTrusted else { throw Failure.notTrusted }
        do { try CtrlPolicy.checkAXSet(attribute, role: role(el)) } catch let r as CtrlPolicy.Refusal { throw Failure.refused(r) }
        let e = AXUIElementSetAttributeValue(el, attribute as CFString, value)
        guard e == .success else { throw Failure.ax(e.rawValue) }
    }

    // MARK: rung 5: per-process key events

    /// Post one key combo to `pid` with a **private** event source: the events carry exactly the flags given here,
    /// are not merged with the modifiers the user is holding, and don't change the session's key state.
    public static func postKey(_ keyCode: CGKeyCode, flags: CGEventFlags, combo: String, to pid: Int32) throws {
        do {
            try CtrlPolicy.checkKey(combo, targetPid: pid, frontmostPid: NSWorkspace.shared.frontmostApplication?.processIdentifier)
        } catch let r as CtrlPolicy.Refusal { throw Failure.refused(r) }
        guard let src = CGEventSource(stateID: .privateState),
              let down = CGEvent(keyboardEventSource: src, virtualKey: keyCode, keyDown: true),
              let up = CGEvent(keyboardEventSource: src, virtualKey: keyCode, keyDown: false)
        else { throw Failure.eventSource }
        down.flags = flags
        up.flags = flags
        down.postToPid(pid)
        up.postToPid(pid)
    }
}
#endif
