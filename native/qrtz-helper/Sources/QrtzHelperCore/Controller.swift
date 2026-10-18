import Foundation

// Platform seams. The macOS executable provides real implementations; tests (and Linux) provide fakes.

public protocol PermissionsProvider: Sendable {
    func status() -> [OsPermission: PermissionState]
    /// Prompts (or opens System Settings) for one permission. Returns after the prompt is shown, not when granted.
    func request(_ p: OsPermission) async
}

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

/// A streaming recogniser that exposes one cumulative transcript.
public protocol StreamingTranscriber: AnyObject, Sendable {
    var name: String { get }
    func prepare() async throws
    func append(_ samples16k: [Float]) async
    func transcript() async -> String
    func finish() async -> String
    func reset() async
    /// False for a model that may take seconds to load (a recording then starts at once and its audio waits).
    var loadsInstantly: Bool { get }
    /// How far the transcript trails the audio fed (one decoding chunk), for segment timestamps.
    var latencyMs: Double { get }
    /// Frees the model (a recording ended; the next one loads it again).
    func unload() async
}

extension StreamingTranscriber {
    public var loadsInstantly: Bool { true }
    public var latencyMs: Double { 0 }
    public func unload() async {}
}

public struct HelperDeps: Sendable {
    public var permissions: PermissionsProvider
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
    /// ADR 0087: load echo's recogniser when the helper starts (kept for its life). Off in tests that count loads.
    public var preloadDictationAsr = true
    /// ADR 0092: the native dictation pill (macOS, when the host asks for it with `QRTZ_NATIVE_PILL=1`).
    public var pill: PillView? = nil
    public var pillTiming = PillDriver.Timing()
    /// Echo: what the helper does between the key release and `dictation.stopped` (tail, drain, pad, finalize).
    public var dictationTail = DictationTail()
    public var helperVersion: String
    public var os: String
    public var features: [String]
    /// Shared AX → audio context (who's speaking, who am I); feeds the echo gate.
    public var context: MeetingContext
    /// Optional AEC (LocalVQE on macOS); the echo gate is always the fallback.
    public var enhancer: MicEnhancer?
    /// qrtz talk: mic-only source, its own recogniser instances (so dictation works during a meeting), insertion, keys.
    public var dictationMic: MicSource?
    /// N143: a system-audio tap echo runs for a dictation's length when the speakers play (nil: no gate without a
    /// meeting). A recording's own system channel is used instead while one runs.
    public var dictationTap: SystemAudioTap? = nil
    public var dictationTranscribers: [StreamingTranscriber]
    public var insertion: InsertionBackend?
    public var hotkeys: HotkeyMonitor?
    /// H6: the shared mic tap. When set, dictation and meeting capture read the mic through views of it, and a
    /// microphone revocation drops every reader at once.
    public var micHub: MicHub?
    /// H5: how often TCC state is re-read while capturing or dictating (0 = never poll; tests call checkPermissions()).
    public var permissionPollMs: Int
    /// N29: when a capture source counts as silent (`capture.silent` / `capture.audible`).
    public var silence: SilenceConfig
    /// ADR 0046 §1 / N55: where the speech module's audio fds arrive (nil = the host passed no speech socket).
    public var speechFds: FdReceiver?
    /// ADR 0075: the on-device summary model (nil = `summarize` answers unavailable/unsupported).
    public var summarizer: OnDeviceSummarizer? = nil
    public init(permissions: PermissionsProvider, meetings: MeetingSource?, audio: [AudioSource], transcribers: [StreamingTranscriber], helperVersion: String, os: String, features: [String], context: MeetingContext = MeetingContext(), enhancer: MicEnhancer? = nil,
                dictationMic: MicSource? = nil, dictationTranscribers: [StreamingTranscriber] = [], insertion: InsertionBackend? = nil, hotkeys: HotkeyMonitor? = nil,
                micHub: MicHub? = nil, permissionPollMs: Int = 2000, silence: SilenceConfig = .default, speechFds: FdReceiver? = nil) {
        self.speechFds = speechFds
        self.micHub = micHub; self.permissionPollMs = permissionPollMs; self.silence = silence
        self.dictationMic = dictationMic; self.dictationTranscribers = dictationTranscribers; self.insertion = insertion; self.hotkeys = hotkeys
        self.permissions = permissions; self.meetings = meetings; self.audio = audio; self.transcribers = transcribers
        self.helperVersion = helperVersion; self.os = os; self.features = features
        self.context = context; self.enhancer = enhancer
    }
}

