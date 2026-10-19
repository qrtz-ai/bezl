import Foundation
import QrtzShared

// ADR 0092: echo's dictation pill drawn by the helper (a native panel), driven by the helper's own talk state.
// This file is the platform-free part: the state machine, the wave math (the same as packages/ui/src/Waveform.tsx,
// which PillTests checks), the geometry and the words. mac/Pill.swift draws it.

/// What the pill shows. Finalizing and inserting both draw "processing" (the slow swell).
public enum PillPhase: Equatable, Sendable {
    case hidden, listening, processing
    case error(String)
    /// A notice, not a failure (the host's "copied to clipboard"): the text without the alert icon.
    case info(String)

    /// The accessibility label (the pill draws no words outside an error).
    public var accessibilityLabel: String {
        switch self {
        case .hidden: "echo"
        case .listening: "listening"
        case .processing: "processing"
        case .error(let m), .info(let m): m
        }
    }
}

/// The platform view. Called from any thread; the implementation hops to the main thread.
public protocol PillView: AnyObject, Sendable {
    func show(_ phase: PillPhase)
    /// Mic RMS 0..1, about 20 Hz while listening.
    func level(_ rms: Double)
    /// ADR 0221: a host `notch.show` / `notch.update` / `notch.dismiss` for the shared notch surface the pill draws on.
    func notch(_ m: HostMessage)
}

extension PillView {
    public func notch(_ m: HostMessage) {}
}

/// Follows the helper's talk flow: the push key and the dictation lines it sends, and the host's start, stop, cancel
/// and insert. Up through finalizing and inserting; hidden on insert, on an empty or cancelled dictation, or after an
/// error has shown for `errorFlashMs`. Timeouts hide a pill whose dictation never started, never finished or whose
/// text was never inserted, so it can't stick on screen.
///
/// N146: the pill follows one dictation at a time. A press starts a new attempt (also during an older dictation's
/// finalizing or inserting, N110); the older dictation's id is retired and its later lines (stopped, insert.result,
/// errors) are ignored, so they can't hide the newer one's pill.
public final class PillDriver: @unchecked Sendable {
    public enum State: Equatable, Sendable { case hidden, listening, finalizing, inserting, error(String), info(String) }

    public struct Timing: Sendable {
        public var errorFlashMs = 1_600
        /// From the press until the host's `dictation.start` (N147: only that long; the recogniser's load after it,
        /// up to ~26 s cold, doesn't hide the pill while the key is held).
        public var startTimeoutMs = 2_000
        public var insertTimeoutMs = 5_000
        /// After a failed insert: how long the pill waits for the host's `pill.flash` (it knows whether the words
        /// went to the clipboard) before it shows the helper's own error.
        public var hostFlashWaitMs = 400
        /// N145: the most a started dictation's release can take (`DictationTail`: tail + the whole drain/finalize
        /// budget, plus a second). nil: the controller fills it from its `DictationTail`.
        public var finalizeBackstopMs: Int?
        public init() {}
    }

    private let view: PillView
    private let timing: Timing
    private let lock = NSLock()
    private var current: State = .hidden
    private var gen = 0
    private var started = false
    /// The host sent `dictation.start` for this attempt.
    private var hostAsked = false
    private var currentId: String?
    private var retired: [String] = []
    private var on = true
    private var inserts = true
    private var pendingInsertError: String?

    /// `talk.showPill` (`dictation.configure{pill}`). Off: nothing shows (the host notifies instead).
    public var enabled: Bool {
        get { lock.withLock { on } }
        set {
            lock.withLock { on = newValue }
            if !newValue { go(.hidden) }
        }
    }

    /// `talk.autoInsert` (`dictation.configure{autoInsert}`). Off: the host inserts nothing, so the pill ends at
    /// `dictation.stopped` instead of waiting out the insert timeout (N147).
    public var autoInsert: Bool {
        get { lock.withLock { inserts } }
        set { lock.withLock { inserts = newValue } }
    }

    /// Runs `fire` after `ms` (a timeout). Tests pass a manual clock so no test races the wall clock.
    public typealias After = @Sendable (_ ms: Int, _ fire: @escaping @Sendable () -> Void) -> Void
    public static let wallClock: After = { ms, fire in
        Task {
            try? await Task.sleep(nanoseconds: UInt64(max(0, ms)) * 1_000_000)
            fire()
        }
    }
    private let after: After

    public init(view: PillView, timing: Timing = Timing(), after: @escaping After = PillDriver.wallClock) {
        self.view = view; self.timing = timing; self.after = after
    }

    public var state: State { lock.withLock { current } }
    /// The dictation the pill follows (nil: a press the host hasn't started yet).
    public var dictationId: String? { lock.withLock { currentId } }

    private var backstopMs: Int { timing.finalizeBackstopMs ?? DictationTail().tailMs + DictationTail().maxWaitMs + 1_000 }

    /// This id's lines move the pill: not an older, retired dictation's.
    private func isCurrent(_ id: String) -> Bool {
        lock.withLock { !retired.contains(id) && (currentId == nil || currentId == id) }
    }

    /// A new attempt: the dictation the pill followed (if any) is retired.
    private func newAttempt(id: String?) {
        lock.withLock {
            if let c = currentId, c != id {
                retired.append(c)
                if retired.count > 8 { retired.removeFirst(retired.count - 8) }
            }
            currentId = id; started = false; hostAsked = false
        }
    }

