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
        // ADR 0137 / 0173 / 0195 / 0197: the appearance, Dock and menu-bar verbs go to System Events and nothing else;
        // nothing else goes to System Events.
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
        // ADR 0195: the Dock's screen edge (System Events tells the Dock; no app comes forward). macOS has no top.
        case "set_dock_edge_left": body = "tell dock preferences to set screen edge to left"
        case "set_dock_edge_right": body = "tell dock preferences to set screen edge to right"
        case "set_dock_edge_bottom": body = "tell dock preferences to set screen edge to bottom"
        // ADR 0200: the Dock's size one fixed step (`CtrlDock.sizeStep` of System Events' 0–1 `dock size`), clamped to
        // the range; System Events tells the Dock, no app comes forward. The step is part of the template, never a
        // number from the wire.
        case "shrink_dock": body = CtrlDock.sizeScript(bigger: false)
        case "grow_dock": body = CtrlDock.sizeScript(bigger: true)
        // ADR 0202: the Dock's magnification on / off (System Events tells the Dock; no app comes forward). Its size
        // (`magnification size`) is never set here.
        case "set_dock_magnification_on": body = "tell dock preferences to set magnification to true"
        case "set_dock_magnification_off": body = "tell dock preferences to set magnification to false"
        // ADR 0197: the menu bar's "automatically hide and show" (System Events' dock preferences; no app comes
        // forward). Only on / off: macOS 13+'s desktop-only and full-screen-only choices stay as the user set them.
        case "set_menu_bar_autohide": body = "tell dock preferences to set autohide menu bar to true"
        case "set_menu_bar_shown": body = "tell dock preferences to set autohide menu bar to false"
        // ADR 0178: the current screen saver (System Events starts it; the screensaver covers the display by design).
        case "start_screen_saver": body = "start current screen saver"
        // ADR 0178 amendment 1: stop it. A password-on-wake Mac keeps its login prompt; nothing here unlocks.
        case "stop_screen_saver": body = "stop current screen saver"
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

/// ADR 0173 / 0195 / 0200 / 0202: reads the Dock's own defaults domain (`com.apple.dock`): `autohide` for hide/show,
/// `orientation` for the screen edge (`left` / `bottom` / `right`; absent = bottom, the macOS default), `tilesize` for
/// the icon size, `magnification` for magnification on / off (absent = off, the macOS default). Nothing else in the domain is looked at, and nothing in it is written (the size goes through System
/// Events).
public enum CtrlDock {
    public static let check = "dock_autohide"
    /// ADR 0195: the Dock's screen edge (0 left, 10 bottom, 20 right — spaced so a Level ±1 never confuses edges).
    public static let edgeCheck = "dock_edge"
    public static let domain = "com.apple.dock"
    public static let edgeLeft = 0
    public static let edgeBottom = 10
    public static let edgeRight = 20

    public static func autohides(_ dock: [String: Any]?) -> Bool {
        switch dock?["autohide"] {
        case let b as Bool: return b
        case let n as NSNumber: return n.boolValue
        case let s as String: return ["1", "true", "yes"].contains(s.lowercased())
        default: return false
        }
    }

    /// ADR 0195: `orientation` as left / right / bottom (absent or anything else → bottom).
    public static func edge(_ dock: [String: Any]?) -> String {
        switch (dock?["orientation"] as? String)?.lowercased() {
        case "left": return "left"
        case "right": return "right"
        default: return "bottom"
        }
    }

    /// ADR 0200: the Dock's icon size in points (`tilesize`, 16–128), or nil when the key is absent (never set on this
    /// Mac: macOS writes it the first time the size changes) or not a number.
    public static let sizeCheck = "dock_size"
    /// ADR 0200: one step of System Events' `dock size` (0 = 16 pt icons, 1 = 128 pt). Rust's `DOCK_SIZE_STEP`.
    public static let sizeStep = "0.15"

    public static func tileSize(_ dock: [String: Any]?) -> Int? {
        switch dock?["tilesize"] {
        case let n as NSNumber: return Int(n.doubleValue.rounded())
        case let s as String: return Double(s).map { Int($0.rounded()) }
        default: return nil
        }
    }

    /// ADR 0200: the body of `shrink_dock` / `grow_dock`: read `dock size`, move it one step, clamp to 0–1, set it.
    static func sizeScript(bigger: Bool) -> String {
        bigger
            ? "tell dock preferences\nset s to (dock size) + \(sizeStep)\nif s > 1 then set s to 1\nset dock size to s\nend tell"
            : "tell dock preferences\nset s to (dock size) - \(sizeStep)\nif s < 0 then set s to 0\nset dock size to s\nend tell"
    }

