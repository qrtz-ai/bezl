// qrtz snap's shortcuts (ADR 0159): Spectacle's defaults (SpectacleDefaultShortcutHelpers.m, MIT, Copyright (c) 2017
// Eric Czarny; notice in SnapGeometry.swift), its key binding strings ("ctrl+alt+cmd+right") and its validation.
// The host passes the user's overrides (config `snap.shortcuts`, from snap's own settings page); "" clears one.
import Foundation

public struct SnapShortcut: Equatable, Hashable, Sendable, CustomStringConvertible {
    public var modifiers: Set<String>
    public var key: String
    public static let modifierOrder = ["ctrl", "alt", "shift", "cmd"]

    /// Parses "alt+shift+cmd+z" (any order; "option"/"opt" and "command" and "control" accepted). Nil if malformed.
    public init?(binding: String) {
        let parts = binding.lowercased().split(separator: "+").map { $0.trimmingCharacters(in: .whitespaces) }
        guard let k = parts.last, !k.isEmpty, SnapKeyCodes.code(for: k) != nil else { return nil }
        var mods = Set<String>()
        for p in parts.dropLast() {
            switch p {
            case "ctrl", "control": mods.insert("ctrl")
            case "alt", "opt", "option": mods.insert("alt")
            case "shift": mods.insert("shift")
            case "cmd", "command": mods.insert("cmd")
            default: return nil
            }
        }
        modifiers = mods
        key = k
    }

    /// Canonical binding, ctrl+alt+shift+cmd order (Spectacle's).
    public var description: String {
        (Self.modifierOrder.filter(modifiers.contains) + [key]).joined(separator: "+")
    }

    /// Carbon's modifier mask (cmdKey 256, shiftKey 512, optionKey 2048, controlKey 4096).
    public var carbonModifiers: UInt32 {
        var m: UInt32 = 0
        if modifiers.contains("cmd") { m |= 256 }
        if modifiers.contains("shift") { m |= 512 }
        if modifiers.contains("alt") { m |= 2048 }
        if modifiers.contains("ctrl") { m |= 4096 }
        return m
    }

    public var keyCode: UInt32 { SnapKeyCodes.code(for: key) ?? 0 }

    /// Spectacle's SpectacleShortcutValidation: a shortcut needs ⌘, ⌥ or ⌃ (shift alone types a character), except
    /// on a function key.
    public var isValid: Bool {
        if key.hasPrefix("f"), key.count > 1, Int(key.dropFirst()) != nil { return true }
        return !modifiers.isDisjoint(with: ["cmd", "alt", "ctrl"])
    }
}

public enum SnapShortcuts {
    /// Spectacle's builtin defaults, unchanged.
    public static let defaults: [SnapAction: String] = [
        .center: "alt+cmd+c",
        .fullscreen: "alt+cmd+f",
        .leftHalf: "alt+cmd+left",
        .rightHalf: "alt+cmd+right",
        .topHalf: "alt+cmd+up",
        .bottomHalf: "alt+cmd+down",
        .upperLeft: "ctrl+cmd+left",
        .lowerLeft: "ctrl+shift+cmd+left",
        .upperRight: "ctrl+cmd+right",
        .lowerRight: "ctrl+shift+cmd+right",
        .nextDisplay: "ctrl+alt+cmd+right",
        .previousDisplay: "ctrl+alt+cmd+left",
        .nextThird: "ctrl+alt+right",
        .previousThird: "ctrl+alt+left",
        .makeLarger: "ctrl+alt+shift+right",
        .makeSmaller: "ctrl+alt+shift+left",
        .undo: "alt+cmd+z",
        .redo: "alt+shift+cmd+z",
    ]

    public struct Resolved: Equatable {
        public var shortcuts: [SnapAction: SnapShortcut]
        /// Overrides that were refused: malformed, invalid, or the same keys as another action (`snap.status` reports
        /// them so snap's settings page can say why). An action whose override is refused has no shortcut.
        public var refused: [SnapAction: String]
    }

    /// The defaults with the user's overrides (unknown action names ignored; "" clears the action's shortcut).
    public static func resolve(overrides: [String: String]) -> Resolved {
        var bindings = defaults
        var refused: [SnapAction: String] = [:]
        for (name, b) in overrides {
            guard let a = SnapAction(rawValue: name) else { continue }
            bindings[a] = b
        }
        var out: [SnapAction: SnapShortcut] = [:]
        for a in SnapAction.allCases {
            guard let b = bindings[a], !b.isEmpty else { continue }
            guard let s = SnapShortcut(binding: b) else { refused[a] = "unknown keys"; continue }
            guard s.isValid else { refused[a] = "needs ⌘, ⌥ or ⌃"; continue }
            out[a] = s
        }
        // Spectacle refuses a shortcut another action already uses; both sides lose theirs here so neither silently wins.
        var seen: [SnapShortcut: [SnapAction]] = [:]
        for (a, s) in out { seen[s, default: []].append(a) }
        for (_, actions) in seen where actions.count > 1 {
            for a in actions {
                out[a] = nil
                refused[a] = "same keys as \(actions.filter { $0 != a }.map(\.label).sorted().joined(separator: ", "))"
            }
        }
        return Resolved(shortcuts: out, refused: refused)
    }
}

/// Carbon virtual key codes (kVK_*), for the key names a binding may use.
public enum SnapKeyCodes {
    static let table: [String: UInt32] = {
        var t: [String: UInt32] = [
            "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13,
            "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25,
            "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "return": 36,
            "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47,
            "tab": 48, "space": 49, "`": 50, "delete": 51, "escape": 53, "forwarddelete": 117, "home": 115,
            "end": 119, "pageup": 116, "pagedown": 121, "left": 123, "right": 124, "down": 125, "up": 126,
        ]
        let f: [UInt32] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90]
        for (i, c) in f.enumerated() { t["f\(i + 1)"] = c }
        return t
    }()

    public static func code(for key: String) -> UInt32? { table[key] }
}
