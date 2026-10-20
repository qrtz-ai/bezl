import Foundation
import QrtzShared

/// What echo needs from its platform (ADR 0146 M2.5; the macOS seams come from QrtzEchoMac).
public struct EchoDeps: Sendable {
    /// qrtz talk: mic-only source, its own recogniser instances (so dictation works during a meeting), insertion, keys.
    public var dictationMic: MicSource?
    public var dictationTranscribers: [StreamingTranscriber]
    public var insertion: InsertionBackend?
    public var hotkeys: HotkeyMonitor?
    /// N143: a system-audio tap echo runs for a dictation's length when the speakers play (nil: no gate without a
    /// meeting). A recording's own system channel is used instead while one runs in the same app.
    public var dictationTap: SystemAudioTap?
    /// Echo: what the helper does between the key release and `dictation.stopped` (tail, drain, pad, finalize).
    public var dictationTail: DictationTail
    /// ADR 0092: the native dictation pill (macOS, when the host asks for it with `QRTZ_NATIVE_PILL=1`).
    public var pill: PillView?
    public var pillTiming: PillDriver.Timing
    /// ADR 0087: load echo's recogniser when the app starts (kept for its life). Off in tests that count loads.
    public var preloadDictationAsr: Bool
    /// N137: the per-kind load gate the startup preload goes through.
    public var modelLoads: ModelLoads
    public init(dictationMic: MicSource? = nil, dictationTranscribers: [StreamingTranscriber] = [], insertion: InsertionBackend? = nil,
                hotkeys: HotkeyMonitor? = nil, dictationTap: SystemAudioTap? = nil, dictationTail: DictationTail = DictationTail(),
                pill: PillView? = nil, pillTiming: PillDriver.Timing = PillDriver.Timing(), preloadDictationAsr: Bool = true,
                modelLoads: ModelLoads = ModelLoads()) {
        self.dictationMic = dictationMic; self.dictationTranscribers = dictationTranscribers; self.insertion = insertion
        self.hotkeys = hotkeys; self.dictationTap = dictationTap; self.dictationTail = dictationTail
        self.pill = pill; self.pillTiming = pillTiming; self.preloadDictationAsr = preloadDictationAsr; self.modelLoads = modelLoads
    }
}