    /// ADR 0202: whether the Dock magnifies its icons under the pointer (`magnification`; absent or junk = off).
    public static let magnificationCheck = "dock_magnification"

    public static func magnifies(_ dock: [String: Any]?) -> Bool {
        switch dock?["magnification"] {
        case let b as Bool: return b
        case let n as NSNumber: return n.boolValue
        case let s as String: return ["1", "true", "yes"].contains(s.lowercased())
        default: return false
        }
    }

    public static func edgeCode(_ dock: [String: Any]?) -> Int {
        switch edge(dock) {
        case "left": return edgeLeft
        case "right": return edgeRight
        default: return edgeBottom
        }
    }
}

/// ADR 0197: reads the global defaults domain's `_HIHideMenuBar`: true when the menu bar hides itself, absent (never
/// hidden on this Mac) or false when it's shown. Nothing else in the domain is looked at, and nothing is written (macOS
/// ignores a plain defaults write of it; the setting goes through System Events).
public enum CtrlMenuBar {
    public static let check = "menu_bar_autohide"
    public static let key = "_HIHideMenuBar"

    public static func autohides(_ global: [String: Any]?) -> Bool {
        switch global?[key] {
        case let b as Bool: return b
        case let n as NSNumber: return n.boolValue
        case let s as String: return ["1", "true", "yes"].contains(s.lowercased())
        default: return false
        }
    }
}

/// ADR 0178: whether the screen saver is running, from the helper's own process list (NSWorkspace), never an Apple
/// Event. `com.apple.ScreenSaver.Engine` is observed only; nothing is sent to it.
public enum CtrlScreensaver {
    public static let check = "screensaver_running"
    /// The ScreenSaverEngine process's bundle id on macOS (observe-only; no Apple Event target).
    public static let engineBundle = "com.apple.ScreenSaver.Engine"

    public static func running(bundleIds: [String], names: [String]) -> Bool {
        bundleIds.contains(engineBundle) || names.contains("ScreenSaverEngine")
    }
}

/// ADR 0180: "turn off my display". The act puts the displays to sleep the way `pmset displaysleepnow` does (IOKit:
/// `IORequestIdle = true` on the display wrangler), in the helper's own process: no process spawned, no Apple Event,
/// no key event, the Mac stays awake and nothing is locked. The measure reads the main display's sleep state
/// (`CGDisplayIsAsleep`) in this process. The names live here (Foundation) so Linux tests see them; the calls are in
/// mac/CtrlDisplayMac.swift.
public enum CtrlDisplay {
    public static let op = "sleep_display"
    public static let check = "display_asleep"
    /// The registry entry and property `pmset displaysleepnow` sets; nothing else in the registry is written.
    public static let wranglerPath = "IOService:/IOResources/IODisplayWrangler"
    public static let requestIdle = "IORequestIdle"
}

/// ADR 0183: "put my mac to sleep". The act asks the power manager for a system sleep the way `pmset sleepnow` does
/// (IOKit: `IOPMSleepSystem` on the root power domain's connection), in the helper's own process: no process spawned,
/// no Apple Event, no key event, nothing quit or closed (the system tells apps itself). The measure is when the Mac last
/// went to sleep (`kern.sleeptime`, whole seconds), read in this process. Right after the act the read waits, bounded,
/// for that time to move (the sleep itself suspends this process; the read finishes once the Mac is awake again), so
/// the host's verify (increased) sees the sleep it asked for. The names and the wait live here (Foundation) so Linux
/// tests see them; the calls are in mac/CtrlSystemSleepMac.swift.
public enum CtrlSystemSleep {
    public static let op = "sleep_system"
    public static let check = "last_sleep"
    /// The sysctl the measure reads; nothing else is read or written.
    public static let sleepTimeName = "kern.sleeptime"
    /// The longest a read after the act waits for the sleep (under the host's 10 s helper timeout).
    public static let settleSeconds: Double = 8
    public static let pollSeconds: Double = 0.25

    /// `read()` until it differs from `baseline` or `settle` seconds of polling have passed; the last read either way
    /// (nil stays nil: unreadable is never made up). No baseline (no act pending): one read, no wait.
    public static func awaitSleep(baseline: Int?, settle: Double = settleSeconds, poll: Double = pollSeconds,
                                  read: () -> Int?, wait: (Double) -> Void) -> Int? {
        var now = read()
        guard let b = baseline else { return now }
        var waited = 0.0
        while now == b && waited < settle {
            wait(poll)
            waited += poll
            now = read()
        }
        return now
    }
}

