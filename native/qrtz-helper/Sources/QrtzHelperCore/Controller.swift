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
        /// N2: torn down by recording.abort; an in-flight stop for it must not touch anything any more.
        var aborted = false
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
                                         inserter: deps.insertion.map { TextInserter(backend: $0) }, hotkeys: deps.hotkeys, send: send, now: now, managedMic: true)
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
        case .shutdown: await shutdown()
        case .dictationConfigure(let warm, let pre, let stream, let key):
            await dictation.configure(warmMic: warm, preRollMs: pre, streamAudio: stream, pushKey: key)
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
                await self?.onSamples(mixed, of: rec)
            }
        }
        for (i, a) in deps.audio.enumerated() {
            do {
                try await a.start(onBlock: { b in samplesIn.yield(b) })
                rec.audio = a
                send(.log(level: .info, msg: "audio: \(a.name)"))
                break
            } catch {
                // H8: structured, so the host can tell which source the recording fell back to and why.
                let next = i + 1 < deps.audio.count ? deps.audio[i + 1].name : nil
                send(.captureFallback(from: a.name, to: next, reason: "\(error)"))
            }
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

    func onSamples(_ s: [Float], of owner: Recording? = nil) async {
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
        if rec.aborted { return }  // N2: recording.abort tore it down while the source was stopping
        rec.samplesIn?.finish()
        await rec.consumer?.value
        if rec.aborted { return }
        if let tail = rec.pipeline?.finish(), !tail.isEmpty { await onSamples(tail, of: rec) }
        if let tr = rec.transcriber {
            let text = await tr.finish()
            if rec.aborted { return }
            for e in rec.segmenter.flush(cumulative: text, nowMs: now() - rec.startMs) { send(.asr(e)) }
        }
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
        if let rec = recording, rec.wav.url.path == path {
            rec.aborted = true
            tick?.cancel(); tick = nil
            recording = nil
            paused = false
            recordingStartMs = nil
            rec.samplesIn?.finish()
            rec.consumer?.cancel()
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
        deps.meetings?.stop()
    }
}
