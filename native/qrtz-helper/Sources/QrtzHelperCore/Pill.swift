import Foundation

// ADR 0092: echo's dictation pill drawn by the helper (a native panel), driven by the helper's own talk state.
// This file is the platform-free part: the state machine, the wave math (the same as packages/ui/src/Waveform.tsx,
// which PillTests checks), the geometry and the words. mac/Pill.swift draws it.

/// What the pill shows. Finalizing and inserting both draw "processing" (the slow swell).
public enum PillPhase: Equatable, Sendable {
    case hidden, listening, processing
    case error(String)

    /// The accessibility label (the pill draws no words outside an error).
    public var accessibilityLabel: String {
        switch self {
        case .hidden: "echo"
        case .listening: "listening"
        case .processing: "processing"
        case .error(let m): m
        }
    }
}

/// The platform view. Called from any thread; the implementation hops to the main thread.
public protocol PillView: AnyObject, Sendable {
    func show(_ phase: PillPhase)
    /// Mic RMS 0..1, about 20 Hz while listening.
    func level(_ rms: Double)
}

/// Follows the helper's talk flow: the push key and the dictation lines it sends, and the host's stop, cancel and
/// insert. Up through finalizing and inserting; hidden on insert, on an empty or cancelled dictation, or after an
/// error has shown for `errorFlashMs`. Timeouts hide a pill whose dictation never started or whose text was never
/// inserted, so it can't stick on screen.
public final class PillDriver: @unchecked Sendable {
    public enum State: Equatable, Sendable { case hidden, listening, finalizing, inserting, error(String) }

    public struct Timing: Sendable {
        public var errorFlashMs = 1_600
        public var startTimeoutMs = 2_000
        public var insertTimeoutMs = 5_000
        public init() {}
    }

    private let view: PillView
    private let timing: Timing
    private let lock = NSLock()
    private var current: State = .hidden
    private var gen = 0
    private var started = false

    public init(view: PillView, timing: Timing = Timing()) { self.view = view; self.timing = timing }

    public var state: State { lock.withLock { current } }

    /// A line the helper sends (seen before it is written, so a key press shows the pill at once).
    public func helper(_ m: HelperMessage) {
        switch m {
        case .hotkey(.down):
            let s = state
            if s == .hidden || isError(s) { lock.withLock { started = false }; go(.listening, timeoutMs: timing.startTimeoutMs) }
        case .hotkey(.up):
            if state == .listening { go(.finalizing) }
        case .dictationStarted:
            lock.withLock { started = true }
            let s = state
            if s == .hidden || s == .listening || isError(s) { go(.listening) }
        case .dictationLevel(let rms):
            if state == .listening { view.level(rms) }
        case .dictationStopped(_, let text, _, let reason):
            if let reason { go(.error(Self.words(reason)), timeoutMs: timing.errorFlashMs) }
            else if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { go(.hidden) }
            else { go(.inserting, timeoutMs: timing.insertTimeoutMs) }
        case .insertResult(_, _, let ok, _, let error):
            if ok { go(.hidden) } else { go(.error(Self.lower(error ?? "couldn't insert the text")), timeoutMs: timing.errorFlashMs) }
        case .error(let code, let message, _, _, _, _, _):
            // Only while the pill is up (a configure-time hotkey error must not pop it), and a refused second press
            // (`dictation.busy`) leaves the first dictation's pill alone.
            guard code.hasPrefix("dictation."), code != "dictation.busy", state != .hidden else { return }
            go(.error(Self.lower(message)), timeoutMs: timing.errorFlashMs)
        default: break
        }
    }

    /// A line the host sends.
    public func host(_ m: HostMessage) {
        switch m {
        case .dictationStart:
            if state == .hidden { go(.listening, timeoutMs: timing.startTimeoutMs) }
        case .dictationStop:
            if state == .listening { go(.finalizing) }
        case .dictationCancel:
            go(.hidden)
        case .insertText:
            if state != .inserting { go(.inserting, timeoutMs: timing.insertTimeoutMs) }
        default: break
        }
    }

