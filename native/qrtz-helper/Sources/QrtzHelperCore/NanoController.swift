import Foundation

/// Nano's half of the helper (ADR 0146 M2): one recording at a time (capture, WAV, per-channel ASR, diarizers,
/// silence, the speech module's recording sinks, `models.warm`). The mic and the TCC arbiter belong to the wrapper
/// (`HelperController`), which admits every command before it reaches this actor; nothing here touches echo's state.
/// When nano runs as its own app (qrtz-nano) this actor is its controller, with an arbiter of its own.
public actor NanoController {
    private let deps: HelperDeps
    private let send: @Sendable (HelperMessage) -> Void
    private let now: @Sendable () -> Double  // monotonic ms
    private let speech: SpeechSinks
    /// N143: a running recording publishes its system channel here (echo's gate reads it).
    nonisolated let echoReference: EchoReference
    /// Runs after a `models.warm` that wasn't cancelled (the monolith prewarms echo's recogniser then).
    private let afterWarm: @Sendable () async -> Void
    private var recording: Recording?
    private var tick: Task<Void, Never>?
    public private(set) var paused = false
    nonisolated(unsafe) private var recordingStartMs: Double?

    init(deps: HelperDeps, send: @escaping @Sendable (HelperMessage) -> Void, now: @escaping @Sendable () -> Double,
         speech: SpeechSinks, echoReference: EchoReference, afterWarm: @escaping @Sendable () async -> Void = {}) {
        self.deps = deps; self.send = send; self.now = now
        self.speech = speech; self.echoReference = echoReference; self.afterWarm = afterWarm
    }

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

    /// ms since recording start (or since helper start when idle). Used for every atMs/startMs on the wire.
    nonisolated public func relativeMs() -> Double {
        let n = now()
        return max(0, n - (recordingStartMs ?? 0))
    }

    public var isRecording: Bool { recording != nil }
    /// The file the running recording writes (nil when idle).
    var audioPath: String? { recording?.wav.url.path }

    func setPaused(_ p: Bool) {
        paused = p
        send(.log(level: .info, msg: p ? "capture paused" : "capture resumed"))
    }

    /// Starts recording to `path` (already admitted). Returns the error text when the file can't be opened (nothing
    /// started); nil otherwise.
    func startRecording(_ path: String) async -> String? {
        if recording != nil { send(.log(level: .warn, msg: "recording.start while recording; ignored")); return nil }
        if let w = warmTask { w.cancel(); send(.log(level: .info, msg: "models.warm cancelled: recording")) }
        let wav: WavWriter
        do { wav = try WavWriter(url: URL(fileURLWithPath: path)) } catch {
            // Fail closed: a recording never runs without a file. Nothing has started yet (no source, no ASR, no
            // started{}); the wrapper releases the admitted recording and tells the host which start died.
            return "\(error)"
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
        tick = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 250_000_000)
                await self?.pollTranscript()
            }
        }
        return nil
    }

    /// N138: the mic's device went away and didn't come back: a recording goes on with system audio and says so
    /// once (`capture.degraded{no-mic}`).
    func micDeviceLost(_ reason: String) {
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
        let send = self.send, after = self.afterWarm
        let t = Task.detached { [weak self] in
            send(.log(level: .info, msg: await warm()))
            await self?.warmDone()
            if !Task.isCancelled { await after() }  // e.g. talk's own instance, now from the compiled cache
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
    func abortRecording(_ path: String) {
        // The wrapper already ran recording.abort through the arbiter (releasing the reader if the stop never got
        // that far).
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
}
