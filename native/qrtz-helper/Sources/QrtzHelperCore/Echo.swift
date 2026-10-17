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
