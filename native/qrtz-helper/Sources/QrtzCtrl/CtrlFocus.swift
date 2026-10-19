// ADR 0158: the focus / do not disturb route, the plain-data half (Foundation-only, tested on Linux). ctrl runs one
// shortcut the user made once, "qrtz Set Focus", through Shortcuts Events (`com.apple.shortcuts.events`, a faceless
// agent: no activation, no window, no key event), with a mode from a closed set as its input. Bozniak's amendments:
// the target is that bundle id only; the name is a compile-time constant on an in-code allowlist; the mode is a strict
// enum checked here before any event is built; a missing shortcut or denied Automation stops (no command-line or script-runner fallback).
import Foundation

/// The shortcut's input. Mirrors Rust `FocusMode` (snake_case); anything else is refused before an event is built.
public enum CtrlFocusMode: String, CaseIterable, Sendable {
    case doNotDisturb = "do_not_disturb"
    case work, personal, sleep, driving, fitness, gaming, mindfulness, reading
    case off
}

public enum CtrlFocus {
    /// Amendment 1: the one Apple Event target of this route, by bundle id (never an app name or pid).
    public static let target = "com.apple.shortcuts.events"
    /// Amendment 2: the only shortcut ctrl runs. Never from settings, the protocol or model output.
    public static let shortcutName = "qrtz Set Focus"
    /// The in-code allowlist; another name needs an entry here and an ADR note.
    public static let allowedShortcuts: Set<String> = [shortcutName]

    /// Amendment 3: exactly one of the enum's spellings, nothing else (no trimming, no case folding).
    public static func mode(_ raw: String) -> CtrlFocusMode? { CtrlFocusMode(rawValue: raw) }

    /// The one `run` command to Shortcuts Events, or nil for a name off the allowlist. Both strings are constants
    /// (the name from the allowlist, the input an enum raw value of [a-z_]), so nothing can break out of the
    /// literals. It compiles to a single `run` event sent to `application id "com.apple.shortcuts.events"`.
    public static func source(shortcut: String, mode: CtrlFocusMode) -> String? {
        guard allowedShortcuts.contains(shortcut), CtrlScripts.safeBundleId(target) else { return nil }
        guard mode.rawValue.unicodeScalars.allSatisfy({ ("a"..."z").contains($0) || $0 == "_" }) else { return nil }
        return "tell application id \"\(target)\" to run shortcut named \"\(shortcut)\" with input \"\(mode.rawValue)\""
    }

    /// Amendment 4: the error number of a failed run → the code the host turns into "set up the shortcut" or
    /// "allow automation". -1743 is errAEEventNotPermitted (Automation denied), -1744 errAEEventWouldRequireUserConsent;
    /// -1728 errAENoSuchObject and -1719 errAEIllegalIndex are what Shortcuts Events answers for a name it doesn't
    /// have. Anything else is a plain failure; nothing else is tried either way.
    public static func failure(errorNumber: Int?) -> CtrlFailure {
        switch errorNumber {
        case -1743, -1744: return CtrlFailure("automation.denied")
        case -1728, -1719: return CtrlFailure("focus.no_shortcut")
        default: return CtrlFailure("focus.failed")
        }
    }
}