/// ADR 0191: "turn off my wifi" / "turn wi-fi back on". The act sets the Wi-Fi radio's power the way the menu bar
/// switch does (CoreWLAN: `setPower` on the default Wi-Fi interface), in the helper's own process: no process spawned
/// (no networksetup), no Apple Event, no key event, no network joined, left or scanned. The measure is the radio's
/// power (`powerOn`), read in this process; nothing that needs Location (a network's name, the networks around) is
/// ever read. The names and the error mapping live here (Foundation) so Linux tests see them; the calls are in
/// mac/CtrlWifiMac.swift.
public enum CtrlWifi {
    public static let op = "set_wifi"
    public static let check = "wifi_on"
    /// CoreWLAN's `kCWOperationNotPermittedErr`: the Mac requires an administrator to turn Wi-Fi on or off (a System
    /// Settings option). Assumed until a Mac check, like ADR 0158's error numbers; anything else is `wifi.refused`.
    public static let notPermittedCode = -3930

    /// The failure an act answers for a CoreWLAN error code (never a prompt, never retried another way).
    public static func failure(code: Int) -> String {
        code == notPermittedCode ? "wifi.needs_admin" : "wifi.refused"
    }

    /// The measure's value: 1 on, 0 off, nil with no Wi-Fi interface.
    public static func value(powerOn: Bool?) -> Int? {
        powerOn.map { $0 ? 1 : 0 }
    }
}

/// ADR 0194: "keep my mac awake for 2 hours" / "let my mac sleep again". The act holds one IOKit power assertion of
/// the helper's own (`PreventUserIdleSystemSleep`, or `PreventUserIdleDisplaySleep` to keep the screen on too: what
/// `caffeinate` / `caffeinate -d` ask for, without spawning it) with a kernel timeout whose action releases it, so it
/// never outlives the task's duration, and it ends with the helper's process. A newer keep-awake replaces the last one;
/// `allow_sleep` releases it. The measure is that one assertion's level, read from IOKit; no other app's assertions are
/// listed or read. Nothing on screen changes, no app, no key event, nothing put to sleep. The names, bounds and parse
/// live here (Foundation) so Linux tests see them; the calls are in mac/CtrlKeepAwakeMac.swift.
public enum CtrlKeepAwake {
    public static let op = "keep_awake"
    public static let releaseOp = "allow_sleep"
    public static let check = "kept_awake"
    /// The longest hold, 12 hours (the host never sends more; more is a bad request, never clamped).
    public static let maxMinutes = 720
    /// IOPMLib.h's assertion types (kIOPMAssertPreventUserIdleSystemSleep / kIOPMAssertPreventUserIdleDisplaySleep).
    public static let systemType = "PreventUserIdleSystemSleep"
    public static let displayType = "PreventUserIdleDisplaySleep"
    /// IOPMLib.h's kIOPMAssertionTimeoutKey, kIOPMAssertionTimeoutActionKey, kIOPMAssertionTimeoutActionRelease and
    /// kIOPMAssertionLevelKey / kIOPMAssertionLevelOn. The default timeout action only turns the level off; release is
    /// set explicitly so nothing of ctrl's is left behind.
    public static let timeoutKey = "TimeoutSeconds"
    public static let timeoutActionKey = "TimeoutAction"
    public static let timeoutActionRelease = "TimeoutActionRelease"
    public static let levelKey = "AssertLevel"
    public static let levelOn = 255
    /// What Activity Monitor / `pmset -g assertions` show as the holder: who, never anything from the task.
    public static let name = "qrtz ctrl: keep awake"

    public static func type(display: Bool) -> String { display ? displayType : systemType }

    /// The timeout for a hold of `minutes`; nil outside 1–720.
    public static func seconds(minutes: Int) -> Double? {
        (1...maxMinutes).contains(minutes) ? Double(minutes * 60) : nil
    }

    /// The measure: 1 while IOKit's record of the assertion says it is on, 0 with none (released, timed out, never
    /// made) or off.
    public static func value(properties: [String: Any]?) -> Int {
        guard let level = (properties?[levelKey] as? NSNumber)?.intValue else { return 0 }
        return level == levelOn ? 1 : 0
    }

    /// `minutes` a JSON whole number 1–720 (objCType not "c" bool, ADR 0141, nor "d"/"f" fraction), `display` a
    /// strict JSON bool (objCType "c"); anything else is a bad request.
    static func parse(_ o: [String: Any]) -> CtrlRoute? {
        guard let m = o["minutes"] as? NSNumber, !["c", "d", "f"].contains(String(cString: m.objCType)),
              let minutes = o["minutes"] as? Int, seconds(minutes: minutes) != nil,
              let d = o["display"] as? NSNumber, String(cString: d.objCType) == "c", let display = o["display"] as? Bool
        else { return nil }
        return .keepAwake(minutes: minutes, display: display)
    }
}
