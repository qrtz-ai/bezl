import Foundation

// N143: echo's dictation hears what the speakers play. Without headphones the mic picks up the system audio (a video,
// a call), and the recogniser types it. While dictating, the mic is gated against the system audio: the meeting's
// system channel when a recording runs, otherwise a system tap that runs for the dictation's length only.

/// What the Mac plays (16 kHz mono). Only started for a dictation when `echoLikely()` and the screen & system
/// audio recording grant is already given (a dictation never prompts). `start` / `stop` are called in order.
public protocol SystemAudioTap: AnyObject, Sendable {
    var name: String { get }
    /// The output is speakers (not headphones) and something is playing.
    func echoLikely() -> Bool
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) async throws
    func stop() async
}

/// The system audio lined up with the mic, for the dictation's gate. Fed by whoever runs a system tap ("recording"
/// or "dictation"); the recording wins when both do, so samples are never counted twice.
public final class EchoReference: @unchecked Sendable {
    private let lock = NSLock()
    private var holders: [String] = []
    /// N152: each attach's token; a detach with a stale token (an older dictation's tap failing late) is a no-op.
    private var tokens: [String: Int] = [:]
    private var nextToken = 0
    private var buf: [Float] = []
    private var last: [Float] = []
    /// System audio more than this ahead of the mic is dropped (the two clocks drift; the gate needs the level only).
    public let maxLagSamples: Int
    public init(maxLagMs: Int = 250) { maxLagSamples = 16 * maxLagMs }

    public var isLive: Bool { lock.withLock { !holders.isEmpty } }
    public func has(_ who: String) -> Bool { lock.withLock { holders.contains(who) } }

    /// Returns the token that `detach(_:token:)` must present.
    @discardableResult
    public func attach(_ who: String) -> Int {
        lock.withLock {
            if !holders.contains(who) { holders.append(who) }
            nextToken += 1; tokens[who] = nextToken
            return nextToken
        }
    }
    /// Detaches `who`; with a `token`, only if it is still that attach (not a later one by the same holder).
    public func detach(_ who: String, token: Int? = nil) {
        lock.withLock {
            if let token, tokens[who] != token { return }
            tokens[who] = nil
            holders.removeAll { $0 == who }
            buf.removeAll(); last.removeAll()  // the other holder (if any) refills it from its own clock
        }
    }

    public func push(_ s: [Float], from who: String) {
        lock.withLock {
            guard let lead = holders.contains("recording") ? "recording" : holders.first, lead == who else { return }
            buf += s
            if buf.count > maxLagSamples { buf.removeFirst(buf.count - maxLagSamples) }
        }
    }

    /// The system audio for the next `n` mic samples, oldest first. Nil when no tap feeds the reference; the last
    /// block again when the tap is a block behind the mic.
    public func take(_ n: Int) -> [Float]? {
        lock.withLock {
            guard !holders.isEmpty else { return nil }
            let k = min(n, buf.count)
            guard k > 0 else { return last }
            last = Array(buf.prefix(k)); buf.removeFirst(k)
            return last
        }
    }
}

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

    public init(reference: EchoReference, tap: SystemAudioTap?, permitted: @escaping @Sendable () -> Bool) {
        self.reference = reference; self.tap = tap; self.permitted = permitted
    }

    /// At dictation.start. Returns the log line saying how this dictation's mic is gated (nil: no tap on this
    /// platform, nothing to say).
    @discardableResult
    public func begin() -> String? {
        blocks = 0; ducked = 0
        latched = enabled
        guard enabled else { return nil }
        // In a meeting the warm mic's blocks already taught the gate the echo path: keep it.
        if reference.has("recording") { return "echo gate: the meeting's system audio" }
        gate.reset()
        guard let tap else { return nil }
        guard permitted() else { return "echo gate: off (no system audio grant, raw mic)" }
        guard tap.echoLikely() else { return "echo gate: off (headphones or nothing playing)" }
        let token = reference.attach("dictation")
        tapToken = token
        let ref = reference, prev = lastStop, log = onLog
        // The tap starts in the background (tens of ms): the mic is not held for it; blocks before its first
        // samples pass ungated.
        tapTask = Task {
            await prev?.value
            do { try await tap.start(onSamples: { ref.push($0, from: "dictation") }) } catch {
                ref.detach("dictation", token: token)  // N152: never the next dictation's hold
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
