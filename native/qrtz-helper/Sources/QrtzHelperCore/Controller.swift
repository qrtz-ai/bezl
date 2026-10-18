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

/// The monolith helper's wrapper (ADR 0146 M2): reacts to host messages, owns what both modules share (the H6/H5
/// session arbiter and TCC polling, the mic hub's permits, permissions, meetings, the speech attach, summaries, the
/// native pill) and hands each command to its module: recordings to `NanoController`, dictation and the push key to
/// `EchoController`. Its public API is the one the executable and tests always used.
public actor HelperController {
    private let deps: HelperDeps
    private let send: @Sendable (HelperMessage) -> Void
    private let now: @Sendable () -> Double  // monotonic ms
    private var shuttingDown = false
    /// Nano's controller (recordings).
    nonisolated public let nano: NanoController
    /// Echo's controller (dictation, the armed mic).
    nonisolated public let echo: EchoController

    /// H6/H5 arbiter state (see Arbiter.swift); `lastPerms` is the TCC snapshot revocations are diffed against.
    public private(set) var arbiter: ArbiterState
    private var lastPerms: [OsPermission: PermissionState]
    private var permWatch: Task<Void, Never>?

    public init(deps: HelperDeps, send: @escaping @Sendable (HelperMessage) -> Void, now: @escaping @Sendable () -> Double) {
        // ADR 0092: the pill sees every line before it is written, so a key press shows it at once.
        var pillTiming = deps.pillTiming
        // N145: a release can't take longer than echo's own release budget (then the pill gives up on it).
        if pillTiming.finalizeBackstopMs == nil { pillTiming.finalizeBackstopMs = deps.dictationTail.tailMs + deps.dictationTail.maxWaitMs + 1_000 }
        let pill = deps.pill.map { PillDriver(view: $0, timing: pillTiming) }
        let raw = send
        let press = PressRelay()
        let send: @Sendable (HelperMessage) -> Void = { m in
            pill?.helper(m)
            raw(m)
            if case .hotkey(let a) = m { press.fire(a) }
        }
        self.pill = pill
        self.deps = deps; self.send = send; self.now = now
        let st = deps.permissions.status()
        self.lastPerms = st
        self.arbiter = ArbiterState(denied: OsPermission.allCases.filter { st[$0] == .denied })
        let speech = SpeechSinks(), echoReference = EchoReference()
        self.speech = speech; self.echoReference = echoReference
        // N143: granted means screen & system audio recording is already allowed, so a dictation never prompts.
        let perms = deps.permissions
        let gate = DictationEcho(reference: echoReference, tap: deps.dictationTap,
                                 permitted: { perms.status()[.screenRecording] == .granted })
        gate.onLog = { level, msg in send(.log(level: level, msg: msg)) }
        let dictation = DictationEngine(mic: deps.dictationMic, transcribers: deps.dictationTranscribers,
                                        inserter: deps.insertion.map { TextInserter(backend: $0) }, hotkeys: deps.hotkeys, send: send, now: now, managedMic: true,
                                        speech: speech, tail: deps.dictationTail, echo: gate)
        self.dictation = dictation
        let admit = AdmitRelay()
        let echo = EchoController(dictation: dictation, admit: admit)
        self.echo = echo
        self.nano = NanoController(deps: deps, send: send, now: now, speech: speech, echoReference: echoReference,
                                   afterWarm: { await dictation.prewarmIfKeyed() })
        self.summaries = SummarizeService(model: deps.summarizer, send: send)
        press.set { [weak echo] a in Task { await echo?.pushKey(a) } }
        admit.set { [weak self] e in await self?.admit(e) ?? false }
    }

    /// Test seam.
    public var isArmed: Bool { get async { await echo.isArmed } }
    /// ADR 0075: summarize requests, off the message queue.
    public let summaries: SummarizeService
    /// Echo's dictation engine (also `echo.dictation`).
    nonisolated public let dictation: DictationEngine
    /// N143: the system audio echo's mic is gated against (a recording's system channel, or echo's own tap).
    nonisolated public let echoReference: EchoReference
    /// The speech module's audio sinks (recordings and dictations).
    nonisolated public let speech: SpeechSinks

    /// ms since recording start (or since helper start when idle). Used for every atMs/startMs on the wire.
    nonisolated public func relativeMs() -> Double { nano.relativeMs() }

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

    public var isRecording: Bool { get async { await nano.isRecording } }
    public var paused: Bool { get async { await nano.paused } }
    /// N121 test seam: each channel's largest diarizer queue (samples) in the last recording.
    var lastDiarizerPeaks: [CaptureSource: Int] { get async { await nano.lastDiarizerPeaks } }

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
            if let path, let current = await nano.audioPath, current != path {
                send(.log(level: .warn, msg: "recording.stop for \(path) while recording \(current); ignored")); return
            }
            guard await admit(.cmd("recording.stop")) else { return }
            await nano.stopRecording()
        case .recordingAbort(let path): await abortRecording(path)
        case .recordingPause: await nano.setPaused(true)
        case .recordingResume: await nano.setPaused(false)
        case .modelsWarm: await nano.warmModels()
        case .shutdown: await shutdown()
        case .dictationConfigure(let warm, let pre, let stream, let key, let consume, let showPill, let autoInsert, let echoGate):
            pill?.enabled = showPill ?? true
            pill?.autoInsert = autoInsert ?? true
            await echo.configure(warmMic: warm, preRollMs: pre, streamAudio: stream, pushKey: key, consumeFn: consume, echoGate: echoGate)
            watchPermissions()
        case .dictationStart(let id):
            guard await admit(.cmd("dictation.start"), id: id) else { return }
            await echo.start(id: id)
            watchPermissions()
        case .dictationStop(let id): await echo.stop(id: id)
        case .dictationCancel(let id): await echo.cancel(id: id)
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

    /// An admitted recording.start: nano starts it; a file that can't be opened releases the admission (its mic
    /// reader and permit) and tells the host which start died.
    func startRecording(_ path: String) async {
        if let err = await nano.startRecording(path) {
            let r = SessionArbiter.step(arbiter, .recordingFileFailed(audioPath: path))
            arbiter = r.state
            await apply(r.effects)
            send(.error(code: "recording.file", message: err, fatal: false, audioPath: path))
            return
        }
        watchPermissions()
    }

    /// N2: releases the reader through the arbiter if the stop never got this far (usually the stop already did:
    /// then a quiet no-op), then nano tears the recording down and answers recording.aborted.
    func abortRecording(_ path: String) async {
        let r = SessionArbiter.step(arbiter, .cmd("recording.abort", audioPath: path))
        arbiter = r.state
        await apply(r.effects)
        await nano.abortRecording(path)
    }

    /// N138: the mic's device went away and didn't come back. A dictation ends with what was heard
    /// (`dictation.stopped{reason: mic-lost}`); a recording goes on with system audio and says so once
    /// (`capture.degraded{no-mic}`).
    func micDeviceLost(_ reason: String) async {
        send(.log(level: .warn, msg: "mic: input device lost (\(reason))"))
        if await dictation.isActive { await dictation.abort(reason: .micLost) }
        await nano.micDeviceLost(reason)
    }

    /// Test seams, forwarded to nano.
    func onSamples(_ s: [Float], of owner: NanoController.Recording? = nil, channels: (system: [Float], mic: [Float]?)? = nil) async {
        await nano.onSamples(s, of: owner, channels: channels)
    }

    @discardableResult
    func warmModels() async -> Task<Void, Never>? { await nano.warmModels() }

    public func shutdown() async {
        guard !shuttingDown else { return }
        shuttingDown = true
        await echo.beginShutdown()
        await nano.stopRecording()
        permWatch?.cancel(); permWatch = nil
        await echo.shutdown()
        speech.endAll()
        deps.meetings?.stop()
    }
}


/// Forwards the push key's lines from the send path to the controller (set once its init is done).
final class PressRelay: @unchecked Sendable {
    private let lock = NSLock()
    private var f: (@Sendable (HotkeyAction) -> Void)?
    func set(_ g: @escaping @Sendable (HotkeyAction) -> Void) { lock.withLock { f = g } }
    func fire(_ a: HotkeyAction) { lock.withLock { f }?(a) }
}
