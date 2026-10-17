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
    func stop() async
}

/// A streaming recogniser that exposes one cumulative transcript.
public protocol StreamingTranscriber: AnyObject, Sendable {
    var name: String { get }
    func prepare() async throws
    func append(_ samples16k: [Float]) async
    func transcript() async -> String
    func finish() async -> String
    func reset() async
}

public struct HelperDeps: Sendable {
    public var permissions: PermissionsProvider
    public var meetings: MeetingSource?
    /// Tried in order; the first that starts wins (e.g. Core Audio tap, then ScreenCaptureKit).
    public var audio: [AudioSource]
    /// Tried in order (e.g. Parakeet via FluidAudio, then Apple SpeechAnalyzer).
    public var transcribers: [StreamingTranscriber]
    public var helperVersion: String
    public var os: String
    public var features: [String]
    /// Shared AX → audio context (who's speaking, who am I); feeds the echo gate.
    public var context: MeetingContext
    /// Optional AEC (LocalVQE on macOS); the echo gate is always the fallback.
    public var enhancer: MicEnhancer?
    /// qrtz talk: mic-only source, its own recogniser instances (so dictation works during a meeting), insertion, keys.
    public var dictationMic: MicSource?
    public var dictationTranscribers: [StreamingTranscriber]
    public var insertion: InsertionBackend?
    public var hotkeys: HotkeyMonitor?
    /// H6: the shared mic tap. When set, dictation and meeting capture read the mic through views of it, and a
    /// microphone revocation drops every reader at once.
    public var micHub: MicHub?
    /// H5: how often TCC state is re-read while capturing or dictating (0 = never poll; tests call checkPermissions()).
    public var permissionPollMs: Int
    public init(permissions: PermissionsProvider, meetings: MeetingSource?, audio: [AudioSource], transcribers: [StreamingTranscriber], helperVersion: String, os: String, features: [String], context: MeetingContext = MeetingContext(), enhancer: MicEnhancer? = nil,
                dictationMic: MicSource? = nil, dictationTranscribers: [StreamingTranscriber] = [], insertion: InsertionBackend? = nil, hotkeys: HotkeyMonitor? = nil,
                micHub: MicHub? = nil, permissionPollMs: Int = 2000) {
        self.micHub = micHub; self.permissionPollMs = permissionPollMs
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
        let segmenter = UtteranceSegmenter()
        var lastLevelMs: Double = 0
        var pendingLevel: [Float] = []
        var samplesIn: AsyncStream<CaptureBlock>.Continuation?
        var pipeline: MicPipeline?
        var consumer: Task<Void, Never>?
        init(wav: WavWriter, startMs: Double) { self.wav = wav; self.startMs = startMs }
    }

    /// H6/H5 arbiter state (see Arbiter.swift); `lastPerms` is the TCC snapshot revocations are diffed against.
    public private(set) var arbiter: ArbiterState
    private var lastPerms: [OsPermission: PermissionState]
    private var permWatch: Task<Void, Never>?

    public init(deps: HelperDeps, send: @escaping @Sendable (HelperMessage) -> Void, now: @escaping @Sendable () -> Double) {
        self.deps = deps; self.send = send; self.now = now
        let st = deps.permissions.status()
        self.lastPerms = st
        self.arbiter = ArbiterState(denied: OsPermission.allCases.filter { st[$0] == .denied })
        self.dictation = DictationEngine(mic: deps.dictationMic, transcribers: deps.dictationTranscribers,
                                         inserter: deps.insertion.map { TextInserter(backend: $0) }, hotkeys: deps.hotkeys, send: send, now: now)
    }
    public let dictation: DictationEngine

    /// ms since recording start (or since helper start when idle). Used for every atMs/startMs on the wire.
    nonisolated public func relativeMs() -> Double {
        let n = now()
        return max(0, n - (recordingStartMs ?? 0))
    }

    public func start() {
        send(.hello(protocol: helperProtocolVersion, helperVersion: deps.helperVersion, os: deps.os, features: deps.features))
        send(.permissionsStatus(deps.permissions.status()))
        deps.meetings?.start(emit: send, clock: { [weak self] in self?.relativeMs() ?? 0 })
    }

    public var isRecording: Bool { recording != nil }

    public func handle(_ msg: HostMessage) async {
        switch msg {
        case .ping(let id): send(.pong(id: id))
        case .permissionsCheck: send(.permissionsStatus(deps.permissions.status()))
        case .permissionsRequest(let p):
            await deps.permissions.request(p)
            send(.permissionsStatus(deps.permissions.status()))
        case .recordingStart(let path):
            guard admit(.cmd("recording.start")) else { return }
            await startRecording(path)
        case .recordingStop:
            guard admit(.cmd("recording.stop")) else { return }
            await stopRecording()
        case .recordingPause: paused = true; send(.log(level: .info, msg: "capture paused"))
        case .recordingResume: paused = false; send(.log(level: .info, msg: "capture resumed"))
        case .shutdown: await shutdown()
        case .dictationConfigure(let warm, let pre, let stream, let key):
            await dictation.configure(warmMic: warm, preRollMs: pre, streamAudio: stream, pushKey: key)
        case .dictationStart(let id):
            guard admit(.cmd("dictation.start")) else { return }
            await dictation.start(id: id)
            if await !dictation.isActive { arbiter.dictating = false; arbiter.mic.removeAll { $0 == "dictation" } }
            watchPermissions()
        case .dictationStop(let id):
            guard await dictation.activeId == id, admit(.cmd("dictation.stop")) else { return }
            await dictation.stop(id: id)
        case .dictationCancel(let id):
            guard await dictation.activeId == id, admit(.cmd("dictation.cancel")) else { return }
            await dictation.cancel(id: id)
        case .insertText(let id, let text, let restore): await dictation.insert(id: id, text: text, restoreClipboard: restore)
        }
    }