/// Echo's half of the helper (ADR 0146 M2): the dictation engine, the native pill and the push key's armed mic
/// (docs/echo/dictation-tuning.md item 2). The mic itself is admitted by its app's arbiter (`ModuleHost`), reached
/// through the `HostLink`; nothing here touches a recording. In qrtz-echo this is the app's only part, with the
/// app's own arbiter; in the monolith it shares one with nano.
public actor EchoController: ModulePart {
    public nonisolated let dictation: DictationEngine
    /// ADR 0092: the native pill's state machine (nil without a pill view).
    public nonisolated let pill: PillDriver?
    private let link: HostLink
    private let preloadAsr: Bool
    /// N174: answers `models.warm` itself (qrtz-echo; in the monolith nano's warm prewarms echo afterwards).
    private let ownsWarm: Bool
    private var shuttingDown = false
    /// `dictation.configure{warmMic}` as the host set it (the arm holds `warm` on top of it).
    private var configuredWarm = false
    /// A press opened the mic and no dictation.start has taken it yet.
    private var armed = false
    private var armGen = 0
    /// The arm's mic reader belongs to the dictation that started from it: released at its stop or cancel.
    private var armHeld = false
    /// Everything since the press is kept this long for the start (talk can queue a press behind a finishing
    /// dictation, N110). A press that never starts lets the mic go `armGraceMs` after the key comes up.
    static let armMaxMs = 10_000
    static let armGraceMs = 2_000

    /// `ownsWarm`: this app has no nano to warm first, so `models.warm` prewarms echo's recogniser directly.
    public init(deps: EchoDeps, link: HostLink, now: @escaping @Sendable () -> Double, speech: SpeechSinks,
                echoReference: EchoReference, permissions: PermissionsProvider, ownsWarm: Bool = false) {
        // ADR 0092: the pill sees every line before it is written, so a key press shows it at once.
        var pillTiming = deps.pillTiming
        // N145: a release can't take longer than echo's own release budget (then the pill gives up on it).
        if pillTiming.finalizeBackstopMs == nil { pillTiming.finalizeBackstopMs = deps.dictationTail.tailMs + deps.dictationTail.maxWaitMs + 1_000 }
        self.pill = deps.pill.map { PillDriver(view: $0, timing: pillTiming) }
        let send = link.sender
        // N143: granted means screen & system audio recording is already allowed, so a dictation never prompts.
        let gate = DictationEcho(reference: echoReference, tap: deps.dictationTap,
                                 permitted: { permissions.status()[.screenRecording] == .granted })
        gate.onLog = { level, msg in send(.log(level: level, msg: msg)) }
        self.dictation = DictationEngine(mic: deps.dictationMic, transcribers: deps.dictationTranscribers,
                                         inserter: deps.insertion.map { TextInserter(backend: $0) }, hotkeys: deps.hotkeys, send: send, now: now,
                                         managedMic: true, speech: speech, tail: deps.dictationTail, echo: gate,
                                         micPermission: { permissions.status()[.microphone] }, loads: deps.modelLoads)
        self.link = link; self.preloadAsr = deps.preloadDictationAsr; self.ownsWarm = ownsWarm
    }

    /// ADR 0087: the startup load of echo's recogniser (tests await it).
    public private(set) var startupPreload: Task<Void, Never>?

    public func start() {
        guard preloadAsr else { return }
        // Inside the "asr" gate (N137), so it never compiles alongside a recording's loads; a press meanwhile
        // doesn't wait for the gate: it starts (or joins) the transcriber's one load itself.
        startupPreload = dictation.preload(gated: true)
    }

    public nonisolated func owns(_ m: HostMessage) -> Bool {
        switch m {
        case .dictationConfigure, .dictationStart, .dictationStop, .dictationCancel, .insertText, .pillFlash, .notchShow, .notchUpdate, .notchDismiss: return true
        case .modelsWarm: return ownsWarm
        default: return false
        }
    }

    public func handle(_ m: HostMessage) async {
        switch m {
        case .dictationConfigure(let warm, let pre, let stream, let key, let consume, let showPill, let autoInsert, let echoGate, let motion):
            pill?.enabled = showPill ?? true
            pill?.motion = motion ?? NotchMotion()
            pill?.autoInsert = autoInsert ?? true
            await configure(warmMic: warm, preRollMs: pre, streamAudio: stream, pushKey: key, consumeFn: consume, echoGate: echoGate)
            await link.watchPermissions()
        case .dictationStart(let id):
            guard await link.admit(.cmd("dictation.start"), id: id) else { return }
            await start(id: id)
            await link.watchPermissions()
        case .dictationStop(let id): await stop(id: id)
        case .dictationCancel(let id): await cancel(id: id)
        case .insertText(let id, let text, let restore): await dictation.insert(id: id, text: text, restoreClipboard: restore)
        case .pillFlash, .notchShow, .notchUpdate, .notchDismiss: break  // the pill driver took it (`observe`); without a native pill there is nothing to show
        case .modelsWarm:
            // N174: a download (or a warm) finished; load the recogniser now, not on the first press.
            await dictation.prewarmIfKeyed(log: true)
        default: break
        }
    }

    public nonisolated func observe(_ m: HostMessage) { pill?.host(m) }
    public nonisolated func willSend(_ m: HelperMessage) { pill?.helper(m) }
    public nonisolated func didSend(_ m: HelperMessage) {
        if case .hotkey(let a, _, _) = m { Task { [weak self] in await self?.pushKey(a) } }
    }

    public func apply(effect: String) async {
        switch effect {
        case "mic.subscribe:dictation": _ = await dictation.attachMic()
        case "mic.unsubscribe:dictation": await dictation.detachMic()
        case "dictation.stop:permission-revoked": await dictation.abort(reason: .permissionRevoked)
        default: break
        }
    }

    public func micRevoked() async { if await !dictation.isActive { await dictation.micDropped() } }
    public func micDeviceLost(_ reason: String) async { if await dictation.isActive { await dictation.abort(reason: .micLost) } }
    public func activeDictationId() async -> String? { await dictation.activeId }

    /// Test seam.
    public var isArmed: Bool { armed }

    /// N195 (ADR 0224): after an admit that may have subscribed the dictation reader, a mic that didn't open is
    /// stepped back out of the arbiter (`micFailed`), so the next press or start subscribes again instead of
    /// reusing a reader that never attached. Ignored by the arbiter when the reader isn't subscribed.
    private func settleMic() async {
        if await !dictation.isMicRunning { await link.step(.micFailed("dictation")) }
    }

    /// The push key's lines, seen as they are sent (the arm doesn't wait for the host).
    func pushKey(_ a: HotkeyAction) async {
        switch a {
        case .down:
            guard !armed, !configuredWarm, !shuttingDown, await dictation.canArm else { return }
            armed = true; armGen += 1
            await dictation.arm(maxMs: Self.armMaxMs)
            _ = await link.admit(.configure(warmMic: true))
            await settleMic()
        case .up:
            guard armed else { return }
            let g = armGen
            Task { [weak self] in
                try? await Task.sleep(nanoseconds: UInt64(Self.armGraceMs) * 1_000_000)
                await self?.disarmIfUnstarted(g)
            }
        }
    }

    private func disarmIfUnstarted(_ g: Int) async {
        guard armed, armGen == g else { return }
        armed = false
        await dictation.disarm()
        await releaseArm()
    }

    /// Back to the host's warm setting (unsubscribes the reader when nothing else holds it).
    private func releaseArm() async {
        guard !armed else { return }
        armHeld = false
        _ = await link.admit(.configure(warmMic: configuredWarm))
    }

    /// `dictation.configure` (the pill's flags were set by `handle`).
    func configure(warmMic warm: Bool, preRollMs pre: Int, streamAudio stream: Bool, pushKey key: String,
                   consumeFn consume: Bool?, echoGate: Bool?) async {
        await dictation.setEchoGate(echoGate ?? true)
        configuredWarm = warm
        if warm, armed { armed = false; armHeld = false; await dictation.disarm() }
        await dictation.configure(warmMic: warm, preRollMs: pre, streamAudio: stream, pushKey: key, consumeFn: consume ?? true)
        _ = await link.admit(.configure(warmMic: warm || armed || armHeld))
        await settleMic()
    }

    /// `dictation.start`, after the arbiter admitted it.
    func start(id: String) async {
        let fromArm = armed
        if fromArm { armed = false; armHeld = true }
        await dictation.start(id: id)
        await settleMic()
        // The engine could not start (no source, no recogniser, no mic): undo through the arbiter so the reader is
        // released too.
        if await !dictation.isActive {
            _ = await link.admit(.cmd("dictation.cancel"))
            if armHeld { await releaseArm() }
        }
    }

    func stop(id: String) async {
        guard await dictation.activeId == id else { return }
        await dictation.stop(id: id)
        _ = await link.admit(.cmd("dictation.stop"))
        if armHeld { await releaseArm() }
    }

    func cancel(id: String) async {
        guard await dictation.activeId == id else { return }
        await dictation.cancel(id: id)
        _ = await link.admit(.cmd("dictation.cancel"))
        if armHeld { await releaseArm() }
    }

    /// No new arm from here on (the host then stops any recording and calls `shutdown`).
    public func beginShutdown() { shuttingDown = true }

    public func shutdown() async { await dictation.shutdown() }
}
