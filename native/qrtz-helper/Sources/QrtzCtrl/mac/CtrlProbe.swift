#if os(macOS)
// ctrl UI probe (ADR 0100): reads what the user owns without asking for anything. Frontmost app, cursor and the
// front window need no permission; the focused element needs Accessibility trust and is nil without it. Nothing
// here prompts: a background run never shows a TCC dialog.
import AppKit
import ApplicationServices
import Carbon
import CoreGraphics
import Foundation

public enum CtrlProbe {
    /// Never prompts (`AXIsProcessTrusted`, not the `...WithOptions(prompt:)` form).
    public static var axTrusted: Bool { AXIsProcessTrusted() }
    /// `kTCCServicePostEvent`, checked without prompting.
    public static var postEventAllowed: Bool { CGPreflightPostEventAccess() }
    public static var secureInput: Bool { IsSecureEventInputEnabled() }

    public static func snapshot() -> UiSnapshot {
        var s = UiSnapshot()
        s.frontmostPid = NSWorkspace.shared.frontmostApplication?.processIdentifier
        if let loc = CGEvent(source: nil)?.location { s.cursor = CtrlPoint(x: Double(loc.x), y: Double(loc.y)) }
        s.keyWindow = frontWindowNumber()
        s.focusedElement = focusedElementId()
        s.hidIdleMs = hidIdleMs()
        return s
    }

    /// Milliseconds since the last physical input. Events posted to a pid don't pass through the HID state.
    public static func hidIdleMs() -> UInt64? {
        guard let any = CGEventType(rawValue: ~0) else { return nil }
        let secs = CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: any)
        guard secs.isFinite, secs >= 0 else { return nil }
        return UInt64(secs * 1000)
    }

    /// The front-most normal-layer (0) window on screen, from the window server. No permission needed for numbers,
    /// owners and layers (titles would need Screen Recording; we don't read them).
    public static func frontWindowNumber() -> UInt32? {
        frontWindow()?.number
    }

    public static func frontWindow() -> (number: UInt32, pid: Int32)? {
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else { return nil }
        for w in list {
            guard (w[kCGWindowLayer as String] as? Int) == 0,
                  let n = w[kCGWindowNumber as String] as? Int,
                  let pid = w[kCGWindowOwnerPID as String] as? Int
            else { continue }
            return (UInt32(n), Int32(pid))
        }
        return nil
    }

    /// Opaque id of the system-wide focused element: owner pid plus the element's CFHash. Never its text.
    public static func focusedElementId() -> String? {
        guard axTrusted else { return nil }
        let sys = AXUIElementCreateSystemWide()
        AXUIElementSetMessagingTimeout(sys, 0.5)
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(sys, kAXFocusedUIElementAttribute as CFString, &v) == .success,
              let v, CFGetTypeID(v) == AXUIElementGetTypeID()
        else { return nil }
        let el = v as! AXUIElement
        var pid: pid_t = 0
        AXUIElementGetPid(el, &pid)
        return "e:\(pid):\(String(CFHash(el), radix: 16))"
    }
}
#endif
