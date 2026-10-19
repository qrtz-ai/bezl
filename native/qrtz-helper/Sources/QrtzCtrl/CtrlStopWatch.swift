// ctrl do's stop key, platform-neutral (ADR 0214): while ctrl acts in the foreground, Esc stops the run and any other
// physical key press, click or scroll stops it too (Jared took over). The macOS backend feeds it from a listen-only
// event tap; events ctrl posts itself carry `ctrlMarker` in their source user data and are ignored. If the tap can't
// start (no permission), the watcher reports `armed: false` and the host doesn't act.
import Foundation

public final class CtrlStopWatch: @unchecked Sendable {
    public enum Input: Equatable, Sendable {
        case keyDown(code: UInt16, marker: Int64)
        case mouse(marker: Int64)
        case scroll(marker: Int64)
        /// The system turned the tap off (timeout or user input disabled): treated as a stop.
        case tapDisabled
    }

    /// Stamped on every event ctrl posts (`kCGEventSourceUserData`), so the watcher can tell them apart.
    public static let ctrlMarker: Int64 = 0x7172_747A_6374_726C // "qrtzctrl"

    private let lock = NSLock()
    private var armed = false
    private var stopped = false
    private var userInput = false

    public init() {}

    /// Start (or restart) watching: the flags reset. `started` is whether the tap is running.
    public func arm(started: Bool) {
        lock.lock(); defer { lock.unlock() }
        armed = started; stopped = false; userInput = false
    }

    public func disarm() {
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
            stopped = true
        }
    }

    public var flags: (armed: Bool, stopped: Bool, userInput: Bool) {
        lock.lock(); defer { lock.unlock() }
        return (armed, stopped, userInput)
    }
}