/// The helper's brain: reacts to host messages and drives the platform seams. All state lives on this actor.
public actor HelperController {
    private let deps: HelperDeps
    private let send: @Sendable (HelperMessage) -> Void
    private let now: @Sendable () -> Double  // monotonic ms
    private var recording: Recording?
    private var tick: Task<Void, Never>?
    private var shuttingDown = false
    public private(set) var paused = false
    nonisolated(unsafe) private var recordingStartMs: Double?

    final class Recording: @unchecked Sendable {
        let wav: WavWriter
        let startMs: Double
        var audio: AudioSource?
        var transcriber: StreamingTranscriber?
        var segmenter = UtteranceSegmenter()
        /// ADR 0079: per-channel mode (mic on `transcriber`, system here).
        var systemTranscriber: StreamingTranscriber?
        var systemSegmenter: UtteranceSegmenter?
        /// False while the recogniser loads in the background; captured audio waits in `backlog` meanwhile.
        var asrReady = true
        var backlog: [AsrBlock] = []
        var backlogSamples = 0
        /// Samples of recording time the recognisers have been given (paused time included): the per-channel clock.
        var asrSamples = 0
        var asrLatencyMs: Double = 0
        var prep: Task<Bool, Never>?
        var diarRuns: [DiarRun] = []
        var lastLevelMs: Double = 0
        var pendingLevel: [Float] = []
        var samplesIn: AsyncStream<CaptureBlock>.Continuation?
        var pipeline: MicPipeline?
        var consumer: Task<Void, Never>?
        /// N2: torn down by recording.abort; an in-flight stop for it must not touch anything any more.
        var aborted = false
        /// N29: per-source silence on the raw (pre-mix) capture channels.
        var silence: SilenceTracker
        /// N35: flags already sent as capture.degraded for this recording (once each).
        var degradedSent: Set<RevocationDegradation> = []
        /// N35: the started source's reports; closed when the recording ends.
        var degradations: DegradationInbox?
        /// N35: the stop (or abort) began; nothing about this recording's capture is reported any more.
        var ending = false
        init(wav: WavWriter, startMs: Double, silence: SilenceConfig = .default) {
            self.wav = wav; self.startMs = startMs; self.silence = SilenceTracker(config: silence)
        }
    }

    struct AsrBlock { let mix: [Float]; let system: [Float]; let mic: [Float]; let paused: Bool }

    /// One channel's diarizer for one recording: its own stream and task, so a model call never holds up capture.
    /// N121: the queue is bounded by `HelperDeps.diarizerBacklogSamples`; past it the run stands down (whole, never
    /// by dropping blocks, so the turns it sent stay on the recording's timeline). After it stood down or closed out
    /// nothing more is sent from it.
    final class DiarRun: @unchecked Sendable {
        let source: CaptureSource
        let diarizer: ChannelDiarizer
        let input: AsyncStream<[Float]>.Continuation
        let stream: AsyncStream<[Float]>
        private let send: @Sendable (HelperMessage) -> Void
        private let lock = NSLock()
        private var queued = 0
        /// Samples the model has been handed (recording time it covers): where speaker turns end on a stand-down.
        private var taken = 0
        private var active = true
        private(set) var peakQueued = 0
        var task: Task<Void, Never>?
        init(source: CaptureSource, diarizer: ChannelDiarizer, send: @escaping @Sendable (HelperMessage) -> Void) {
            self.source = source; self.diarizer = diarizer; self.send = send
            (stream, input) = AsyncStream<[Float]>.makeStream()
        }
        var isActive: Bool { lock.withLock { active } }
        /// Queues one block; false when that would pass `cap` (the run has then stood down and its stream ended).
        func offer(_ s: [Float], cap: Int) -> Bool {
            let ok: Bool = lock.withLock {
                guard active else { return true }
                if queued + s.count > cap {
                    // N126: the open turn up to the processed audio goes out, then the host is told from when on
                    // this channel has no speakers.
                    active = false
                    for t in diarizer.abandon() { send(.diarizationSegment(source: source, speaker: t.speaker, startMs: t.startMs, endMs: t.endMs)) }
                    send(.diarizationUnavailable(source: source, atMs: Double(taken) / 16, reason: .backlog))
                    return false
                }
                queued += s.count
                peakQueued = max(peakQueued, queued)
                input.yield(s)
                return true
            }
            if !ok { input.finish() }
            return ok
        }
        func took(_ n: Int) { lock.withLock { queued -= n; taken += n } }
        /// N126: the model didn't load: no turns at all on this channel (unless the run already ended).
        func unavailable(_ why: DiarizationUnavailableReason) {
            lock.withLock {
                guard active else { return }
                active = false
                send(.diarizationUnavailable(source: source, atMs: 0, reason: why))
            }
        }
        func emit(_ turns: [DiarizedTurn]) {
            lock.withLock {
                guard active else { return }
                for t in turns { send(.diarizationSegment(source: source, speaker: t.speaker, startMs: t.startMs, endMs: t.endMs)) }
            }
        }
        /// The run is over (finished, stood down or closed out): nothing more is sent.
        func end() { lock.withLock { active = false } }
        /// Stop gave up waiting: send the tracker's open turns once, then nothing.
        func closeOut() {
            lock.withLock {
                guard active else { return }
                active = false
                for t in diarizer.abandon() { send(.diarizationSegment(source: source, speaker: t.speaker, startMs: t.startMs, endMs: t.endMs)) }
            }
            input.finish()
            task?.cancel()
        }
    }

    /// Fires once (whichever of a completion and a deadline comes first).
    final class Once: @unchecked Sendable {
        private let lock = NSLock()
        private var done = false
        func fire(_ f: () -> Void) { let first = lock.withLock { () -> Bool in defer { done = true }; return !done }; if first { f() } }
    }

    /// True if every run's task ended within `ms`.
    static func waitAll(_ runs: [DiarRun], ms: UInt64) async -> Bool {
        let tasks = runs.compactMap(\.task)
        return await withCheckedContinuation { (c: CheckedContinuation<Bool, Never>) in
            let once = Once()
            Task { for t in tasks { await t.value }; once.fire { c.resume(returning: true) } }
            Task { try? await Task.sleep(nanoseconds: ms * 1_000_000); once.fire { c.resume(returning: false) } }
        }
    }

    /// N121 test seam: each channel's largest diarizer queue (samples) in the last recording.
    var lastDiarizerPeaks: [CaptureSource: Int] = [:]
    /// N125 (ADR 0082): a closed-out or aborted run's task, which may still be inside a model call on its channel's
    /// shared diarizer. The next run on that channel waits for it before `prepare()`, so the old task's trailing
    /// `unload()` can never take the model away from a newer recording.
    var lingering: [CaptureSource: Task<Void, Never>] = [:]
    /// A `models.warm` is running (a second one is ignored).
    var warming = false

    /// At most this much audio waits for a loading recogniser (older audio is then left untranscribed).
    static let maxBacklogSamples = 16_000 * 180

    /// H6/H5 arbiter state (see Arbiter.swift); `lastPerms` is the TCC snapshot revocations are diffed against.
    public private(set) var arbiter: ArbiterState
    private var lastPerms: [OsPermission: PermissionState]
    private var permWatch: Task<Void, Never>?

    public init(deps: HelperDeps, send: @escaping @Sendable (HelperMessage) -> Void, now: @escaping @Sendable () -> Double) {
        // ADR 0092: the pill sees every line before it is written, so a key press shows it at once.
        let pill = deps.pill.map { PillDriver(view: $0, timing: deps.pillTiming) }
        let raw = send
        let send: @Sendable (HelperMessage) -> Void
        if let pill { send = { m in pill.helper(m); raw(m) } } else { send = raw }
        self.pill = pill
        self.deps = deps; self.send = send; self.now = now
        let st = deps.permissions.status()
        self.lastPerms = st
        self.arbiter = ArbiterState(denied: OsPermission.allCases.filter { st[$0] == .denied })
        // N143: granted means screen & system audio recording is already allowed, so a dictation never prompts.
        let perms = deps.permissions
        let echo = DictationEcho(reference: echoReference, tap: deps.dictationTap,
                                 permitted: { perms.status()[.screenRecording] == .granted })
        echo.onLog = { level, msg in send(.log(level: level, msg: msg)) }
        self.dictation = DictationEngine(mic: deps.dictationMic, transcribers: deps.dictationTranscribers,
                                         inserter: deps.insertion.map { TextInserter(backend: $0) }, hotkeys: deps.hotkeys, send: send, now: now, managedMic: true,
                                         speech: speech, tail: deps.dictationTail, echo: echo)
        self.summaries = SummarizeService(model: deps.summarizer, send: send)
    }
    /// ADR 0075: summarize requests, off the message queue.
    public let summaries: SummarizeService
    public let dictation: DictationEngine
    /// N143: the system audio echo's mic is gated against (a recording's system channel, or echo's own tap).
    nonisolated public let echoReference = EchoReference()
    /// The speech module's audio sinks (recordings and dictations).
    public let speech = SpeechSinks()

    /// ms since recording start (or since helper start when idle). Used for every atMs/startMs on the wire.
    nonisolated public func relativeMs() -> Double {
        let n = now()
        return max(0, n - (recordingStartMs ?? 0))
    }

    public func start() {
        send(.hello(protocol: helperProtocolVersion, helperVersion: deps.helperVersion, os: deps.os, features: deps.features))
        send(.permissionsStatus(deps.permissions.status()))
        deps.meetings?.start(emit: send, clock: { [weak self] in self?.relativeMs() ?? 0 })
        deps.micHub?.addLostListener { [weak self] reason in Task { await self?.micDeviceLost(reason) } }
        if deps.preloadDictationAsr {
            // Inside the "asr" gate (N137), so it never compiles alongside a recording's loads; a press meanwhile
            // doesn't wait for the gate: it starts (or joins) the transcriber's one load itself.
            let engine = dictation, loads = deps.modelLoads
            startupPreload = Task { _ = try? await loads.load("asr") { await engine.preload().value } }
        }
    }

    /// ADR 0087: the startup load of echo's recogniser (tests await it).
    public var startupPreload: Task<Void, Never>?

    public var isRecording: Bool { recording != nil }

    /// ADR 0092: the native pill's state machine (nil without a pill view).
    nonisolated public let pill: PillDriver?

    public func handle(_ msg: HostMessage) async {
        pill?.host(msg)
        switch msg {
        case .ping(let id): send(.pong(id: id))
        case .permissionsCheck: send(.permissionsStatus(deps.permissions.status()))
        case .permissionsRequest(let p):
            await deps.permissions.request(p)
            send(.permissionsStatus(deps.permissions.status()))
        case .recordingStart(let path):
            guard await admit(.cmd("recording.start", audioPath: path), audioPath: path) else { return }
            await startRecording(path)
        case .recordingStop(let path):
            // A stop aimed at another file (the host's stale-start cleanup racing a newer recording) is ignored.
            if let path, let rec = recording, rec.wav.url.path != path {
                send(.log(level: .warn, msg: "recording.stop for \(path) while recording \(rec.wav.url.path); ignored")); return
            }
            guard await admit(.cmd("recording.stop")) else { return }
            await stopRecording()
        case .recordingAbort(let path): await abortRecording(path)
        case .recordingPause: paused = true; send(.log(level: .info, msg: "capture paused"))
        case .recordingResume: paused = false; send(.log(level: .info, msg: "capture resumed"))
        case .modelsWarm: await warmModels()
        case .shutdown: await shutdown()
        case .dictationConfigure(let warm, let pre, let stream, let key, let consume, let showPill):
            pill?.enabled = showPill ?? true
            await dictation.configure(warmMic: warm, preRollMs: pre, streamAudio: stream, pushKey: key, consumeFn: consume ?? true)
            _ = await admit(.configure(warmMic: warm))
            watchPermissions()
        case .dictationStart(let id):
            guard await admit(.cmd("dictation.start"), id: id) else { return }
            await dictation.start(id: id)
            // The engine could not start (no source): undo through the arbiter so the reader is released too.
            if await !dictation.isActive { _ = await admit(.cmd("dictation.cancel")) }
            watchPermissions()
        case .dictationStop(let id):
            guard await dictation.activeId == id else { return }
            await dictation.stop(id: id)
            _ = await admit(.cmd("dictation.stop"))
        case .dictationCancel(let id):
            guard await dictation.activeId == id else { return }
            await dictation.cancel(id: id)
            _ = await admit(.cmd("dictation.cancel"))
        case .insertText(let id, let text, let restore): await dictation.insert(id: id, text: text, restoreClipboard: restore)
        case .pillFlash: break  // the pill driver took it (`pill?.host`); without a native pill there is nothing to show
        case .speechAttach(let sid, let target, let channels):
            if let why = speech.accept(deps.speechFds, sessionId: sid, target: target, channels: channels) {
                send(.speechAttachRejected(sessionId: sid, reason: why))
            } else {
                send(.speechAttached(sessionId: sid))
            }
        case .summarize(let id, let stage, let prompt): await summaries.start(id: id, stage: stage, prompt: prompt)
        case .summarizeCancel(let id): await summaries.cancel(id: id)
        case .summarizeCheck(let id):
            // Availability can take a moment (model assets); answer off the queue too.
            let svc = summaries
            Task { await svc.check(id: id) }
        }
    }

    public func heartbeat() { send(.heartbeat(ts: now())) }

    /// Runs an event through the arbiter and carries out its mic effects (the arbiter is enforcing: its `mic`
    /// list is the hub's reader list). An ignored start answers error{recording.busy | dictation.busy} naming both
    /// the ignored start (`id` / `audioPath`) and the session that owns the helper (`activeId` / `activeAudioPath`),
    /// so the host can tell its own start apart and clean up an orphan; a refused dictation.start answers
    /// error{dictation.mic}; other ignored commands are logged.
    @discardableResult
    func admit(_ e: ArbiterEvent, id: String? = nil, audioPath: String? = nil) async -> Bool {
        let r = SessionArbiter.step(arbiter, e)
        arbiter = r.state
        await apply(r.effects)
        guard case .cmd(let c, _) = e else { return true }
        switch r.decision {
        case "ignore":
            switch r.reason {
            case "already-recording":
                // The invariant guarantees a running recording has a file, so a busy always names it.
                send(.error(code: "recording.busy", message: "a recording is already running", fatal: false,
                            audioPath: audioPath, activeAudioPath: arbiter.activeAudioPath))
            case "already-dictating":
                let active = await dictation.activeId
                send(.error(code: "dictation.busy", message: "a dictation is already running", fatal: false, id: id, activeId: active))
            default: send(.log(level: .warn, msg: "\(c) while \(r.reason ?? "busy"); ignored"))
            }
            return false
        case "refuse":
            if r.reason == "no-audio-path" { send(.log(level: .warn, msg: "\(c) without an audio path; refused")); return false }
            send(.error(code: "dictation.mic", message: "microphone access denied", fatal: false)); return false
        default:
            if r.mux { send(.log(level: .info, msg: "mux: recording and dictation share the mic tap")) }
            return true
        }
    }

    private func apply(_ effects: [String]) async {
        for fx in effects {
            switch fx {
            case "mic.subscribe:dictation": await dictation.attachMic()
            case "mic.unsubscribe:dictation": await dictation.detachMic()
            case "mic.subscribe:recording": deps.micHub?.permit("recording")
            case "mic.unsubscribe:recording": deps.micHub?.revokePermit("recording"); deps.micHub?.unsubscribe("recording")
            case "dictation.stop:permission-revoked": await dictation.abort(reason: .permissionRevoked)
            default: break  // degrade:* is applied by the host; recording.lost: the caller already tore the recording down
            }
        }
    }

    /// Anything holding (or wanting) the mic: a recording, a dictation, or the warm mic, even with nothing running,
    /// so a revoke drops the warm reader promptly and a re-grant brings it back.
    private var pollWanted: Bool { arbiter.recording || arbiter.dictating || arbiter.warm }

    /// H5: polls TCC while anything captures or the mic is warm; stops by itself once idle.
    func watchPermissions() {
        guard permWatch == nil, deps.permissionPollMs > 0, pollWanted else { return }
        let ns = UInt64(deps.permissionPollMs) * 1_000_000
        permWatch = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: ns)
                guard let self, await self.pollPermissionsOnce() else { return }
            }
        }
    }

    private func pollPermissionsOnce() async -> Bool {
        await checkPermissions()
        if pollWanted { return true }
        permWatch = nil
        return false
    }

    /// Diffs TCC state against the last snapshot. granted → denied emits permission.revoked and applies the
    /// arbiter's effects: a dictation ends itself (dictation.stopped{reason: permission-revoked}) and the shared
    /// mic tap is dropped; the recording keeps whatever source still works (the host adds the degradation).
    public func checkPermissions() async {
        let st = deps.permissions.status()
        defer { lastPerms = st }
        for p in OsPermission.allCases {
            let was = lastPerms[p], now = st[p]
            if now == .denied, was != .denied {
                let r = SessionArbiter.step(arbiter, .revoked(p))
                guard r.decision == "accept" else { arbiter = r.state; continue }
                // N4: carry the arbiter's own degradations so the host never re-derives them from a stale snapshot.
                let degraded = r.effects.compactMap { f in
                    f.hasPrefix("degrade:") ? RevocationDegradation(rawValue: String(f.dropFirst("degrade:".count))) : nil
                }
                send(.permissionRevoked(permission: p, atMs: relativeMs(), degraded: degraded))
                arbiter = r.state
                await apply(r.effects)
                if p == .microphone {
                    deps.micHub?.dropAll()
                    // No session holds the engine's reader any more; make sure it forgets it (warm mic).
                    if await !dictation.isActive { await dictation.micDropped() }
                }
            } else if now == .granted, was == .denied {
                await admit(.granted(p))
            }
        }
    }

    func startRecording(_ path: String) async {
        if recording != nil { send(.log(level: .warn, msg: "recording.start while recording; ignored")); return }
        if let w = warmTask { w.cancel(); send(.log(level: .info, msg: "models.warm cancelled: recording")) }
        let wav: WavWriter
        do { wav = try WavWriter(url: URL(fileURLWithPath: path)) } catch {
            // Fail closed: a recording never runs without a file. Nothing has started yet (no source, no ASR, no
            // started{}); release the admitted recording (its mic reader and permit) and tell the host which start died.
            let r = SessionArbiter.step(arbiter, .recordingFileFailed(audioPath: path))
            arbiter = r.state
            await apply(r.effects)
            send(.error(code: "recording.file", message: "\(error)", fatal: false, audioPath: path))
            return
        }
        let rec = Recording(wav: wav, startMs: now(), silence: deps.silence)
        recordingStartMs = rec.startMs
        recording = rec

        // ADR 0046 §1: a part the host attached to the speech module (speech.attach precedes recording.start) is
        // transcribed there only; the helper's own recogniser is not loaded for it.
        if speech.has(.recording(audioPath: rec.wav.url.path)) {
            send(.log(level: .info, msg: "asr: speech module"))
        } else if deps.systemTranscribers.isEmpty && (deps.transcribers.first?.loadsInstantly ?? true) {
            _ = await prepareAsr(rec)
        } else {
            // ADR 0079: a model that takes seconds to load must not delay the capture: audio starts now and waits in
            // the backlog; the diarizers start with it and wait for the recogniser's mode (per channel or not).
            rec.asrReady = false
            let prep = Task { [weak self] () -> Bool in await self?.prepareAsr(rec) ?? false }
            rec.prep = prep
            startDiarizers(rec, after: prep)
        }

        // Audio callbacks arrive on capture threads; funnel them through one ordered stream into the actor.
        let pipeline = MicPipeline(gate: EchoGate(context: deps.context), enhancer: deps.enhancer)
        let send = self.send
        pipeline.onLog = { level, msg in send(.log(level: level, msg: msg)) }
        await pipeline.start()
        rec.pipeline = pipeline
        let (blocks, samplesIn) = AsyncStream<CaptureBlock>.makeStream(bufferingPolicy: .bufferingNewest(400))
        rec.samplesIn = samplesIn
        rec.consumer = Task { [weak self] in
            for await b in blocks {
                await self?.trackSilence(b, of: rec)
                await self?.feedSpeech(b, of: rec)
                let ch = await pipeline.processChannels(b)
                await self?.onSamples(MicPipeline.mix(ch), of: rec, channels: ch)
            }
        }
        let echoRef = echoReference
        echoRef.attach("recording")
        for (i, a) in deps.audio.enumerated() {
            let inbox = DegradationInbox()
            do {
                try await a.start(onBlock: { b in echoRef.push(b.system, from: "recording"); samplesIn.yield(b) },
                                  onDegraded: { d, r in inbox.post(d, r) })
                rec.audio = a
                rec.degradations = inbox
                send(.log(level: .info, msg: "audio: \(a.name)"))
                break
            } catch {
                inbox.discard()
                // H8: structured, so the host can tell which source the recording fell back to and why.
                let next = i + 1 < deps.audio.count ? deps.audio[i + 1].name : nil
                send(.captureFallback(from: a.name, to: next, reason: "\(error)"))
            }
        }
        if rec.audio == nil { echoRef.detach("recording") }
        if rec.audio == nil { send(.error(code: "audio.unavailable", message: "no audio source could start (check Screen & System Audio Recording and Microphone permissions)", fatal: false)) }

        send(.recordingStarted(audioPath: path))
        if let inbox = rec.degradations {
            let held = inbox.goLive { [weak self, weak rec] d, r in
                guard let rec else { return }
                Task { await self?.sourceDegraded(d, reason: r, of: rec) }
            }
            for (d, r) in held { sourceDegraded(d, reason: r, of: rec) }
        }
        watchPermissions()
        tick = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 250_000_000)
                await self?.pollTranscript()
            }
        }
    }

    /// N138: the mic's device went away and didn't come back. A dictation ends with what was heard
    /// (`dictation.stopped{reason: mic-lost}`); a recording goes on with system audio and says so once
    /// (`capture.degraded{no-mic}`).
    func micDeviceLost(_ reason: String) async {
        send(.log(level: .warn, msg: "mic: input device lost (\(reason))"))
        if await dictation.isActive { await dictation.abort(reason: .micLost) }
        if let rec = recording { sourceDegraded(.noMic, reason: "microphone lost: \(reason)", of: rec) }
    }

    /// N35: a started source lost part of its input (its in-source mic). `capture.degraded` once per flag per
    /// recording, and only for the current, live recording (not after its stop or abort).
    func sourceDegraded(_ d: RevocationDegradation, reason: String, of owner: Recording) {
        guard let rec = recording, owner === rec, !rec.ending, !rec.degradedSent.contains(d) else { return }
        rec.degradedSent.insert(d)
        send(.captureDegraded(degraded: [d], reason: reason))
    }

    /// N29: feed the raw channels (before echo gate and mix) to the recording's silence tracker. A mic channel the
    /// source does not deliver (no mic, or revoked) is not tracked: permission.revoked / degradations cover that.
    func trackSilence(_ b: CaptureBlock, of owner: Recording) {
        guard let rec = recording, owner === rec, !rec.aborted else { return }
        if paused { rec.silence.pause(); return }
        let t = now() - rec.startMs
        if let m = rec.silence.update(.system, rms: DSP.rms(b.system), atMs: t) { send(m) }
        if let mic = b.mic, let m = rec.silence.update(.mic, rms: DSP.rms(mic), atMs: t) { send(m) }
    }

    /// ADR 0046 §1: the raw channels (pre echo gate and mix) to the module, contiguous from the start: silence
    /// while paused and for a mic the source does not deliver.
    func feedSpeech(_ b: CaptureBlock, of owner: Recording) {
        guard let rec = recording, owner === rec, !rec.aborted else { return }
        let t = SpeechTarget.recording(audioPath: rec.wav.url.path)
        guard speech.has(t) else { return }
        let zero = [Float](repeating: 0, count: b.system.count)
        speech.write(t, .system, paused ? zero : b.system)
        speech.write(t, .mic, paused ? zero : (b.mic ?? zero))
    }

    /// Picks the recogniser(s) for `rec` and feeds them whatever audio waited. Per channel when a system instance
    /// exists and both load; otherwise the first of `transcribers` that loads, on the mix. True = per channel.
    func prepareAsr(_ rec: Recording) async -> Bool {
        var perChannel = false
        var failed: StreamingTranscriber?
        if let micT = deps.transcribers.first, let sysT = deps.systemTranscribers.first {
            do {
                try await deps.modelLoads.load("asr") { try await micT.prepare(); try await sysT.prepare() }
                await micT.reset(); await sysT.reset()
                rec.transcriber = micT; rec.systemTranscriber = sysT
                rec.segmenter = UtteranceSegmenter(idPrefix: "mic-u")
                rec.systemSegmenter = UtteranceSegmenter(idPrefix: "system-u")
                rec.asrLatencyMs = micT.latencyMs
                perChannel = true
                send(.log(level: .info, msg: "asr: \(micT.name) (mic and system)"))
            } catch {
                send(.log(level: .warn, msg: "asr \(micT.name) unavailable: \(error)"))
                failed = micT
                await micT.unload(); await sysT.unload()
            }
        }
        if !perChannel {
            for t in deps.transcribers where t !== failed {
                do {
                    try await deps.modelLoads.load("asr") { try await t.prepare() }
                    await t.reset(); rec.transcriber = t; send(.log(level: .info, msg: "asr: \(t.name)")); break
                }
                catch { send(.log(level: .warn, msg: "asr \(t.name) unavailable: \(error)")) }
            }
            if rec.transcriber == nil { send(.error(code: "asr.unavailable", message: "no speech recogniser available; recording audio only", fatal: false)) }
        }
        // Audio that arrived meanwhile, in order (new blocks keep queueing behind it until the backlog is empty).
        while !rec.backlog.isEmpty, !rec.aborted {
            let b = rec.backlog.removeFirst()
            rec.backlogSamples -= b.mix.count
            await pushAsr(b, to: rec)
        }
        rec.backlog.removeAll(); rec.backlogSamples = 0
        rec.asrReady = true
        if rec.aborted { await unloadAsr(rec) }
        return perChannel
    }

    /// `models.warm`: in the background, never next to a recording (memory: one model at a time, only while idle).
    /// The task is returned for tests.
    @discardableResult
    func warmModels() async -> Task<Void, Never>? {
        guard let warm = deps.modelWarmup else { return nil }
        if recording != nil { send(.log(level: .info, msg: "models.warm skipped: recording")); return nil }
        if warming { return nil }
        warming = true
        let send = self.send, dictation = self.dictation
        let t = Task.detached { [weak self] in
            send(.log(level: .info, msg: await warm()))
            await self?.warmDone()
            if !Task.isCancelled { await dictation.prewarmIfKeyed() }  // talk's own instance, now from the compiled cache
        }
        warmTask = t
        return t
    }

    /// N127: the running `models.warm`; a recording that starts cancels it (the warm checks between models and
    /// unloads what it loaded), so no warm-up model stays resident next to the recording's.
    var warmTask: Task<Void, Never>?

    func warmDone() { warming = false; warmTask = nil }

    /// ADR 0079: one task per channel diarizer. It loads the model (local files only) while capture runs, then waits
    /// for the recogniser's mode: speakers only make sense with per-channel transcripts, so it stands down otherwise.
    func startDiarizers(_ rec: Recording, after prep: Task<Bool, Never>) {
        guard !deps.systemTranscribers.isEmpty else { return }
        let send = self.send, loads = deps.modelLoads
        for (source, d) in deps.diarizers.sorted(by: { $0.key.rawValue < $1.key.rawValue }) {
            let run = DiarRun(source: source, diarizer: d, send: send)
            let stream = run.stream
            let prior = lingering.removeValue(forKey: source)
            run.task = Task.detached {
                defer { run.end() }
                if let prior { await prior.value }  // N125: one model call at most; the queue bound covers it
                do { try await loads.load("diarizer") { try await d.prepare() }; await d.reset() } catch {
                    send(.log(level: .warn, msg: "diarizer \(source.rawValue) unavailable: \(error)"))
                    run.unavailable(.modelUnavailable)
                    for await s in stream { run.took(s.count) }
                    return
                }
                guard await prep.value else {
                    run.end()
                    for await s in stream { run.took(s.count) }
                    await d.unload()
                    return
                }
                if run.isActive { send(.log(level: .info, msg: "diarizer: \(d.name) (\(source.rawValue))")) }
                for await s in stream {
                    run.took(s.count)
                    if !run.isActive || Task.isCancelled { break }
                    run.emit(await d.append(s))
                }
                if run.isActive && !Task.isCancelled { run.emit(await d.finish()) }
                await d.unload()
            }
            rec.diarRuns.append(run)
        }
    }

    func pushAsr(_ b: AsrBlock, to rec: Recording) async {
        rec.asrSamples += b.mix.count
        if b.paused { return }
        if let st = rec.systemTranscriber {
            await rec.transcriber?.append(b.mic)
            await st.append(b.system)
        } else {
            await rec.transcriber?.append(b.mix)
        }
    }

    func unloadAsr(_ rec: Recording) async {
        if !(rec.transcriber?.loadsInstantly ?? true) || rec.systemTranscriber != nil {
            await rec.transcriber?.unload()
            await rec.systemTranscriber?.unload()
        }
    }

    /// Per-channel segment clock: recording time the recognisers heard, less their decoding lag.
    private func asrClock(_ rec: Recording) -> Double { max(0, Double(rec.asrSamples) / 16 - rec.asrLatencyMs) }

    func onSamples(_ s: [Float], of owner: Recording? = nil, channels: (system: [Float], mic: [Float]?)? = nil) async {
        // Only the recording that captured these samples (an aborted one's late blocks never reach a newer file).
        guard let rec = recording, !s.isEmpty, owner == nil || owner === rec else { return }
        // Paused: keep the WAV timeline aligned with wall-clock (silence) but capture and transcribe nothing.
        let s = paused ? [Float](repeating: 0, count: s.count) : s
        do { try rec.wav.append(s) } catch { send(.error(code: "recording.write", message: "\(error)", fatal: false)) }
        rec.pendingLevel += s
        let t = now()
        if t - rec.lastLevelMs >= 100 {
            send(.audioLevel(rms: min(1, DSP.rms(rec.pendingLevel) * 4)))  // speech RMS sits ~0.05–0.25; scale for the meter
            rec.pendingLevel.removeAll(keepingCapacity: true)
            rec.lastLevelMs = t
        }
        let zero = [Float](repeating: 0, count: s.count)
        let sys = channels.map { $0.system.count == s.count ? $0.system : zero } ?? zero
        let mic = channels.map { $0.mic.flatMap { $0.count == s.count ? $0 : nil } ?? zero } ?? s
        // Diarizers hear the same timeline as the WAV (silence while paused).
        for run in rec.diarRuns where !run.offer(paused ? zero : (run.source == .mic ? mic : sys), cap: deps.diarizerBacklogSamples) {
            send(.log(level: .warn, msg: "diarizer \(run.source.rawValue) fell \(deps.diarizerBacklogSamples / 16_000) s behind: no more speaker turns on this channel for this recording"))
        }
        let b = AsrBlock(mix: s, system: sys, mic: mic, paused: paused)
        if rec.asrReady {
            await pushAsr(b, to: rec)
        } else {
            rec.backlog.append(b); rec.backlogSamples += s.count
            while rec.backlogSamples > Self.maxBacklogSamples, !rec.backlog.isEmpty {
                let old = rec.backlog.removeFirst()
                rec.backlogSamples -= old.mix.count
                rec.asrSamples += old.mix.count
            }
        }
    }

    func pollTranscript() async {
        guard let rec = recording, rec.asrReady, let tr = rec.transcriber else { return }
        let text = await tr.transcript()
        if let st = rec.systemTranscriber, let seg = rec.systemSegmenter {
            let t = asrClock(rec)
            for e in rec.segmenter.update(cumulative: text, nowMs: t) { send(.asr(e)) }
            for e in seg.update(cumulative: await st.transcript(), nowMs: t) { send(.asr(e)) }
            return
        }
        for e in rec.segmenter.update(cumulative: text, nowMs: now() - rec.startMs) { send(.asr(e)) }
    }

    func stopRecording() async {
        guard let rec = recording else { return }
        rec.ending = true
        rec.degradations?.discard()
        tick?.cancel(); tick = nil
        await rec.audio?.stop()
        echoReference.detach("recording")
        if rec.aborted { return }  // N2: recording.abort tore it down while the source was stopping
        rec.samplesIn?.finish()
        await rec.consumer?.value
        // Every captured block reached the module: close its audio (EOF ends its channels).
        speech.end(.recording(audioPath: rec.wav.url.path))
        if rec.aborted { return }
        if let tail = rec.pipeline?.finish(), !tail.isEmpty { await onSamples(tail, of: rec, channels: (tail, nil)) }
        // Audio still waiting for a loading recogniser is transcribed before the stop is acknowledged.
        if let prep = rec.prep { _ = await prep.value }
        if rec.aborted { return }
        if let st = rec.systemTranscriber, let seg = rec.systemSegmenter {
            let t = asrClock(rec)
            let micText = await rec.transcriber?.finish() ?? ""
            let sysText = await st.finish()
            if rec.aborted { return }
            for e in rec.segmenter.flush(cumulative: micText, nowMs: t) { send(.asr(e)) }
            for e in seg.flush(cumulative: sysText, nowMs: t) { send(.asr(e)) }
        } else if let tr = rec.transcriber {
            let text = await tr.finish()
            if rec.aborted { return }
            for e in rec.segmenter.flush(cumulative: text, nowMs: now() - rec.startMs) { send(.asr(e)) }
        }
        // Speaker turns flush before the stop ack, so the host has all of them when it finalizes.
        // N121: only a few seconds; a run still busy is closed out (its open turns sent) and cancelled.
        for run in rec.diarRuns { run.input.finish() }
        if !rec.diarRuns.isEmpty, !(await Self.waitAll(rec.diarRuns, ms: deps.diarizerStopWaitMs)) {
            for run in rec.diarRuns where run.isActive {
                send(.log(level: .warn, msg: "diarizer \(run.source.rawValue) still busy at stop: closed out"))
                run.closeOut()
            }
            for run in rec.diarRuns { lingering[run.source] = run.task }  // N125: stood-down runs may be mid-call too
        }
        lastDiarizerPeaks = Dictionary(rec.diarRuns.map { ($0.source, $0.peakQueued) }, uniquingKeysWith: max)
        rec.diarRuns.removeAll()
        await unloadAsr(rec)
        recording = nil
        paused = false
        do { try rec.wav.finish() } catch { send(.error(code: "recording.file", message: "\(error)", fatal: false)) }
        send(.recordingStopped(audioPath: rec.wav.url.path, durationMs: rec.wav.durationMs))
        recordingStartMs = nil
    }

    /// N2: the host gave up on recording.stop's ack. Tear down the recording writing `path` now, without the
    /// flush: no recogniser finish, no tail, and never waiting on the (possibly wedged) audio source. Its file is
    /// closed so no writer remains, its mic reader and permit are released through the arbiter, and a dictation is
    /// untouched. Always answers recording.aborted{audioPath}, also when nothing records that file (idempotent).
    func abortRecording(_ path: String) async {
        // Releases the reader if the stop never got this far (usually the stop already did: then a quiet no-op).
        let r = SessionArbiter.step(arbiter, .cmd("recording.abort", audioPath: path))
        arbiter = r.state
        await apply(r.effects)
        speech.end(.recording(audioPath: path))
        if let rec = recording, rec.wav.url.path == path {
            rec.aborted = true
            rec.ending = true
            rec.degradations?.discard()
            tick?.cancel(); tick = nil
            echoReference.detach("recording")
            recording = nil
            paused = false
            recordingStartMs = nil
            rec.samplesIn?.finish()
            rec.consumer?.cancel()
            for run in rec.diarRuns { run.end(); run.input.finish(); run.task?.cancel(); lingering[run.source] = run.task }
            rec.diarRuns.removeAll()
            if rec.asrReady { Task { [weak self] in await self?.unloadAsr(rec) } }  // else prepareAsr unloads when done
            if let audio = rec.audio { Task.detached { await audio.stop() } }  // may be what is wedged: don't wait
            do { try rec.wav.finish() } catch { send(.error(code: "recording.file", message: "\(error)", fatal: false)) }
            send(.log(level: .warn, msg: "recording aborted: \(path)"))
        }
        send(.recordingAborted(audioPath: path))
    }

    public func shutdown() async {
        guard !shuttingDown else { return }
        shuttingDown = true
        await stopRecording()
        permWatch?.cancel(); permWatch = nil
        await dictation.shutdown()
        speech.endAll()
        deps.meetings?.stop()
    }
}
