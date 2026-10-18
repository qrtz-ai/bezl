import Foundation
import QrtzShared

/// The decision half of the native push-to-talk key monitor, kept free of CoreGraphics so it is tested on any
/// platform. `MacHotkeys` (mac/Talk.swift) feeds it every event its session-level CGEvent tap sees and drops the
/// ones it says to consume.
///
/// Consuming: with `fn` as the talk key, macOS also runs its own Fn/globe action on release (switch input source,
/// emoji picker, dictation; System Settings → Keyboard → "Press 🌐 key to"). With `consumeFn` on (the default,
/// `talk.blockGlobeKey`) the Fn/globe events are swallowed, but only while `fn` is the talk key: Fn's
/// `flagsChanged` (key code 63) and the globe key's own key down/up (key code 179, sent by Apple globe
/// keyboards). Every other event passes through, Fn+key combinations included (they carry the Fn flag on the key
/// event itself), and the other talk keys (right ⌥/⌘/⌃) are never consumed: they are ordinary modifiers.
public struct HotkeyFilter: Equatable, Sendable {
    /// CGEventType raw values.
    public static let keyDown: UInt32 = 10, keyUp: UInt32 = 11, flagsChanged: UInt32 = 12
    /// kVK_Function and the globe key's key code.
    public static let fnKeyCode: Int64 = 63, globeKeyCode: Int64 = 179
    /// CGEventFlags raw values.
    public static let fnFlag: UInt64 = 0x80_0000, optionFlag: UInt64 = 0x8_0000, commandFlag: UInt64 = 0x10_0000,
        controlFlag: UInt64 = 0x4_0000

    public let key: String
    public let keyCode: Int64
    public let flag: UInt64
    public let consumeFn: Bool
    public private(set) var down = false

    /// nil for a key this monitor doesn't support.
    public init?(key: String, consumeFn: Bool) {
        switch key {
        case "fn": (keyCode, flag) = (Self.fnKeyCode, Self.fnFlag)
        case "right-option": (keyCode, flag) = (61, Self.optionFlag)
        case "right-command": (keyCode, flag) = (54, Self.commandFlag)
        case "right-control": (keyCode, flag) = (62, Self.controlFlag)
        default: return nil
        }
        self.key = key
        self.consumeFn = consumeFn
    }

    /// True when the tap has to be able to drop events (an active, not listen-only, tap: needs Accessibility).
    public var consumes: Bool { consumeFn && key == "fn" }

    /// What one event means: a push-to-talk edge (`action`), and whether the tap drops the event.
    public mutating func handle(type: UInt32, keyCode code: Int64, flags: UInt64) -> (action: HotkeyAction?, consume: Bool) {
        var action: HotkeyAction?
        if type == Self.flagsChanged, code == keyCode {
            let isDown = flags & flag != 0
            if isDown != down {
                down = isDown
                action = isDown ? .down : .up
            }
        }
        guard consumes else { return (action, false) }
        let fnEvent = (type == Self.flagsChanged && code == Self.fnKeyCode)
            || ((type == Self.keyDown || type == Self.keyUp) && code == Self.globeKeyCode)
        return (action, fnEvent)
    }

    /// The tap was disabled (timeout or user input): forget a held key so the next press starts clean.
    public mutating func reset() -> HotkeyAction? {
        defer { down = false }
        return down ? .up : nil
    }
}