    /// A line the helper sends (seen before it is written, so a key press shows the pill at once).
    public func helper(_ m: HelperMessage) {
        switch m {
        case .hotkey(.down, _):
            let s = state
            guard s != .listening else { return }
            newAttempt(id: nil)
            // N151/N157: a press during an older dictation's finalizing or inserting is queued by talk (N110) until
            // that dictation is done, so its dictation.start can come as late as the release budget, not within 2 s.
            go(.listening, timeoutMs: s == .finalizing || s == .inserting ? backstopMs : timing.startTimeoutMs)
        case .hotkey(.up, _):
            guard state == .listening else { return }
            // N145: a press the host never started (refused before dictation.start) can't wait for a stop.
            go(.finalizing, timeoutMs: lock.withLock { hostAsked || started } ? backstopMs : timing.startTimeoutMs)
        case .dictationStarted(let id, _):
            // A started dictation is the newest one, unless it is a retired id.
            guard lock.withLock({ !retired.contains(id) }) else { return }
            if lock.withLock({ currentId != nil && currentId != id }) { newAttempt(id: id) }
            lock.withLock { currentId = id; started = true }
            let s = state
            if s == .hidden || s == .listening || isError(s) { go(.listening) }
            else if s == .finalizing { go(.finalizing, timeoutMs: backstopMs) }
        case .dictationLevel(let rms):
            if state == .listening { view.level(rms) }
        case .dictationStopped(let id, let text, _, let reason, _):
            guard isCurrent(id) else { return }
            if let reason { go(.error(Self.words(reason)), timeoutMs: timing.errorFlashMs) }
            else if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !autoInsert { go(.hidden) }
            else { go(.inserting, timeoutMs: timing.insertTimeoutMs) }
        case .insertResult(let id, _, let ok, _, let error):
            guard isCurrent(id) else { return }
            if ok { go(.hidden); return }
            // The host answers a failed insert with `pill.flash` (error, or info "copied to clipboard"): stay on
            // processing for it; show this error only if it doesn't come.
            let msg = error == SecureField.insertRefused ? "echo won't type into password fields"
                : Self.lower(error ?? "couldn't insert the text")
            let g: Int = lock.withLock { pendingInsertError = msg; return gen }
            after(timing.hostFlashWaitMs) { [weak self] in
                guard let self else { return }
                let m: String? = self.lock.withLock { self.gen == g ? self.pendingInsertError : nil }
                if let m { self.go(.error(m), timeoutMs: self.timing.errorFlashMs) }
            }
        case .error(let code, let message, _, let id, _, _, _, _):
            // Only while the pill is up (a configure-time hotkey error must not pop it), and only for the dictation
            // it follows. A refused press (`dictation.busy`) has nothing to show: it ends quietly.
            guard code.hasPrefix("dictation."), state != .hidden else { return }
            if let id, !isCurrent(id) { return }
            if code == "dictation.busy" { go(.hidden); return }
            go(.error(Self.lower(message)), timeoutMs: timing.errorFlashMs)
        default: break
        }
    }

    /// A line the host sends.
    public func host(_ m: HostMessage) {
        switch m {
        case .dictationStart(let id):
            if lock.withLock({ currentId != nil && currentId != id }) { newAttempt(id: id) }
            lock.withLock { currentId = id; hostAsked = true }
            // N147: the host answered, so the start timeout is disarmed (same phase, no redraw).
            switch state {
            case .listening, .hidden, .error, .info: go(.listening)
            case .finalizing: go(.finalizing, timeoutMs: backstopMs)
            case .inserting: break
            }
        case .dictationStop(let id):
            if isCurrent(id), state == .listening { go(.finalizing, timeoutMs: backstopMs) }
        case .dictationCancel(let id):
            if isCurrent(id) { go(.hidden) }
        case .insertText(let id, _, _):
            if isCurrent(id), state != .inserting { go(.inserting, timeoutMs: timing.insertTimeoutMs) }
        case .pillFlash(let kind, let message):
            let m = Self.lower(message)
            go(kind == .error ? .error(m) : .info(m), timeoutMs: timing.errorFlashMs)
        case .notchShow, .notchUpdate, .notchDismiss:
            // ADR 0221: host-owned content (ctrl run status, module requests); the surface arbitrates against dictation.
            view.notch(m)
        default: break
        }
    }

    public static func phase(_ s: State) -> PillPhase {
        switch s {
        case .hidden: .hidden
        case .listening: .listening
        case .finalizing, .inserting: .processing
        case .error(let m): .error(m)
        case .info(let m): .info(m)
        }
    }

    static func words(_ r: DictationStopReason) -> String {
        switch r {
        case .permissionRevoked: "microphone access was turned off"
        case .micLost: "the microphone went away"
        }
    }

    static func lower(_ s: String) -> String {
        let t = s.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return t.isEmpty ? "something went wrong" : t
    }

    private func isError(_ s: State) -> Bool {
        switch s { case .error, .info: true; default: false }
    }

    private func go(_ s: State, timeoutMs: Int? = nil) {
        let (g, changed): (Int, Bool) = lock.withLock {
            // Off (`talk.showPill` false): track nothing, show nothing.
            let s = on ? s : .hidden
            let changed = Self.phase(current) != Self.phase(s)
            current = s; gen += 1; pendingInsertError = nil
            return (gen, changed)
        }
        if changed { view.show(Self.phase(s)) }
        guard let ms = timeoutMs else { return }
        after(ms) { [weak self] in
            guard let self else { return }
            let stale: Bool = self.lock.withLock {
                // A start timeout only fires if the dictation never started.
                self.gen != g || (s == .listening && self.started)
            }
            if !stale { self.go(.hidden) }
        }
    }
}

// PillWave (the wave math) moved to QrtzShared/PillWave.swift (ADR 0221): the notch surface draws it.