    public static func phase(_ s: State) -> PillPhase {
        switch s {
        case .hidden: .hidden
        case .listening: .listening
        case .finalizing, .inserting: .processing
        case .error(let m): .error(m)
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

    private func isError(_ s: State) -> Bool { if case .error = s { true } else { false } }

    private func go(_ s: State, timeoutMs: Int? = nil) {
        let (g, changed): (Int, Bool) = lock.withLock {
            let changed = Self.phase(current) != Self.phase(s)
            current = s; gen += 1
            return (gen, changed)
        }
        if changed { view.show(Self.phase(s)) }
        guard let ms = timeoutMs else { return }
        Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(ms) * 1_000_000)
            guard let self else { return }
            let stale: Bool = self.lock.withLock {
                // A start timeout only fires if the dictation never started.
                self.gen != g || (s == .listening && self.started)
            }
            if !stale { self.go(.hidden) }
        }
    }
}

/// The wave, as in packages/ui/src/Waveform.tsx: three layered sines under a tapered envelope, the amplitude eased
/// toward the mic level each display frame (fast attack, slower release), a slow low swell while processing.
public enum PillWave {
    public enum Mode: Sendable { case live, idle }
    public static let liveFloor = 0.06
    public static let idleAmp = 0.14
    public static let attackMs = 60.0
    public static let releaseMs = 220.0
    public static func speed(_ m: Mode) -> Double { m == .live ? 7 : 2.2 }
    public struct Layer: Sendable { public let cycles, gain, opacity, rate, offset: Double }
    public static let layers = [
        Layer(cycles: 1.6, gain: 1, opacity: 1, rate: 1, offset: 0),
        Layer(cycles: 2.3, gain: 0.66, opacity: 0.5, rate: -1.3, offset: 1.9),
        Layer(cycles: 3.1, gain: 0.4, opacity: 0.28, rate: 1.7, offset: 4.1),
    ]
    public static let width = 160.0, height = 24.0, points = 48

    static func clamp01(_ x: Double) -> Double { x.isFinite ? min(1, max(0, x)) : 0 }

    public static func target(_ mode: Mode, _ level: Double) -> Double {
        mode == .idle ? idleAmp : liveFloor + (1 - liveFloor) * clamp01(level).squareRoot()
    }

    /// One frame of exponential smoothing toward `target` over `dtMs`; frame-rate independent.
    public static func step(_ cur: Double, _ target: Double, _ dtMs: Double) -> Double {
        let dt = max(0, min(dtMs, 250))
        let tau = target > cur ? attackMs : releaseMs
        return target + (cur - target) * exp(-dt / tau)
    }

    public static func envelope(_ u: Double) -> Double { pow(sin(Double.pi * clamp01(u)), 1.6) }

    /// One layer's points, y measured from the top like the SVG (the view flips).
    public static func layerPoints(amp: Double, phase: Double, cycles: Double, width: Double = width, height: Double = height) -> [(x: Double, y: Double)] {
        let mid = height / 2, a = clamp01(amp) * (mid - 1)
        return (0...points).map { i in
            let u = Double(i) / Double(points)
            return (u * width, mid + a * envelope(u) * sin(2 * Double.pi * cycles * u + phase))
        }
    }

    public static func frame(amp: Double, phase: Double) -> [[(x: Double, y: Double)]] {
        layers.map { layerPoints(amp: amp * $0.gain, phase: phase * $0.rate + $0.offset, cycles: $0.cycles) }
    }
}

/// Where and how big: 240 x 52 pt (the web pill's window), bottom-center of the active window's screen, 24 pt above
/// the bottom of its visible frame (above the Dock).
public enum PillGeometry {
    public static let width = 240.0, height = 52.0, bottomGap = 24.0, padding = 16.0, gap = 8.0, iconSize = 16.0
    public static let fontSize = 13.158

    /// Cocoa coordinates (origin bottom-left).
    public static func frame(visibleX x: Double, y: Double, width w: Double, height _: Double) -> (x: Double, y: Double, w: Double, h: Double) {
        (x + ((w - width) / 2).rounded(), y + bottomGap, width, height)
    }

    /// Room for the error text: the pill minus its padding, the icon and the gap.
    public static var errorTextWidth: Double { width - 2 * padding - iconSize - gap }
}
