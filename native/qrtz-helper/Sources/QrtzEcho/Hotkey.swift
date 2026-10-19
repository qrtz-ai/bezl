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
    /// `CGEventSourceStateID.hidSystemState`: an event from real hardware.
    public static let hidSystemState: Int64 = 1
    /// The source user data ctrl's own posted events carry: the one `CtrlEventMarker` in QrtzShared, which
    /// `CtrlStopWatch.ctrlMarker` also is.
    public static let ctrlMarker: Int64 = CtrlEventMarker.value

    public let key: String
    public let keyCode: Int64
    public let flag: UInt64
    public let consumeFn: Bool
    public private(set) var down = false
    /// ADR 0216: this press began with ⌃ held (ctrl+fn → ctrl do). Read at the down edge only, kept until the up
    /// edge, so ⌃ let go first still ends as a ctrl press, and ⌃ pressed after fn is still dictation. fn only.
    public private(set) var withControl = false

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
    /// Review M-E: ⌃ makes a ctrl press only when the event came from the keyboard (`sourceState` is the HID
    /// system state) and isn't one ctrl posted (`userData` is not `ctrlMarker`): a synthetic ⌃ (ctrl's own
    /// key combo, another app's automation) never routes speech to ctrl do.
    public mutating func handle(type: UInt32, keyCode code: Int64, flags: UInt64, sourceState: Int64 = HotkeyFilter.hidSystemState,
                                userData: Int64 = 0) -> (action: HotkeyAction?, consume: Bool) {
        var action: HotkeyAction?
        if type == Self.flagsChanged, code == keyCode {
            let isDown = flags & flag != 0
            if isDown != down {
                down = isDown
                action = isDown ? .down : .up
                if isDown {
                    withControl = key == "fn" && flags & Self.controlFlag != 0 && sourceState == Self.hidSystemState
                        && userData != Self.ctrlMarker
                }
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
        return down ? .up : nil  // `withControl` stays for this release; the next press sets it again
    }

    /// N196 (ADR 0224): the Mac is going to sleep (lid closed while holding the key). The release may never reach
    /// the tap, so a held key ends here, like a disabled tap.
    public mutating func willSleep() -> HotkeyAction? { reset() }

    /// N196: the Mac woke. `flags` is the session's current modifier state (`CGEventSource.flagsState`); a key
    /// still believed held whose flag is clear was released while asleep (or the sleep notice never came).
    public mutating func didWake(flags: UInt64) -> HotkeyAction? {
        guard down, flags & flag == 0 else { return nil }
        return reset()
    }
}
