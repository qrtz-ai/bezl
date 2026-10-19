// ctrl do's stop key, platform-neutral (ADR 0214): while ctrl acts in the foreground, Esc stops the run and any other
// physical key press, click or scroll stops it too (Jared took over). The macOS backend feeds it from a listen-only
// event tap; events ctrl posts itself carry `ctrlMarker` in their source user data and are ignored. If the tap can't
// start (no permission), the watcher reports `armed: false` and the host doesn't act.
import Foundation
import QrtzShared

public final class CtrlStopWatch: @unchecked Sendable {
    public enum Input: Equatable, Sendable {
        case keyDown(code: UInt16, marker: Int64)
        case mouse(marker: Int64)
        case scroll(marker: Int64)
        /// The system turned the tap off (timeout or user input disabled): treated as a stop.
        case tapDisabled
    }

    /// Stamped on every event ctrl posts (`kCGEventSourceUserData`), so the watcher can tell them apart.
    public static let ctrlMarker: Int64 = CtrlEventMarker.value // "qrtzctrl"

    private let lock = NSLock()
    private var armed = false
    private var stopped = false
    private var userInput = false
    /// A run is being watched (from the first arm to `disarm`). Review C1: the host re-arms before every action,
    /// so the flags reset only on the disarmed→armed edge; a stop stays latched for the whole run.
    private var session = false

    public init() {}

    /// Start watching. `started` is whether the tap is running (and allowed: permission, no secure input). The
    /// first arm of a run clears the flags; a re-arm inside the run never does (C1), and can't revive a watcher
    /// the system turned off (M6).
    public func arm(started: Bool) {
        lock.lock(); defer { lock.unlock() }
        if !session {
            session = true; stopped = false; userInput = false; armed = started
        } else if !started {
            armed = false
        }
    }

    /// The run ended: the next arm starts clean.
    public func disarm() {
        lock.lock(); defer { lock.unlock() }
        armed = false; session = false
    }

    /// Review (d): secure event input is on (a password prompt): the tap sees nothing, so it counts as unarmed.
    public func lost() {
        lock.lock(); defer { lock.unlock() }
        armed = false
    }

    public func record(_ input: Input) {
        lock.lock(); defer { lock.unlock() }
        guard armed else { return }
        switch input {
        case let .keyDown(code, marker):
            guard marker != Self.ctrlMarker else { return }
            if code == CtrlKeys.escape { stopped = true } else { userInput = true }
        case let .mouse(marker), let .scroll(marker):
            if marker != Self.ctrlMarker { userInput = true }
        case .tapDisabled:
            // M6: unarmed at once (a re-arm in this run can't revive it) and the run stops.
            stopped = true; armed = false
        }
    }

    /// Review C2: the helper refuses to act once anything tripped (or nothing watches).
    public var mayAct: Bool {
        lock.lock(); defer { lock.unlock() }
        return armed && !stopped && !userInput
    }

    public var flags: (armed: Bool, stopped: Bool, userInput: Bool) {
        lock.lock(); defer { lock.unlock() }
        return (armed, stopped, userInput)
    }
}
