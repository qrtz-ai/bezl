// ctrl never-takes-over-the-ui tables (ADR 0100). Mirrors crates/qrtz-ctrl/src/policy.rs; both test against
// Fixtures/ctrl/policy.json. The Rust ladder decides the route; the helper re-checks these before it touches
// anything (defense in depth), so a bad route is refused on both sides.
import Foundation

public enum CtrlPolicy {
    public static let forbiddenAXActions = ["AXRaise", "AXShowMenu", "AXPick"]
    public static let forbiddenAXSetAttributes = ["AXFocused", "AXMain", "AXFrontmost", "AXPosition", "AXSize", "AXFullScreen"]
    public static let menuOpeningRoles = ["AXMenuBar", "AXMenuBarItem", "AXMenuItem", "AXMenuButton", "AXPopUpButton", "AXComboBox"]
    public static let secureRoles = ["AXSecureTextField"]
    public static let focusIntentShortcuts = [
        "cmd+l", "cmd+shift+g", "cmd+tab", "cmd+grave", "cmd+space",
        "cmd+1", "cmd+2", "cmd+3", "cmd+4", "cmd+5", "cmd+6", "cmd+7", "cmd+8", "cmd+9",
        "cmd+[", "cmd+]", "cmd+shift+[", "cmd+shift+]", "ctrl+tab", "ctrl+shift+tab",
        "ctrl+left", "ctrl+right", "ctrl+up", "ctrl+down",
    ]
    public static let userActiveWindowMs: UInt64 = 2000
    public static let cursorTolerancePt = 0.5

    public enum Refusal: Error, Equatable {
        case forbiddenAction(String)
        case forbiddenAttribute(String)
        case opensMenu(String)
        case secureField
        case focusIntentShortcut(String)
        case targetIsFrontmost
    }

    /// Canonical combo: lowercase, modifiers ordered cmd, ctrl, option, shift, then the key. Menu glyphs are spelled out.
    public static func normalizeCombo(_ combo: String) -> String {
        var s = combo.lowercased()
        for (g, w) in [("⌘", "cmd+"), ("⌃", "ctrl+"), ("⌥", "option+"), ("⇧", "shift+")] { s = s.replacingOccurrences(of: g, with: w) }
        var mods = [false, false, false, false]
        var key = ""
        for raw in s.split(separator: "+") {
            let p = raw.trimmingCharacters(in: .whitespaces)
            switch p {
            case "": continue
            case "cmd", "command", "meta", "super": mods[0] = true
            case "ctrl", "control": mods[1] = true
            case "option", "opt", "alt": mods[2] = true
            case "shift": mods[3] = true
            case "`": key = "grave"
            default: key = p
            }
        }
        let names = ["cmd", "ctrl", "option", "shift"]
        return (zip(names, mods).filter { $0.1 }.map { $0.0 } + [key]).joined(separator: "+")
    }

    public static func isFocusIntent(_ combo: String) -> Bool {
        let c = normalizeCombo(combo)
        return focusIntentShortcuts.contains { normalizeCombo($0) == c }
    }

    /// Re-check an AX action on an element with `role`.
    public static func checkAXAction(_ action: String, role: String) throws {
        if secureRoles.contains(role) { throw Refusal.secureField }
        if forbiddenAXActions.contains(action) { throw Refusal.forbiddenAction(action) }
        if action == "AXPress", menuOpeningRoles.contains(role) { throw Refusal.opensMenu(role) }
    }

    public static func checkAXSet(_ attribute: String, role: String) throws {
        if secureRoles.contains(role) { throw Refusal.secureField }
        if forbiddenAXSetAttributes.contains(attribute) { throw Refusal.forbiddenAttribute(attribute) }
    }

    public static func checkKey(_ combo: String, targetPid: Int32, frontmostPid: Int32?) throws {
        if isFocusIntent(combo) { throw Refusal.focusIntentShortcut(normalizeCombo(combo)) }
        if frontmostPid == targetPid { throw Refusal.targetIsFrontmost }
    }
}
