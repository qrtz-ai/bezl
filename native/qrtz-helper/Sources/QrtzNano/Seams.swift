import Foundation
import QrtzShared

// Nano's platform seams (moved from QrtzHelperCore, ADR 0146 M2.5).

/// Emits meeting.* and ax.* messages. `clock` gives the current recording-relative time for atMs fields.
public protocol MeetingSource: AnyObject, Sendable {
    func start(emit: @escaping @Sendable (HelperMessage) -> Void, clock: @escaping @Sendable () -> Double)
    func stop()
}

/// Captured audio, converted to 16 kHz mono Float32.
public protocol AudioSource: AnyObject, Sendable {
    var name: String { get }
    /// Delivers aligned system + mic blocks (16 kHz mono). Echo handling and mixing happen in the controller.
    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void) async throws
    /// N35: like `start(onBlock:)`, plus `onDegraded` for input the source lost inside itself while it keeps
    /// recording the rest (its in-source mic failing: `no-mic` and the error text). May be called during `start` or
    /// later; the controller turns it into `capture.degraded`, once per flag per recording and only while recording.
    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void,
               onDegraded: @escaping @Sendable (RevocationDegradation, String) -> Void) async throws
    func stop() async
}

public extension AudioSource {
    /// Sources that cannot lose part of their input ignore `onDegraded`.
    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void,
               onDegraded: @escaping @Sendable (RevocationDegradation, String) -> Void) async throws {
        try await start(onBlock: onBlock)
    }
}

/// N35: collects a source's `onDegraded` reports. While the source is starting they are held (a source that then
/// throws never degraded the recording, and `recording.started` must come first); once the recording is started
/// they are handed straight to the controller.
final class DegradationInbox: @unchecked Sendable {
    private let lock = NSLock()
    private var held: [(RevocationDegradation, String)] = []
    private var live: (@Sendable (RevocationDegradation, String) -> Void)?
    private var closed = false

    func post(_ d: RevocationDegradation, _ reason: String) {
        lock.lock()
        if closed { lock.unlock(); return }
        if let live { lock.unlock(); live(d, reason); return }
        held.append((d, reason))
        lock.unlock()
    }

    /// The source failed to start: forget what it reported and ignore anything later.
    func discard() { lock.lock(); held.removeAll(); closed = true; lock.unlock() }

    /// The recording started on this source: return what was held, deliver later reports to `f`.
    func goLive(_ f: @escaping @Sendable (RevocationDegradation, String) -> Void) -> [(RevocationDegradation, String)] {
        lock.lock(); defer { lock.unlock() }
        if closed { return [] }
        live = f
        let h = held
        held.removeAll()
        return h
    }
}

/// What nano needs from its platform (ADR 0146 M2.5; the macOS seams come from QrtzNanoMac).
public struct NanoDeps: Sendable {
    public var meetings: MeetingSource?
    /// Tried in order; the first that starts wins (e.g. Core Audio tap, then ScreenCaptureKit).
    public var audio: [AudioSource]
    /// Tried in order (e.g. Nemotron via FluidAudio, then Apple SpeechAnalyzer).
    public var transcribers: [StreamingTranscriber]
    /// ADR 0079: a second instance of `transcribers.first` for the system channel. Set = a recording transcribes mic
    /// (echo-cleaned) and system separately (`mic-…` / `system-…` utterance ids) when both load; else the mix.
    public var systemTranscribers: [StreamingTranscriber] = []
    /// ADR 0079: one diarizer per raw channel, recordings only (never dictation), used with per-channel ASR.
    public var diarizers: [CaptureSource: ChannelDiarizer] = [:]
    /// N121: audio (16 kHz samples) a channel's diarizer may have queued, unprocessed (a slow model call or a slow
    /// load). Past it that diarizer stands down for the rest of the recording (turns already sent stay). 30 s.
    public var diarizerBacklogSamples = 16_000 * 30
    /// N121: how long the stop waits for the diarizers to process their queue before closing them out (their open
    /// turns are still sent) so `recording.stopped` stays well inside the host's 10 s ack.
    public var diarizerStopWaitMs: UInt64 = 3_000
    /// ADR 0079 / N121 (`models.warm`): loads every speech model once, one at a time, and unloads it, so the first
    /// recording after a download doesn't pay the OS model compile (about 50 s for the diarizer). Returns a summary.
    public var modelWarmup: (@Sendable () async -> String)?
    /// N137: the per-kind load gate `modelWarmup` and recordings share (one load of a model at a time).
    public var modelLoads = ModelLoads()
    /// Shared AX → audio context (who's speaking, who am I); feeds the echo gate.
    public var context: MeetingContext
    /// Optional AEC (LocalVQE on macOS); the echo gate is always the fallback.
    public var enhancer: MicEnhancer?
    /// N29: when a capture source counts as silent (`capture.silent` / `capture.audible`).
    public var silence: SilenceConfig
    /// ADR 0075: the on-device summary model (nil = `summarize` answers unavailable/unsupported).
    public var summarizer: OnDeviceSummarizer? = nil
    public init(meetings: MeetingSource? = nil, audio: [AudioSource] = [], transcribers: [StreamingTranscriber] = [],
                context: MeetingContext = MeetingContext(), enhancer: MicEnhancer? = nil, silence: SilenceConfig = .default) {
        self.meetings = meetings; self.audio = audio; self.transcribers = transcribers
        self.context = context; self.enhancer = enhancer; self.silence = silence
    }
}
