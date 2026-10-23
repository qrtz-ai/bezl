// qrtz snap's "summon agent": hold left ⌘ and right ⌘ together (nothing else) and snap brings the chosen app to the
// front, or, when it already is, sends you back to the app you were in. Foundation-only, so the chord decision and the
// toggle are unit tested on Linux; mac/SnapSummonMac.swift is the listen-only CGEvent tap and NSWorkspace side.
import Foundation

/// The chord's decision half: fed every `flagsChanged` and `keyDown` the session tap sees.
/// Fires once, when the chord ends (the first ⌘ let go), and only if both ⌘ keys were down together with no other
/// modifier and no key pressed in between, so ⌘C, ⌘⇧4 and the like never summon. Synthetic events (not from the
/// keyboard) are ignored, so another app's automation can't trigger it.
public struct SummonChordFilter: Equatable, Sendable {
    /// CGEventType raw values.
    public static let keyDown: UInt32 = 10, flagsChanged: UInt32 = 12
    /// kVK_Command and kVK_RightCommand.
    public static let leftCmdKeyCode: Int64 = 55, rightCmdKeyCode: Int64 = 54
    /// Device-dependent bits (NX_DEVICELCMDKEYMASK, NX_DEVICERCMDKEYMASK) in CGEventFlags.
    public static let leftCmdDevice: UInt64 = 0x08, rightCmdDevice: UInt64 = 0x10
    /// Shift, control, option and fn: any of them spoils the chord.
    public static let otherModifiers: UInt64 = 0x2_0000 | 0x4_0000 | 0x8_0000 | 0x80_0000
    /// `CGEventSourceStateID.hidSystemState`.
    public static let hidSystemState: Int64 = 1

    public private(set) var left = false
    public private(set) var right = false
    /// Both were down together this chord.
    public private(set) var armed = false
    /// A key or another modifier joined in: this chord fires nothing.
    public private(set) var spoiled = false
    /// Echo's push-to-talk key is right ⌘ (summonClash): the chord never arms, so a right-⌘-first press that started
    /// dictation can't also summon. Survives `reset()`.
    public var blocked = false

    public init(blocked: Bool = false) { self.blocked = blocked }

    /// True exactly when the chord ends cleanly (summon now).
    public mutating func handle(type: UInt32, keyCode code: Int64, flags: UInt64,
                                sourceState: Int64 = SummonChordFilter.hidSystemState) -> Bool {
        guard !blocked, sourceState == Self.hidSystemState else { return false }
        if type == Self.keyDown {
            if left || right { spoiled = true }
            return false
        }
        guard type == Self.flagsChanged else { return false }
        left = flags & Self.leftCmdDevice != 0
        right = flags & Self.rightCmdDevice != 0
        if flags & Self.otherModifiers != 0, left || right { spoiled = true }
        if code != Self.leftCmdKeyCode && code != Self.rightCmdKeyCode, left || right { spoiled = true }
        if left && right { armed = true }
        var fire = false
        if armed, !(left && right) {
            // The first ⌘ let go: the chord is over. Fire once; pressing it again while the other ⌘ is still held
            // is a new chord, so tapping one ⌘ toggles back and forth.
            fire = !spoiled
            armed = false
        }
        if !left && !right {
            spoiled = false
            armed = false
        }
        return fire
    }

    /// The tap was disabled or the Mac slept: forget everything, fire nothing.
    public mutating func reset() { self = SummonChordFilter(blocked: blocked) }
}

/// What a summon does, given who's in front.
public enum SummonStep: Equatable {
    /// Bring the chosen app forward (launching it if it isn't running); remember `previous` to go back to.
    case summon(previous: String?)
    /// The chosen app is in front: go back to `previous` (and hide the chosen app).
    case dismiss(back: String?)
}

public enum SnapSummon {
    public static func step(frontmost: String?, chosen: String, previous: String?) -> SummonStep {
        if let f = frontmost, f == chosen { return .dismiss(back: previous) }
        return .summon(previous: frontmost)
    }

    /// The webview can write config, so a bundle id is only ever activated or launched when it is one of the
    /// installed apps the picker offered. Anything else: beep, do nothing.
    public static func allowed(_ bundleId: String, installed: [SnapApp]) -> Bool {
        !bundleId.isEmpty && installed.contains { $0.bundleId == bundleId }
    }
}

/// An app the picker can offer.
public struct SnapApp: Equatable, Sendable {
    public var bundleId: String
    public var name: String
    public init(bundleId: String, name: String) { self.bundleId = bundleId; self.name = name }
}

/// The summon part of `snap.configure`.
public struct SnapSummonConfig: Equatable, Sendable {
    public var enabled = false
    public var bundleId = ""
    /// Echo's push-to-talk key, so snap can say when right ⌘ is also Echo's.
    public var echoKey = ""
    public init(enabled: Bool = false, bundleId: String = "", echoKey: String = "") {
        self.enabled = enabled; self.bundleId = bundleId; self.echoKey = echoKey
    }
    public var clashesWithEcho: Bool { echoKey == "right-command" }

    init(json o: [String: Any]?) {
        guard let o else { return }
        enabled = o["enabled"] as? Bool ?? false
        bundleId = (o["bundleId"] as? String ?? "").trimmingCharacters(in: .whitespaces)
        echoKey = o["echoKey"] as? String ?? ""
    }
}