    public func heartbeat() { send(.heartbeat(ts: now())) }

    /// Runs a command through the arbiter. Ignored commands are logged; a refused dictation.start is an error.
    private func admit(_ e: ArbiterEvent) -> Bool {
        let r = SessionArbiter.step(arbiter, e)
        arbiter = r.state
        guard case .cmd(let c) = e else { return true }
        switch r.decision {
        case "ignore":
            send(.log(level: .warn, msg: "\(c) while \(r.reason ?? "busy"); ignored")); return false
        case "refuse":
            send(.error(code: "dictation.mic", message: "microphone access denied", fatal: false)); return false
        default:
            if r.mux { send(.log(level: .info, msg: "mux: recording and dictation share the mic tap")) }
            return true
        }
    }

    /// H5: polls TCC while anything captures; stops by itself once idle.
    func watchPermissions() {
        guard permWatch == nil, deps.permissionPollMs > 0, arbiter.recording || arbiter.dictating else { return }
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
        if arbiter.recording || arbiter.dictating { return true }
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
                arbiter = r.state
                guard r.decision == "accept" else { continue }
                send(.permissionRevoked(permission: p, atMs: relativeMs()))
                for fx in r.effects {
                    if fx == "dictation.stop:permission-revoked" { await dictation.abort(reason: .permissionRevoked) }
                    if fx.hasPrefix("mic.unsubscribe:") { deps.micHub?.unsubscribe(String(fx.dropFirst("mic.unsubscribe:".count))) }
                }
                if p == .microphone { deps.micHub?.dropAll() }
            } else if now == .granted, was == .denied {
                arbiter = SessionArbiter.step(arbiter, .granted(p)).state
            }
        }
    }

    func startRecording(_ path: String) async {
        if recording != nil { send(.log(level: .warn, msg: "recording.start while recording; ignored")); return }
        let wav: WavWriter
        do { wav = try WavWriter(url: URL(fileURLWithPath: path)) } catch {
            send(.error(code: "recording.file", message: "\(error)", fatal: false)); return
        }
        let rec = Recording(wav: wav, startMs: now())
        recordingStartMs = rec.startMs
        recording = rec

        for t in deps.transcribers {
            do { try await t.prepare(); await t.reset(); rec.transcriber = t; send(.log(level: .info, msg: "asr: \(t.name)")); break }
            catch { send(.log(level: .warn, msg: "asr \(t.name) unavailable: \(error)")) }
        }
        if rec.transcriber == nil { send(.error(code: "asr.unavailable", message: "no speech recogniser available; recording audio only", fatal: false)) }

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
                let mixed = await pipeline.process(b)
                await self?.onSamples(mixed)
            }
        }
        for a in deps.audio {
            do {
                try await a.start(onBlock: { b in samplesIn.yield(b) })
                rec.audio = a
                send(.log(level: .info, msg: "audio: \(a.name)"))
                break
            } catch { send(.log(level: .warn, msg: "audio \(a.name) failed: \(error)")) }
        }
        if rec.audio == nil { send(.error(code: "audio.unavailable", message: "no audio source could start (check Screen & System Audio Recording and Microphone permissions)", fatal: false)) }

        send(.recordingStarted(audioPath: path))
        watchPermissions()
        tick = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 250_000_000)
                await self?.pollTranscript()
            }
        }
    }

    func onSamples(_ s: [Float]) async {
        guard let rec = recording, !s.isEmpty else { return }
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
        if !paused { await rec.transcriber?.append(s) }
    }

    func pollTranscript() async {
        guard let rec = recording, let tr = rec.transcriber else { return }
        let text = await tr.transcript()
        for e in rec.segmenter.update(cumulative: text, nowMs: now() - rec.startMs) { send(.asr(e)) }
    }

    func stopRecording() async {
        guard let rec = recording else { return }
        tick?.cancel(); tick = nil
        await rec.audio?.stop()
        rec.samplesIn?.finish()
        await rec.consumer?.value
        if let tail = rec.pipeline?.finish(), !tail.isEmpty { await onSamples(tail) }
        if let tr = rec.transcriber {
            let text = await tr.finish()
            for e in rec.segmenter.flush(cumulative: text, nowMs: now() - rec.startMs) { send(.asr(e)) }
        }
        recording = nil
        paused = false
        do { try rec.wav.finish() } catch { send(.error(code: "recording.file", message: "\(error)", fatal: false)) }
        send(.recordingStopped(audioPath: rec.wav.url.path, durationMs: rec.wav.durationMs))
        recordingStartMs = nil
    }

    public func shutdown() async {
        guard !shuttingDown else { return }
        shuttingDown = true
        await stopRecording()
        permWatch?.cancel(); permWatch = nil
        await dictation.shutdown()
        deps.meetings?.stop()
    }
}
