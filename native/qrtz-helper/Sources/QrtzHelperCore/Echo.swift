import Foundation

/// Shared, thread-safe meeting context the audio path consults (written by the AX layer, read on audio threads).
public final class MeetingContext: @unchecked Sendable {
    private let lock = NSLock()
    private var _selfName: String?
    private var _activeSpeaker: String?
    public init() {}

    public func update(selfName: String?, activeSpeaker: String?) {
        lock.lock(); defer { lock.unlock() }
        if let selfName { _selfName = selfName }
        if let activeSpeaker { _activeSpeaker = activeSpeaker }
    }

    public func reset() { lock.lock(); _selfName = nil; _activeSpeaker = nil; lock.unlock() }

    /// true: AX says the local user is speaking; false: someone else is; nil: unknown.
    public var selfSpeaking: Bool? {
        lock.lock(); defer { lock.unlock() }
        guard let a = _activeSpeaker, let me = _selfName else { return nil }
        return a.caseInsensitiveCompare(me) == .orderedSame
    }
}

/// Echo suppression for the mic channel when the user isn't wearing headphones: the mic re-captures the far end
/// from the speakers, so the mixed track (and the transcript) would contain remote speech twice.
///
/// Policy (cheap, no adaptive filter):
///  1. AX knows who's talking: if it's someone else and the system channel is active, duck the mic to -20 dB;
///     if it's the user, pass the mic at full gain.
///  2. Unknown speaker: energy heuristic. If the system channel is clearly louder than the mic (the mic hears
///     mostly echo), duck the mic to -10 dB.
/// Gain changes are smoothed so ducking doesn't click. Real AEC (FluidAudio LocalVQE with the system channel
/// as the far-end reference) is a later upgrade; see docs/mac-verify.md.
public final class EchoGate: @unchecked Sendable {
    public let context: MeetingContext
    public var systemActiveRms: Double = 0.01
    private var gain: Float = 1
    public init(context: MeetingContext) { self.context = context }

    /// Target gain for one block, before smoothing.
    public func target(systemRms: Double, micRms: Double) -> Float {
        guard systemRms > systemActiveRms else { return 1 }
        switch context.selfSpeaking {
        case .some(true): return 1
        case .some(false): return 0.1
        case .none: return systemRms > micRms * 1.5 ? 0.316 : 1
        }
    }

    /// Applies the smoothed gain to `mic` in place and returns it.
    public func process(system: [Float], mic: inout [Float]) {
        let t = target(systemRms: DSP.rms(system), micRms: DSP.rms(mic))
        let n = mic.count
        guard n > 0 else { return }
        let start = gain
        for i in 0..<n {
            let g = start + (t - start) * Float(i + 1) / Float(n)
            mic[i] *= g
        }
        gain = t
    }
}

/// One aligned block of capture: far-end/system audio and the user's mic, both 16 kHz mono, equal length.
/// `mic` is nil when no microphone is available (system audio only).
public struct CaptureBlock: Sendable, Equatable {
    public var system: [Float]
    public var mic: [Float]?
    public init(system: [Float], mic: [Float]?) {
        precondition(mic == nil || mic!.count == system.count, "capture block channels must be aligned")
        self.system = system; self.mic = mic
    }
}

/// Acoustic echo canceller for the mic channel using the system/far-end audio as the reference
/// (FluidAudio LocalVQE on macOS). Output sample i corresponds to input sample i but may be delivered later,
/// so `enhance` can return fewer or more samples than it was given.
public protocol MicEnhancer: AnyObject, Sendable {
    var name: String { get }
    func prepare() async throws
    func enhance(mic: [Float], reference: [Float]) async throws -> [Float]
    func reset() async
}

/// Turns aligned capture blocks into the single mixed track that is recorded and transcribed.
/// Uses the AEC when it is available and healthy; on any AEC failure it permanently falls back to the EchoGate
/// for the rest of the recording (and says so once via `onLog`).
public final class MicPipeline: @unchecked Sendable {
    public let gate: EchoGate
    public let enhancer: MicEnhancer?
    private var aecReady = false
    private var aecFailed = false
    private var pendingSystem: [Float] = []  // system samples awaiting their (delayed) enhanced mic samples
    public var onLog: (@Sendable (LogLevel, String) -> Void)?

    public init(gate: EchoGate, enhancer: MicEnhancer?) { self.gate = gate; self.enhancer = enhancer }

    public var mode: String { aecReady && !aecFailed ? "aec:\(enhancer?.name ?? "")" : "gate" }

    /// Call at recording start. Never throws: an AEC that can't load just leaves the gate in charge.
    public func start() async {
        pendingSystem.removeAll()
        aecFailed = false
        aecReady = false
        guard let e = enhancer else { return }
        do {
            try await e.prepare()
            await e.reset()
            aecReady = true
            onLog?(.info, "echo: \(e.name)")
        } catch {
            onLog?(.warn, "echo: \(e.name) unavailable (\(error)); using echo gate")
        }
    }

    /// The echo-cleaned mic and the system audio it lines up with (equal lengths; the AEC may hold some back).
    public func processChannels(_ b: CaptureBlock) async -> (system: [Float], mic: [Float]?) {
        guard var mic = b.mic else { return (b.system, nil) }
        if let e = enhancer, aecReady, !aecFailed {
            pendingSystem += b.system
            do {
                let clean = try await e.enhance(mic: mic, reference: b.system)
                let n = min(clean.count, pendingSystem.count)
                let sys = Array(pendingSystem[0..<n])
                pendingSystem.removeFirst(n)
                return (sys, Array(clean[0..<n]))
            } catch {
                aecFailed = true
                onLog?(.warn, "echo: \(e.name) failed (\(error)); falling back to echo gate")
                // Release the system audio that was waiting on the AEC, then continue with the gate.
                let held = Array(pendingSystem.dropLast(b.system.count))
                pendingSystem.removeAll()
                gate.process(system: b.system, mic: &mic)
                return (held + b.system, [Float](repeating: 0, count: held.count) + mic)
            }
        }
        gate.process(system: b.system, mic: &mic)
        return (b.system, mic)
    }

    /// system + mic, clipped (what the WAV and a single recogniser get).
    public static func mix(_ ch: (system: [Float], mic: [Float]?)) -> [Float] {
        guard let mic = ch.mic else { return ch.system }
        var out = [Float](repeating: 0, count: min(mic.count, ch.system.count))
        for i in 0..<out.count { out[i] = max(-1, min(1, ch.system[i] + mic[i])) }
        return out
    }

    public func process(_ b: CaptureBlock) async -> [Float] { Self.mix(await processChannels(b)) }

    /// End of recording: whatever system audio is still waiting is emitted (its mic tail is lost, ≤ 1 hop).
    public func finish() -> [Float] {
        defer { pendingSystem.removeAll() }
        return pendingSystem
    }
}
