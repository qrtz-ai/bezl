import Foundation

/// The monolith helper's platform (qrtz-helper runs echo and nano in one app, ADR 0146 M2): everything both parts
/// and their host need, split into `hostDeps`, `echoDeps` and `nanoDeps`.
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

    /// The monolith from the module apps' own platform parts (macOS: `makeEchoPlatform` and `makeNanoPlatform` on one
    /// shared mic hub and one model-load gate).
    public init(host: HostDeps, echo: EchoDeps, nano: NanoDeps) {
        self.init(permissions: host.permissions, meetings: nano.meetings, audio: nano.audio, transcribers: nano.transcribers,
                  helperVersion: host.helperVersion, os: host.os, features: host.features, context: nano.context, enhancer: nano.enhancer,
                  dictationMic: echo.dictationMic, dictationTranscribers: echo.dictationTranscribers, insertion: echo.insertion,
                  hotkeys: echo.hotkeys, micHub: host.micHub, permissionPollMs: host.permissionPollMs, silence: nano.silence,
                  speechFds: host.speechFds)
        systemTranscribers = nano.systemTranscribers; diarizers = nano.diarizers
        diarizerBacklogSamples = nano.diarizerBacklogSamples; diarizerStopWaitMs = nano.diarizerStopWaitMs
        modelWarmup = nano.modelWarmup; modelLoads = nano.modelLoads; summarizer = nano.summarizer
        preloadDictationAsr = echo.preloadDictationAsr; pill = echo.pill; pillTiming = echo.pillTiming
        dictationTail = echo.dictationTail; dictationTap = echo.dictationTap
    }

    /// The app's host part (hello, permissions, the arbiter, the speech attach).
    public var hostDeps: HostDeps {
        HostDeps(permissions: permissions, micHub: micHub, permissionPollMs: permissionPollMs, speechFds: speechFds,
                 helperVersion: helperVersion, os: os, features: features, module: "helper")
    }

    public var echoDeps: EchoDeps {
        EchoDeps(dictationMic: dictationMic, dictationTranscribers: dictationTranscribers, insertion: insertion, hotkeys: hotkeys,
                 dictationTap: dictationTap, dictationTail: dictationTail, pill: pill, pillTiming: pillTiming,
                 preloadDictationAsr: preloadDictationAsr, modelLoads: modelLoads)
    }

    public var nanoDeps: NanoDeps {
        var d = NanoDeps(meetings: meetings, audio: audio, transcribers: transcribers, context: context, enhancer: enhancer, silence: silence)
        d.systemTranscribers = systemTranscribers; d.diarizers = diarizers
        d.diarizerBacklogSamples = diarizerBacklogSamples; d.diarizerStopWaitMs = diarizerStopWaitMs
        d.modelWarmup = modelWarmup; d.modelLoads = modelLoads; d.summarizer = summarizer
        return d
    }
}

/// The monolith helper (ADR 0146 M2): one app running both module parts behind one host (`ModuleHost`: hello,
/// permissions, the speech attach and one H6/H5 arbiter they share): recordings, meetings and summaries in
/// `NanoController`, dictation, the pill and the push key in `EchoController`. The module apps (qrtz-echo,
/// qrtz-nano) run one part each behind a host of their own. Its public API is the one the executable and tests
/// always used.
public actor HelperController {
    nonisolated public let host: ModuleHost
    /// Nano's controller (recordings, meetings, summaries).
    nonisolated public let nano: NanoController
    /// Echo's controller (dictation, the pill, the armed mic).
    nonisolated public let echo: EchoController

    public init(deps: HelperDeps, send: @escaping @Sendable (HelperMessage) -> Void, now: @escaping @Sendable () -> Double) {
        let link = HostLink(send: send)
        let speech = SpeechSinks(), echoReference = EchoReference()
        let echo = EchoController(deps: deps.echoDeps, link: link, now: now, speech: speech, echoReference: echoReference,
                                  permissions: deps.permissions)
        let dictation = echo.dictation
        let nano = NanoController(deps: deps.nanoDeps, link: link, now: now, speech: speech, echoReference: echoReference,
                                  afterWarm: { await dictation.prewarmIfKeyed() })
        self.echo = echo; self.nano = nano; self.echoReference = echoReference
        // Echo first: it sees the shutdown before nano finishes the recording; nano owns models.warm here.
        self.host = ModuleHost(deps: deps.hostDeps, link: link, now: now, speech: speech, parts: [echo, nano])
    }

    /// Test seam.
    public var isArmed: Bool { get async { await echo.isArmed } }
    /// ADR 0075: summarize requests, off the message queue.
    nonisolated public var summaries: SummarizeService { nano.summaries }
    /// Echo's dictation engine (also `echo.dictation`).
    nonisolated public var dictation: DictationEngine { echo.dictation }
    /// N143: the system audio echo's mic is gated against (a recording's system channel, or echo's own tap).
    nonisolated public let echoReference: EchoReference
    /// The speech module's audio sinks (recordings and dictations).
    nonisolated public var speech: SpeechSinks { host.speech }
    /// ADR 0092: the native pill's state machine (nil without a pill view).
    nonisolated public var pill: PillDriver? { echo.pill }
    /// H6/H5 arbiter state.
    public var arbiter: ArbiterState { get async { await host.arbiter } }

    /// ms since recording start (or since helper start when idle). Used for every atMs/startMs on the wire.
    nonisolated public func relativeMs() -> Double { nano.relativeMs() }

    public func start() async { await host.start() }

    /// ADR 0087: the startup load of echo's recogniser (tests await it).
    public var startupPreload: Task<Void, Never>? { get async { await echo.startupPreload } }

    public var isRecording: Bool { get async { await nano.isRecording } }
    public var paused: Bool { get async { await nano.paused } }
    public func handle(_ msg: HostMessage) async { await host.handle(msg) }
    public func heartbeat() async { await host.heartbeat() }
    public func checkPermissions() async { await host.checkPermissions() }
    func watchPermissions() async { await host.watchPermissions() }

    public func shutdown() async { await host.shutdown() }
}
