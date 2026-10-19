// The pieces of the macOS backend that are plain data (slice 5b, ADR 0110), Foundation-only so they are tested on
// Linux: the AppleScript source for each scripting verb, and a key combo ("cmd-w") → virtual key code + modifiers
// for per-process key events. No script here can launch, activate, open or reopen anything; the backend only runs
// one against an app that is already running (`tell application id` would otherwise launch it).
import Foundation

public enum CtrlScripts {
    /// A bundle id we put into a script: reverse-DNS characters only, so nothing can break out of the string.
    public static func safeBundleId(_ s: String) -> Bool {
        !s.isEmpty && s.count <= 255 && s.unicodeScalars.allSatisfy {
            CharacterSet.alphanumerics.contains($0) && $0.isASCII || $0 == "." || $0 == "-" || $0 == "_"
        }
    }

    /// The script for `verb` (Rust `ScriptVerb`) against `bundleId`, or nil for an unknown verb or unsafe id.
    /// Counting verbs return an integer. `close_window` keeps the app's own save behaviour (it never says
    /// `saving no`, which would throw away unsaved work).
    public static func source(verb: String, bundleId: String) -> String? {
        guard safeBundleId(bundleId), CtrlRoute.scriptVerbs.contains(verb) else { return nil }
        // ADR 0137 / 0173: the appearance and Dock verbs go to System Events and nothing else; nothing else goes to
        // System Events.
        guard CtrlRoute.systemEventsVerbs.contains(verb) == (bundleId == CtrlRoute.systemEvents) else { return nil }
        let body: String
        switch verb {
        case "count_tabs":
            body = """
            set n to 0
            repeat with w in windows
            try
            set n to n + (count of tabs of w)
            end try
            end repeat
            return n
            """
        case "close_all_tabs": body = "close every tab of every window"
        case "close_tab": body = "if (count of windows) > 0 then close current tab of front window"
        case "count_windows": body = "return count of windows"
        case "close_window": body = "if (count of windows) > 0 then close front window"
        case "set_dark_mode": body = "tell appearance preferences to set dark mode to true"
        case "set_light_mode": body = "tell appearance preferences to set dark mode to false"
        // ADR 0173: the Dock's "automatically hide and show the Dock" (System Events tells the Dock; no app comes
        // forward).
        case "set_dock_autohide": body = "tell dock preferences to set autohide to true"
        case "set_dock_shown": body = "tell dock preferences to set autohide to false"
        default: return nil
        }
        return "tell application id \"\(bundleId)\"\n\(body)\nend tell"
    }
}

/// A key combo for a per-process key event (the ladder sends "cmd+w").
public struct CtrlKeyCombo: Equatable, Sendable {
    public let keyCode: UInt16
    public let command: Bool, shift: Bool, option: Bool, control: Bool

    /// ANSI virtual key codes (Carbon `kVK_*`), so this file needs no Apple framework.
    static let keys: [String: UInt16] = [
        "a": 0x00, "s": 0x01, "d": 0x02, "f": 0x03, "h": 0x04, "g": 0x05, "z": 0x06, "x": 0x07, "c": 0x08, "v": 0x09,
        "b": 0x0B, "q": 0x0C, "w": 0x0D, "e": 0x0E, "r": 0x0F, "y": 0x10, "t": 0x11, "1": 0x12, "2": 0x13, "3": 0x14,
        "4": 0x15, "6": 0x16, "5": 0x17, "=": 0x18, "9": 0x19, "7": 0x1A, "-": 0x1B, "8": 0x1C, "0": 0x1D, "]": 0x1E,
        "o": 0x1F, "u": 0x20, "[": 0x21, "i": 0x22, "p": 0x23, "l": 0x25, "j": 0x26, "k": 0x28, ";": 0x29, ",": 0x2B,
        "/": 0x2C, "n": 0x2D, "m": 0x2E, ".": 0x2F, "`": 0x32,
        "return": 0x24, "enter": 0x24, "tab": 0x30, "space": 0x31, "delete": 0x33, "escape": 0x35, "esc": 0x35,
        "left": 0x7B, "right": 0x7C, "down": 0x7D, "up": 0x7E,
    ]

    /// Any spelling `CtrlPolicy.normalizeCombo` accepts ("cmd+w", "⌘W", "Cmd + Shift + T").
    public static func parse(_ combo: String) -> CtrlKeyCombo? {
        var parts = CtrlPolicy.normalizeCombo(combo).split(separator: "+").map(String.init)
        guard var key = parts.popLast() else { return nil }
        if key == "grave" { key = "`" }
        guard let code = keys[key] else { return nil }
        return CtrlKeyCombo(keyCode: code, command: parts.contains("cmd"), shift: parts.contains("shift"),
                            option: parts.contains("option"), control: parts.contains("ctrl"))
    }
}

/// ADR 0137: reads the global domain's `AppleInterfaceStyle` ("Dark" in dark mode, absent in light mode).
public enum CtrlAppearance {
    public static func isDark(_ global: [String: Any]?) -> Bool {
        (global?["AppleInterfaceStyle"] as? String)?.caseInsensitiveCompare("dark") == .orderedSame
    }
}

/// ADR 0173: reads the Dock's own defaults domain (`com.apple.dock`): `autohide` true when the Dock hides itself,
/// absent or false when it's always shown. Nothing else in the domain is looked at.
public enum CtrlDock {
    public static let check = "dock_autohide"
    public static let domain = "com.apple.dock"

    public static func autohides(_ dock: [String: Any]?) -> Bool {
        switch dock?["autohide"] {
        case let b as Bool: return b
        case let n as NSNumber: return n.boolValue
        case let s as String: return ["1", "true", "yes"].contains(s.lowercased())
        default: return false
        }
    }
}
