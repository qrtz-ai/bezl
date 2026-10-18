import Foundation
import QrtzShared

// N143: echo's dictation hears what the speakers play. Without headphones the mic picks up the system audio (a video,
// a call), and the recogniser types it. While dictating, the mic is gated against the system audio: the meeting's
// system channel when a recording runs, otherwise a system tap that runs for the dictation's length only.

/// The dictation side of N143. Owned by the dictation engine (its actor calls everything here).
public final class DictationEcho: @unchecked Sendable {
    public let reference: EchoReference
    let tap: SystemAudioTap?
    let permitted: @Sendable () -> Bool
    let gate = DictationEchoGate()
    var tapTask: Task<Void, Never>?
    var tapToken: Int?
    var lastStop: Task<Void, Never>?
    var blocks = 0, ducked = 0
    public var onLog: (@Sendable (LogLevel, String) -> Void)?
    /// N153: `talk.echoGate` (`dictation.configure{echoGate}`). Off: no gate and no tap, the raw mic.
    public var enabled = true
    /// N158: `enabled` as it was at `begin()`, until `end()`: a settings change mid-dictation applies from the next
    /// dictation, so a running one keeps its gate (and its tap). nil outside a dictation.
    private var latched: Bool?
    /// ADR 0150 N173: how this dictation's mic is gated, for the timing line (`gate=`): `meeting`, `tap`, or off with
    /// its reason: `off-setting`, `off-no-grant` (no screen & system audio grant on this app: the split echo needs
    /// its own), `off-quiet` (headphones or nothing playing), `off-no-tap` (no tap on this platform), `off-tap-failed`.
    /// "" before the first dictation.
    public var mode: String { modeState.value }
    private let modeState = ModeState()
    final class ModeState: @unchecked Sendable {
        private let lock = NSLock()
        private var started = ""
        private var failed = false
        var value: String { lock.withLock { failed ? "off-tap-failed" : started } }
        func set(_ m: String) { lock.withLock { started = m; failed = false } }
        func tapFailed() { lock.withLock { failed = true } }
    }

    public init(reference: EchoReference, tap: SystemAudioTap?, permitted: @escaping @Sendable () -> Bool) {
        self.reference = reference; self.tap = tap; self.permitted = permitted
    }

    /// At dictation.start. Returns the log line saying how this dictation's mic is gated (nil: no tap on this
    /// platform, nothing to say).
    @discardableResult
    public func begin() -> String? {
        blocks = 0; ducked = 0
        latched = enabled
        func mode(_ m: String) { modeState.set(m) }
        guard enabled else { mode("off-setting"); return nil }
        // In a meeting the warm mic's blocks already taught the gate the echo path: keep it.
        if reference.has("recording") { mode("meeting"); return "echo gate: the meeting's system audio" }
        gate.reset()
        guard let tap else { mode("off-no-tap"); return nil }
        guard permitted() else { mode("off-no-grant"); return "echo gate: off (no system audio grant, raw mic)" }
        guard tap.echoLikely() else { mode("off-quiet"); return "echo gate: off (headphones or nothing playing)" }
        mode("tap")
        let token = reference.attach("dictation")
        tapToken = token
        let ref = reference, prev = lastStop, log = onLog, ms = modeState
        // The tap starts in the background (tens of ms): the mic is not held for it; blocks before its first
        // samples pass ungated.
        tapTask = Task {
            await prev?.value
            do { try await tap.start(onSamples: { ref.push($0, from: "dictation") }) } catch {
                ref.detach("dictation", token: token)  // N152: never the next dictation's hold
                ms.tapFailed()
                log?(.warn, "echo gate: \(tap.name) didn't start (\(error)), raw mic")
            }
        }
        return "echo gate: \(tap.name) for this dictation"
    }

    /// At the dictation's end. The tap stops in the background; a quick re-press's start waits for it.
    public func end() {
        if blocks > 0 { onLog?(.info, "echo gate: \(ducked) of \(blocks) mic blocks held down") }
        blocks = 0; ducked = 0
        latched = nil
        guard let t = tapTask, let tap else { return }
        tapTask = nil
        reference.detach("dictation", token: tapToken)
        lastStop = Task { await t.value; await tap.stop() }
    }

    /// One live mic block, gated when a tap feeds the reference.
    public func process(_ mic: [Float]) -> [Float] {
        guard latched ?? enabled, let sys = reference.take(mic.count) else { return mic }
        var m = mic
        gate.process(system: sys, mic: &m)
        blocks += 1
        if gate.gain < 1 { ducked += 1 }
        return m
    }

    /// Test seam: the tap's stop (if any) has returned.
    public func settled() async { await tapTask?.value; await lastStop?.value }
}

/// The dictation's gate. The meeting's `EchoGate` asks "is the system louder than the mic", which would also hold
/// down a user speaking softly over a loud video. This one learns how loud the system audio comes back through the
/// mic (the echo path's coupling, from the quietest recent blocks) and holds a block down only when the mic is
/// within 6 dB of that echo: the user's own words lift the mic well above it and pass at full gain. No AX speaker
/// context: while dictating, "someone else is talking in the call" is not a reason to drop the user's words.
public final class DictationEchoGate: @unchecked Sendable {
    /// Below this the system is silent: nothing to gate.
    public var systemActiveRms: Double = 0.01
    /// Gain for an echo-only block (-30 dB: the recogniser hears nothing of it).
    public var duck: Float = 0.0316
    /// The echo path's assumed coupling before any block taught it (mic RMS / system RMS), and its ceiling.
    public static let priorCoupling = 0.15, maxCoupling = 0.5
    public private(set) var coupling = DictationEchoGate.priorCoupling
    public private(set) var gain: Float = 1
    public init() {}

    public func reset() { coupling = Self.priorCoupling; gain = 1 }

    /// Target gain for one block. Fast down (an echo-only block teaches the coupling at once), slow up (the volume
    /// went up; never fast, or a long sentence would teach the gate that the user's voice is echo).
    public func target(systemRms: Double, micRms: Double) -> Float {
        guard systemRms > systemActiveRms else { return 1 }
        let ratio = micRms / systemRms
        let echoOnly = ratio < 2 * coupling
        if ratio < coupling { coupling = 0.5 * coupling + 0.5 * ratio }
        else { coupling = min(Self.maxCoupling, coupling * (echoOnly ? 1.05 : 1.005)) }
        return echoOnly ? duck : 1
    }

    /// Applies the gain to `mic` in place: ramped over 4 ms (no click, no clipped onset).
    public func process(system: [Float], mic: inout [Float]) {
        let t = target(systemRms: DSP.rms(system), micRms: DSP.rms(mic))
        let n = mic.count
        guard n > 0 else { return }
        let start = gain
        let ramp = min(n, 64)
        for i in 0..<n { mic[i] *= i < ramp ? start + (t - start) * Float(i + 1) / Float(ramp) : t }
        gain = t
    }
}
