import Foundation
import QrtzShared

// qrtz talk: push-to-talk dictation. Foundation-only so the logic (pre-roll, insertion fallback order,
// clipboard restore, PCM framing) is tested on Linux; the macOS executable supplies the real seams.

/// Native key monitor for push-to-talk (Electron global shortcuts only report key-down).
public extension HotkeyMonitor {
    var pressWithControl: Bool { false }
}

public protocol HotkeyMonitor: AnyObject, Sendable {
    /// key: "fn", "right-option", "right-command", "right-control". consumeFn: with `fn`, swallow the Fn/globe events
    /// so macOS doesn't also run its globe action ([`HotkeyFilter`]). Returns false if unsupported or not permitted.
    func start(key: String, consumeFn: Bool, onChange: @escaping @Sendable (HotkeyAction) -> Void) -> Bool
    /// ADR 0216: the current press began as ctrl+fn (read when an edge is reported).
    var pressWithControl: Bool { get }
    func stop()
}

/// The OS calls text insertion needs. Each AX call returns false when the focused element refuses it.
public protocol InsertionBackend: AnyObject, Sendable {
    func focusedAppName() -> String?
    func axSetSelectedText(_ text: String) -> Bool
    func axInsertIntoValue(_ text: String) -> Bool
    /// Opaque snapshot of every pasteboard item/type so it can be restored exactly.
    func pasteboardSnapshot() -> Any?
    func pasteboardSetString(_ text: String)
    func pasteboardRestore(_ snapshot: Any?)
    func pasteboardChangeCount() -> Int
    /// Posts Cmd-V to the focused app. False if events can't be posted (Accessibility not granted).
    func postPaste() -> Bool
    /// ADR 0225: the focused element is a password field (`SecureField`), or secure event input is on. Asked right
    /// before every insert step, never cached from the press: focus can move while the user speaks.
    func focusedFieldIsSecure() -> Bool
}

/// Ring buffer of the last N samples, so the words said just before the key press aren't clipped.
public struct PreRollBuffer {
    public private(set) var capacity: Int
    private var buf: [Float] = []
    public init(capacity: Int) { self.capacity = max(0, capacity) }
    public mutating func setCapacity(_ c: Int) { capacity = max(0, c); trim() }
    public mutating func append(_ s: [Float]) { buf += s; trim() }
    public mutating func drain() -> [Float] { defer { buf.removeAll(keepingCapacity: true) }; return buf }
    public var count: Int { buf.count }
    private mutating func trim() { if buf.count > capacity { buf.removeFirst(buf.count - capacity) } }
}

public enum PCM16 {
    /// 16-bit little-endian PCM, base64 (what cloud streaming engines take).
    public static func base64(_ s: [Float]) -> String {
        var d = Data(capacity: s.count * 2)
        for v in s {
            let i = Int16(max(-1, min(1, v)) * Float(Int16.max))
            withUnsafeBytes(of: i.littleEndian) { d.append(contentsOf: $0) }
        }
        return d.base64EncodedString()
    }
}

/// Inserts text into the focused app: AX selected text (replaces the selection / types at the caret), then AX value
/// splice, then pasteboard + Cmd-V with the user's clipboard put back afterwards.
///
/// The paste fallback answers as soon as Cmd-V is posted (the text is in): the user's clipboard is put back
/// `restoreDelayMs` later in the background. A next insert waits for that restore before it touches the pasteboard,
/// so back-to-back pastes never lose the user's clipboard.
public final class TextInserter: @unchecked Sendable {
    let backend: InsertionBackend
    let restoreDelayNs: UInt64
    private let lock = NSLock()
    private var pendingRestore: Task<Void, Never>?
    public init(backend: InsertionBackend, restoreDelayMs: Int = 250) {
        self.backend = backend; self.restoreDelayNs = UInt64(restoreDelayMs) * 1_000_000
    }

    /// The clipboard restore of the last paste, if one is still waiting (tests, shutdown).
    public func restored() async { await lock.withLock { pendingRestore }?.value }

    public func insert(_ text: String, restoreClipboard: Bool) async -> (method: InsertMethod, ok: Bool, app: String?, error: String?) {
        let app = backend.focusedAppName()
        guard !text.isEmpty else { return (.none, true, app, nil) }
        // ADR 0225: never type into a password field: nothing is inserted and the pasteboard is not touched.
        let refused: (method: InsertMethod, ok: Bool, app: String?, error: String?) = (.none, false, app, SecureField.insertRefused)
        if backend.focusedFieldIsSecure() { return refused }
        if backend.axSetSelectedText(text) { return (.axSelectedText, true, app, nil) }
        if backend.axInsertIntoValue(text) { return (.axValue, true, app, nil) }
        // The previous paste's restore first: its snapshot is the user's clipboard, not our text.
        await restored()
        // ADR 0225: again right before the pasteboard (the AX tries and the restore wait take time).
        if backend.focusedFieldIsSecure() { return refused }
        let snap = restoreClipboard ? backend.pasteboardSnapshot() : nil
        backend.pasteboardSetString(text)
        let ours = backend.pasteboardChangeCount()
        guard backend.postPaste() else {
            if restoreClipboard { backend.pasteboardRestore(snap) }
            // ADR 0225: focus moved into a password field between the check and Cmd-V: say so, so the host keeps nothing.
            if backend.focusedFieldIsSecure() { return refused }
            return (.none, false, app, "cannot post Cmd-V (Accessibility permission?)")
        }
        if restoreClipboard {
            // Give the target app time to read the pasteboard, then put the user's clipboard back unless
            // something else wrote to it meanwhile. Not awaited: insert.result goes out now.
            let backend = self.backend, delay = restoreDelayNs
            let t = Task {
                try? await Task.sleep(nanoseconds: delay)
                if backend.pasteboardChangeCount() == ours { backend.pasteboardRestore(snap) }
            }
            lock.withLock { pendingRestore = t }
        }
        return (.paste, true, app, nil)
    }
}

/// What happens between the key release (`dictation.stop`) and `dictation.stopped`, so the last words get in:
/// people let go while the last syllable is still coming, the mic delivers in ~85 ms blocks that are still queued,
/// and a streaming recogniser (Nemotron: 560 ms chunks) only decodes audio that has a full chunk after it.
public struct DictationTail: Sendable, Equatable {
    /// Keep capturing this long after the release (real audio: the end of the last word, the tap's latency).
    public var tailMs: Int
    /// Then wait for the mic blocks already queued to reach the recogniser: this long, plus 2 ms per ms of audio
    /// queued at the release (a recogniser at half real time still catches up), within `maxWaitMs`.
    public var drainTimeoutMs: Int
    /// Then feed this much silence (≥ one recogniser chunk) so the last partial chunk is decoded, not left behind.
    public var padMs: Int
    /// Then wait for the recogniser's final text: this long, plus 100 ms per second of dictation and 1 ms per ms
    /// queued at the release, within what's left of `maxWaitMs`. Only a recogniser still not done after that (hung)
    /// is given up on: the last partial is sent and the recogniser is kept out of use until its finish returns.
    public var finalizeTimeoutMs: Int
    /// Drain + finalize never wait longer than this in total (qrtz-talk's own stop wait is capped at 20 s, N124).
    public var maxWaitMs: Int
    /// The mic stream's backlog (audio captured, not yet handed to the recogniser) is capped here. Past it the mic
    /// stands down for the rest of the dictation (later audio dropped, `error{dictation.backlog}`), never silently.
    public var backlogCapMs: Int
    /// Part of the drain (inside its limit): wait this long at most for the first mic block delivered after the
    /// tail, so the block the tap was still filling at the release (~85 ms) is in before the barrier. The short tail
    /// relies on this, and the pad protects the last word's decoding.
    public var nextBlockWaitMs: Int
    /// The block is waited on until its due time (the last block's arrival + its length) plus this much jitter.
    public var nextBlockSlackMs = 40
    public init(tailMs: Int = 50, drainTimeoutMs: Int = 300, padMs: Int = 640, finalizeTimeoutMs: Int = 1500,
                maxWaitMs: Int = 20_000, backlogCapMs: Int = 20_000, nextBlockWaitMs: Int = 120) {
        self.tailMs = tailMs; self.drainTimeoutMs = drainTimeoutMs; self.padMs = padMs; self.finalizeTimeoutMs = finalizeTimeoutMs
        self.maxWaitMs = maxWaitMs; self.backlogCapMs = backlogCapMs; self.nextBlockWaitMs = nextBlockWaitMs
    }
    public func drainLimitMs(queuedMs: Int) -> Int { min(maxWaitMs, drainTimeoutMs + 2 * max(0, queuedMs)) }
    public func finalizeLimitMs(audioMs: Int, queuedMs: Int) -> Int {
        let want = finalizeTimeoutMs + max(0, audioMs) / 10 + max(0, queuedMs)
        return min(want, max(finalizeTimeoutMs, maxWaitMs - drainLimitMs(queuedMs: queuedMs)))
    }
    /// Release to `dictation.stopped` at most (a recogniser that keeps up: nothing queued).
    public func budgetMs(audioMs: Int = 0, queuedMs: Int = 0) -> Int {
        tailMs + drainLimitMs(queuedMs: queuedMs) + finalizeLimitMs(audioMs: audioMs, queuedMs: queuedMs)
    }
}

/// Fires once (whichever of a completion and a deadline comes first).
final class FirstOf: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    func fire(_ f: () -> Void) { let first = lock.withLock { () -> Bool in defer { done = true }; return !done }; if first { f() } }
}

/// The mic stream's backlog: samples the audio thread yielded that the engine hasn't handled yet. Past `cap` the
/// mic stands down until re-armed (the next dictation): later blocks are dropped and counted, and the first refusal
/// is reported, so a recogniser that can't keep up never grows memory without bound or loses audio silently.
final class MicBacklog: @unchecked Sendable {
    private let lock = NSLock()
    private var queued = 0, dropped = 0, down = false, yielded = 0
    private var lastAt: ContinuousClock.Instant?, lastMs = 0
    let cap: Int
    init(cap: Int) { self.cap = cap }
    /// `ok`: yield it. `first`: this is the block that stood the mic down.
    func offer(_ n: Int) -> (ok: Bool, first: Bool) {
        lock.withLock {
            if down { dropped += n; return (false, false) }
            if queued + n > cap { down = true; dropped += n; return (false, true) }
            queued += n; yielded += 1; lastAt = .now; lastMs = n / 16; return (true, false)
        }
    }
    func took(_ n: Int) { lock.withLock { queued -= n } }
    var queuedSamples: Int { lock.withLock { queued } }
    /// Blocks the tap handed to the stream so far.
    var yieldedBlocks: Int { lock.withLock { yielded } }
    /// When the tap's next block is due: the last one's arrival plus its length (nil: none yet).
    var nextDue: ContinuousClock.Instant? { lock.withLock { lastAt.map { $0 + .milliseconds(lastMs) } } }
    var droppedSamples: Int { lock.withLock { dropped } }
    var stoodDown: Bool { lock.withLock { down } }
    func rearm() { lock.withLock { down = false; dropped = 0 } }
}

/// One dictation at a time: mic → (pre-roll) → on-device ASR + optional PCM stream to the host.
public actor DictationEngine {
    let mic: MicSource?
    let transcribers: [StreamingTranscriber]
    let inserter: TextInserter?
    let hotkeys: HotkeyMonitor?
    let send: @Sendable (HelperMessage) -> Void
    let now: @Sendable () -> Double

    var warmMic = false
    var streamAudio = false
    var pushKey = ""
    var consumeFn = true
    var preRoll = PreRollBuffer(capacity: 16_000 * 300 / 1000)
    var preRollMs = 300
    /// The push key went down and the mic was opened for it before the host's `dictation.start` (`arm`): the ring
    /// holds everything since the press (up to `armMaxMs`), and the next start takes it as its pre-roll.
    var armed = false
    var micRunning = false
    /// Set by HelperController: the session arbiter owns the mic lifecycle (attachMic/detachMic per its
    /// mic.subscribe/unsubscribe:dictation effects) and configure/start/stop never touch the mic themselves.
    let managedMic: Bool
    var micIn: AsyncStream<[Float]>.Continuation?
    var consumer: Task<Void, Never>?

    struct Session { let id: String; let startMs: Double; var transcriber: StreamingTranscriber?; var seq = 0; let segmenter = UtteranceSegmenter(idPrefix: "d"); var samples = 0; var pendingLevel: [Float] = []; var lastLevelMs: Double = 0; var pendingPcm: [Float] = []
        /// Released: still capturing the tail. `closed`: past the drain, nothing more reaches the recogniser.
        var stopping = false; var closed = false
        /// The latest partial (poll), the fallback for a hung finalize: never read from a recogniser mid-finish.
        var lastPartial = ""
        /// Cancel / abort while the stop is finalizing: the stop answers them (one dictation.stopped).
        var cancelled = false; var abortReason: DictationStopReason?
        /// Echo tuning (ADR 0099): this dictation's stage timings, logged once at its stop (content-free).
        var timing = DictationTiming(); let began = ContinuousClock.now }
    public let tail: DictationTail
    /// Waiting for the mic stream to deliver everything queued before a stop (an empty block is the barrier).
    var drainWaiters: [CheckedContinuation<Void, Never>] = []
    var backlog: MicBacklog?
    /// Appends in flight per recogniser (actor reentrancy) and who waits for them to end (true: idle, false: gave up).
    var appending: [ObjectIdentifier: Int] = [:]
    var idleWaiters: [ObjectIdentifier: [(UUID, CheckedContinuation<Bool, Never>)]] = [:]
    /// N131: recognisers held `busy` because an append outlived `close()`'s limit; released when it returns.
    var heldByAppend: Set<ObjectIdentifier> = []
    /// Recognisers whose finish() outlived the finalize limit: not used again until it returns.
    var busy: Set<ObjectIdentifier> = []
    var session: Session?
    /// ADR 0099: the last stopped dictation's stage timings (the bench reads them; the log line carries them).
    public private(set) var lastTiming: DictationTiming?
    /// ADR 0099: set by `finalText` when the final decode outlived its limit (the last partial was sent).
    var finalTimedOut = false
    var tick: Task<Void, Never>?
    /// ADR 0046 §1: the dictation's mic audio also goes to the speech module when one is attached.
    let speech: SpeechSinks?
    /// N143: the mic is gated against what the speakers play (a meeting's system channel or a dictation tap).
    nonisolated public let echo: DictationEcho?

    public init(mic: MicSource?, transcribers: [StreamingTranscriber], inserter: TextInserter?, hotkeys: HotkeyMonitor?,
                send: @escaping @Sendable (HelperMessage) -> Void, now: @escaping @Sendable () -> Double, managedMic: Bool = false,
                speech: SpeechSinks? = nil, tail: DictationTail = DictationTail(), echo: DictationEcho? = nil) {
        self.managedMic = managedMic; self.speech = speech; self.tail = tail; self.echo = echo
        self.mic = mic; self.transcribers = transcribers; self.inserter = inserter; self.hotkeys = hotkeys; self.send = send; self.now = now
    }

    /// N153: `talk.echoGate`. Takes effect at the next dictation's start (a running one keeps its gate).
    public func setEchoGate(_ on: Bool) { echo?.enabled = on }

    public var isActive: Bool { session != nil }
    public var activeId: String? { session?.id }
    public var isMicRunning: Bool { micRunning }
    public var preRollSamples: Int { preRoll.count }

    public func configure(warmMic: Bool, preRollMs: Int, streamAudio: Bool, pushKey: String, consumeFn: Bool = true) {
        self.warmMic = warmMic; self.streamAudio = streamAudio
        self.preRollMs = preRollMs
        if !armed { preRoll.setCapacity(16_000 * preRollMs / 1000) }
        if pushKey != self.pushKey || consumeFn != self.consumeFn {
            hotkeys?.stop()
            self.pushKey = pushKey
            self.consumeFn = consumeFn
            if !pushKey.isEmpty, let hk = hotkeys {
                let send = self.send
                if !hk.start(key: pushKey, consumeFn: consumeFn, onChange: { [weak hk] a in send(.hotkey(a, ctrl: hk?.pressWithControl ?? false)) }) {
                    send(.error(code: "dictation.hotkey", message: "push-to-talk key \(pushKey) unavailable (Input Monitoring / Accessibility?)", fatal: false))
                }
            }
        }
        // ADR 0079: echo latency: load the recogniser as soon as a push key is set, not on the first press. Idempotent,
        // and only local files are read (a missing model fails fast and the next press tries again).
        if !self.pushKey.isEmpty { prewarm() }
        guard !managedMic else { return }
        if warmMic { startMic() } else if session == nil { stopMic() }
    }

    /// `prewarm()` when a push key is set (after the models were downloaded and warmed). `log`: say how long it took
    /// (N174: a split echo's prewarm after a download, so the first press can be checked against it).
    public func prewarmIfKeyed(log: Bool = false) { if !pushKey.isEmpty { preload(log: log) } }

    /// Loads the first recogniser that can load, in the background (idempotent: a loaded one returns at once).
    public func prewarm() { preload(log: false) }

    /// ADR 0087: echo's recogniser is resident for the life of the helper. Loaded when the helper starts (if the
    /// models are downloaded; only local files are read), never unloaded by the engine. A press while this runs joins
    /// the same in-flight load (the transcriber's `SharedLoad`), so there is never a second load. Logs how long it took.
    @discardableResult
    nonisolated public func preload(log: Bool = true) -> Task<Void, Never> {
        let ts = transcribers, send = self.send
        return Task.detached {
            for t in ts {
                let t0 = ContinuousClock.now
                do {
                    try await t.prepare()
                    let d = t0.duration(to: .now)
                    let ms = Int(d.components.seconds) * 1000 + Int(d.components.attoseconds / 1_000_000_000_000_000)
                    if log { send(.log(level: .info, msg: "echo asr: \(t.name) resident (\(ms) ms)")) }
                    return
                } catch {
                    if log { send(.log(level: .info, msg: "echo asr: \(t.name) not loaded at start: \(error)")) }
                }
            }
        }
    }

    /// N195 (ADR 0224): why a mic open failed, for `error{dictation.mic}.reason` (nil when the error doesn't say).
    /// Talk maps `no-device` to mic-lost and anything else to mic-denied.
    public static func micFailureReason(_ error: Error) -> String? {
        let d = "\(error)".lowercased()
        if d.contains("no microphone") || d.contains("no input device") || d.contains("device not found") { return "no-device" }
        if d.contains("denied") || d.contains("not authorized") || d.contains("not permitted") { return "denied" }
        return nil
    }

    /// Opens the mic (no-op while it runs). False when it could not open; the error is sent, naming `id` when a
    /// start asked (N195).
    @discardableResult
    func startMic(id: String? = nil) -> Bool {
        guard !micRunning else { return true }
        guard let mic else { return false }
        // Mic callbacks arrive on an audio thread; keep their order with one stream into the actor.
        // Unbounded, but MicBacklog caps what's queued and stands the mic down explicitly past it.
        let (blocks, input) = AsyncStream<[Float]>.makeStream(bufferingPolicy: .unbounded)
        let bl = MicBacklog(cap: 16 * tail.backlogCapMs)
        backlog = bl
        consumer = Task { [weak self] in for await b in blocks { await self?.onSamples(b); bl.took(b.count) } }
        micIn = input
        do {
            try mic.start(onSamples: { [weak self] s in
                let r = bl.offer(s.count)
                if r.ok { input.yield(s) } else if r.first { Task { await self?.backlogStoodDown() } }
            })
            micRunning = true
            return true
        } catch {
            input.finish(); consumer?.cancel(); consumer = nil; micIn = nil; backlog = nil
            send(.error(code: "dictation.mic", message: "\(error)", fatal: false, id: id, reason: Self.micFailureReason(error)))
            return false
        }
    }

    /// Arbiter effect mic.subscribe:dictation. False when the mic did not open: the controller then steps the
    /// arbiter back (`micFailed`) so the next press or start subscribes again (N195).
    @discardableResult
    public func attachMic() -> Bool { startMic() }

    /// Arbiter effect mic.unsubscribe:dictation.
    public func detachMic() { stopMic() }
    /// The shared tap was dropped under us (microphone revoked): forget the reader so the next attach really
    /// re-subscribes. Without this a warm mic stays `micRunning` with no reader and dictation hears nothing.
    public func micDropped() { stopMic() }

    func stopMic() {
        guard micRunning else { return }
        mic?.stop(); micRunning = false; _ = preRoll.drain()
        micIn?.finish(); micIn = nil; consumer = nil; backlog = nil
    }

    func backlogStoodDown() {
        let secs = tail.backlogCapMs / 1000
        send(.log(level: .warn, msg: "dictation: the recogniser fell \(secs) s behind; mic stood down for this dictation"))
        if session != nil {
            send(.error(code: "dictation.backlog", message: "speech recognition fell \(secs) s behind; what was heard so far is kept", fatal: false))
        } else {
            backlog?.rearm()  // not dictating: nothing to stand down for
        }
    }

    /// The mic can be opened for a press: not already warm, and no dictation listening (one still finishing is
    /// fine: talk queues the press until it is done, N110).
    public var canArm: Bool { !warmMic && mic != nil && (session == nil || session?.stopping == true) }

    /// Item 2 (docs/echo/dictation-tuning.md): the push key is down. Everything the mic hears from now on is kept
    /// (up to `maxMs`) for the dictation the host is about to start, so its first word isn't lost to the round trip
    /// (and the host's engine handshake) before `dictation.start`. The controller subscribes the reader.
    public func arm(maxMs: Int) {
        armed = true
        _ = preRoll.drain()  // nothing from before the press
        preRoll.setCapacity(16 * max(maxMs, preRollMs))
    }

    /// The press never became a dictation: back to the configured pre-roll.
    public func disarm() {
        guard armed else { return }
        armed = false
        _ = preRoll.drain()
        preRoll.setCapacity(16_000 * preRollMs / 1000)
    }

    public func start(id: String) async {
        if session != nil { send(.log(level: .warn, msg: "dictation.start while dictating; ignored")); return }
        guard mic != nil else { send(.error(code: "dictation.mic", message: "no microphone source", fatal: false)); return }
        var s = Session(id: id, startMs: now())
        backlog?.rearm()
        for t in transcribers {
            if busy.contains(ObjectIdentifier(t)) {
                send(.log(level: .warn, msg: "dictation asr \(t.name) still finishing the last dictation; skipped")); continue
            }
            do { try await t.prepare(); await t.reset(); s.transcriber = t; break }
            catch { send(.log(level: .warn, msg: "dictation asr \(t.name) unavailable: \(error)")) }
        }
        // N134: nothing would hear this dictation (every recogniser busy or failing, no speech module attached):
        // refuse it, so the host says so instead of pasting nothing after the release.
        if s.transcriber == nil, !(speech?.has(.dictation(id: id)) ?? false) {
            let why = transcribers.isEmpty ? "no speech recogniser" : "no speech recogniser is ready"
            send(.error(code: "dictation.asr", message: why, fatal: false, id: id))
            return
        }
        // N195 (ADR 0224): with a managed mic the arbiter's mic.subscribe opens it, at the press (the arm) or at
        // this start's admit. If that open failed (or the reader was dropped under a warm mic), try once more here;
        // if it still fails, refuse this start by its id (error{dictation.mic, id, reason}), like N134.
        if managedMic, !micRunning, !startMic(id: id) { return }
        let pre = preRoll.drain()
        if armed {
            armed = false; s.timing.armed = true
            preRoll.setCapacity(16_000 * preRollMs / 1000)
        }
        s.timing.asr = s.transcriber?.name ?? ""
        s.timing.preRollMs = pre.count / 16
        session = s
        send(.dictationStarted(id: id, preRollMs: Double(pre.count) * 1000 / 16_000))
        if let line = echo?.begin() { send(.log(level: .info, msg: line)) }
        if !managedMic { startMic() }
        if !pre.isEmpty { await feed(pre) }
        tick = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 100_000_000)
                await self?.poll()
            }
        }
    }

    public func onSamples(_ raw: [Float]) async {
        guard !raw.isEmpty else {
            // The drain barrier: every block queued before it has been fed.
            let w = drainWaiters; drainWaiters.removeAll()
            for c in w { c.resume() }
            return
        }
        let s = echo?.process(raw) ?? raw
        // Past the stop's drain, audio is the next dictation's pre-roll (a quick re-press), never the recogniser's.
        guard let sess = session, !sess.closed else { preRoll.append(s); return }
        await feed(s)
    }

    func feed(_ s: [Float]) async {
        guard var sess = session, !sess.closed else { return }
        sess.samples += s.count
        sess.pendingLevel += s
        var pk = max(sess.timing.peak, 0)
        for v in s { pk = max(pk, abs(v)) }
        sess.timing.peak = pk
        let t = now()
        if t - sess.lastLevelMs >= 50 {
            send(.dictationLevel(rms: min(1, DSP.rms(sess.pendingLevel) * 4)))
            sess.pendingLevel.removeAll(keepingCapacity: true); sess.lastLevelMs = t
        }
        if streamAudio {
            sess.pendingPcm += s
            if sess.pendingPcm.count >= 1600 {  // 100 ms frames
                send(.dictationAudio(id: sess.id, seq: sess.seq, pcm16: PCM16.base64(sess.pendingPcm)))
                sess.seq += 1; sess.pendingPcm.removeAll(keepingCapacity: true)
            }
        }
        session = sess
        speech?.write(.dictation(id: sess.id), .mic, s)
        guard let tr = sess.transcriber else { return }
        let k = ObjectIdentifier(tr)
        appending[k, default: 0] += 1
        let t0 = ContinuousClock.now
        await tr.append(s)
        let took = t0.msToNow()
        if var cur = session, cur.id == sess.id {
            cur.timing.appends += 1; cur.timing.appendTotalMs += took; cur.timing.appendMaxMs = max(cur.timing.appendMaxMs, took)
            session?.timing = cur.timing
        }
        appending[k, default: 1] -= 1
        if appending[k] == 0 {
            appending[k] = nil
            let w = idleWaiters.removeValue(forKey: k) ?? []
            for (_, c) in w { c.resume(returning: true) }
            if heldByAppend.remove(k) != nil { busy.remove(k) }
        }
    }

    /// No append starts after this. True: the ones in flight returned within `limitMs`, the recogniser is the
    /// caller's alone. False (N131): one is still running; the recogniser is held `busy` until it returns, and the
    /// caller must not touch it (no pad, no finish).
    func close(limitMs: Int) async -> Bool {
        session?.closed = true
        guard let tr = session?.transcriber else { return true }
        let k = ObjectIdentifier(tr)
        guard (appending[k] ?? 0) > 0 else { return true }
        let token = UUID()
        let idle = await withCheckedContinuation { (c: CheckedContinuation<Bool, Never>) in
            idleWaiters[k, default: []].append((token, c))
            Task { [weak self] in
                try? await Task.sleep(nanoseconds: UInt64(max(0, limitMs)) * 1_000_000)
                await self?.giveUpIdle(k, token)
            }
        }
        if !idle {
            busy.insert(k); heldByAppend.insert(k)
            send(.log(level: .warn, msg: "dictation: \(tr.name) append still running after \(limitMs) ms; using the last partial (recogniser held until it returns)"))
        }
        return idle
    }

    func giveUpIdle(_ k: ObjectIdentifier, _ token: UUID) {
        guard var w = idleWaiters[k], let i = w.firstIndex(where: { $0.0 == token }) else { return }
        let c = w.remove(at: i).1
        idleWaiters[k] = w.isEmpty ? nil : w
        c.resume(returning: false)
    }

    func poll() async {
        guard let s = session, let tr = s.transcriber else { return }
        guard !s.closed else { return }
        let text = await tr.transcript()
        if session?.id == s.id {
            session?.lastPartial = text
            if !text.isEmpty, (session?.timing.firstPartialMs ?? 0) == -1 { let ms = Int(s.began.msToNow()); session?.timing.firstPartialMs = ms }
        }
        for e in s.segmenter.update(cumulative: text, nowMs: now() - s.startMs) { send(.dictationAsr(id: s.id, event: e)) }
    }

    public func stop(id: String) async {
        guard let s0 = session, s0.id == id, !s0.stopping else { return }
        session?.stopping = true
        let released = ContinuousClock.now
        // The tap's block count at the release: the block it was filling then is the one the drain must wait for.
        // A block that lands during the tail already is it (no second wait for the block after it).
        let releaseBlocks = backlog?.yieldedBlocks
        // 1. The tail: keep capturing (onSamples still feeds this session while we wait).
        if tail.tailMs > 0 { try? await Task.sleep(nanoseconds: UInt64(tail.tailMs) * 1_000_000) }
        guard session?.id == id else { return }  // cancelled or aborted meanwhile (that path answered)
        session?.timing.tailMs = Int(released.msToNow())
        // 2. Drain: the mic blocks still queued in the stream reach the recogniser (bounded, scaled to the queue).
        let queuedMs = (backlog?.queuedSamples ?? 0) / 16
        // ADR 0118: a segmented recogniser's whole release is `SegmentedTail.budgetMs` (drain at its unqueued limit:
        // its appends only buffer, so nothing should be queued), the streaming `budgetMs` otherwise.
        let segmented = session?.transcriber is SegmentedTranscriber
        let drainLimit = tail.drainLimitMs(queuedMs: segmented ? 0 : queuedMs)
        let drainStart = ContinuousClock.now
        let drained = await drainMic(limitMs: drainLimit, sinceBlocks: releaseBlocks)
        guard session?.id == id else { return }
        session?.timing.drainMs = Int(released.msToNow()); session?.timing.drained = drained
        // ADR 0115: the resampler's held-back tail (end of stream) reaches the recogniser before the pad.
        if drained, micRunning, let mic {
            mic.flush()
            _ = await drainMic(limitMs: Self.flushDrainMs, waitForBlock: false)
            guard session?.id == id else { return }
        }
        session?.timing.queuedAtReleaseMs = queuedMs
        // N139: one deadline for the whole release (drain + finalize; `budgetMs` unchanged). close() may use all of
        // it except a reserve for pad + finish, so a drain that timed out still waits for a slow (not hung) append and
        // finalizes its audio; past that the last partial is the answer (N131).
        let finalizeLimit = finalizeLimitMs(session?.transcriber, audioMs: (session?.samples ?? 0) / 16, queuedMs: queuedMs)
        let deadline = drainStart + .milliseconds(drainLimit + finalizeLimit)
        let reserveMs = finalizeLimit / 2
        let idle = await close(limitMs: max(0, Self.msLeft(until: deadline) - reserveMs))
        if session?.id == id { session?.timing.closeMs = Int(released.msToNow()); session?.timing.appendHeld = !idle }
        if !drained { _ = preRoll.drain() }  // what was still queued is this dictation's stale audio, not pre-roll
        guard var s = session, s.id == id else { return }
        tick?.cancel(); tick = nil
        if streamAudio, !s.pendingPcm.isEmpty {
            send(.dictationAudio(id: s.id, seq: s.seq, pcm16: PCM16.base64(s.pendingPcm)))
            s.seq += 1; s.pendingPcm.removeAll()
        }
        var text = ""
        if s.transcriber != nil, !idle {
            text = s.lastPartial.trimmingCharacters(in: .whitespacesAndNewlines)
            for e in s.segmenter.flush(cumulative: text, nowMs: now() - s.startMs) { send(.dictationAsr(id: s.id, event: e)) }
        } else if let tr = s.transcriber {
            // 3. Pad and finalize: silence after the last word puts it in a full chunk; the final text is awaited.
            finalTimedOut = false
            text = await finalText(tr, limitMs: max(0, Self.msLeft(until: deadline)), partial: s.lastPartial).trimmingCharacters(in: .whitespacesAndNewlines)
            for e in s.segmenter.flush(cumulative: text, nowMs: now() - s.startMs) { send(.dictationAsr(id: s.id, event: e)) }
        }
        let end = session
        var timing = end?.timing ?? s.timing
        if s.transcriber != nil, idle { timing.finalizeMs = Int(released.msToNow()); timing.finalTimedOut = finalTimedOut }
        timing.audioMs = s.samples / 16
        if let seg = s.transcriber as? PaddedFinishing { timing.segments = await seg.segmentStats() }
        timing.words = text.split(whereSeparator: { $0.isWhitespace }).count
        timing.gateDucked = echo?.ducked ?? 0; timing.gateBlocks = echo?.blocks ?? 0; timing.gate = echo?.mode ?? ""
        session = nil
        echo?.end()
        speech?.end(.dictation(id: id))
        if end?.abortReason != nil { stopMic() } else if !warmMic && !managedMic { stopMic() }
        timing.releaseToStoppedMs = Int(released.msToNow())
        if end?.cancelled == true {
            send(.dictationStopped(id: id, text: "", durationMs: 0))
        } else {
            send(.dictationStopped(id: id, text: text, durationMs: Double(s.samples) * 1000 / 16_000, reason: end?.abortReason,
                                   asr: Self.asrInfo(s.transcriber, timing.segments)))
            // ADR 0099: one content-free timing line per dictation, after the answer (it never delays it).
            lastTiming = timing
            send(.log(level: .info, msg: timing.logLine))
        }
    }

    /// `dictation.stopped`'s recogniser fields: which recogniser heard it, and a segmented one's counts, plus
    /// (ADR 0132) the release tail's confidence (3 places, 0...1) and `tailDropped` only when the floor dropped it.
    static func asrInfo(_ tr: StreamingTranscriber?, _ g: SegmentStats?) -> DictationAsrInfo? {
        guard let tr else { return nil }
        let conf = g?.tailConf.flatMap { c -> Double? in
            let v = Double(c)
            return v.isFinite ? (min(1, max(0, v)) * 1000).rounded() / 1000 : nil
        }
        return DictationAsrInfo(asr: tr.name, segments: g?.segments,
                                tailDecodeMs: g.flatMap { $0.tailDecodeMs < 0 ? nil : ($0.tailDecodeMs * 10).rounded() / 10 },
                                tailConf: conf, tailDropped: g?.tailDropped == true ? true : nil)
    }

    /// The pad + finish limit of a release: ADR 0118's segmented budget for a segmented recogniser (its decode is
    /// the last segment, not the dictation), the streaming one otherwise.
    func finalizeLimitMs(_ tr: StreamingTranscriber?, audioMs: Int, queuedMs: Int) -> Int {
        if let seg = tr as? SegmentedTranscriber { return SegmentedTail(tail: tail, rule: seg.rule).finalizeLimitMs(audioMs: audioMs) }
        return tail.finalizeLimitMs(audioMs: audioMs, queuedMs: queuedMs)
    }

    /// How long a stop waits for the resampler's flushed tail to reach the recogniser (a few ms of audio, already
    /// queued behind the drained blocks).
    static let flushDrainMs = 50

    static func msLeft(until t: ContinuousClock.Instant) -> Int {
        let d = ContinuousClock.now.duration(to: t)
        return Int(d.components.seconds) * 1000 + Int(d.components.attoseconds / 1_000_000_000_000_000)
    }

    /// Waits (bounded) until the mic stream has handed over every block it had queued. False: it timed out.
    func drainMic(limitMs: Int, sinceBlocks: Int? = nil, waitForBlock: Bool = true) async -> Bool {
        guard let input = micIn, micRunning else { return true }
        let start = ContinuousClock.now
        if waitForBlock, tail.nextBlockWaitMs > 0, limitMs > 0 { await nextBlock(limitMs: min(tail.nextBlockWaitMs, limitMs), since: sinceBlocks) }
        let used = start.duration(to: .now)
        let limitMs = max(0, limitMs - Int(used.components.seconds) * 1000 - Int(used.components.attoseconds / 1_000_000_000_000_000))
        drainTimedOut = false
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            drainWaiters.append(c)
            input.yield([])
            Task { [weak self] in
                try? await Task.sleep(nanoseconds: UInt64(limitMs) * 1_000_000)
                await self?.releaseDrainWaiters(limitMs)
            }
        }
        return !drainTimedOut
    }
    var drainTimedOut = false

    /// Waits (bounded; a timeout is not a failure) until the tap has yielded a block after `since` (the count at the
    /// release; default now), so that block (the one it was filling at the release) is queued ahead of the barrier.
    func nextBlock(limitMs: Int, since: Int? = nil) async {
        guard let bl = backlog else { return }
        let seen = since ?? bl.yieldedBlocks
        // Not past the block's due time plus jitter: a tap that has stopped delivering (the device went quiet, a
        // file-fed bench) isn't waited on for the whole limit; a live tap's block lands well inside it.
        var deadline = ContinuousClock.now + .milliseconds(limitMs)
        if let due = bl.nextDue { deadline = min(deadline, due + .milliseconds(tail.nextBlockSlackMs)) }
        while bl.yieldedBlocks == seen, ContinuousClock.now < deadline { try? await Task.sleep(nanoseconds: 5_000_000) }
    }

    func releaseDrainWaiters(_ ms: Int) {
        let w = drainWaiters; drainWaiters.removeAll()
        if !w.isEmpty {
            drainTimedOut = true
            send(.log(level: .warn, msg: "dictation: mic queue not drained in \(ms) ms"))
        }
        for c in w { c.resume() }
    }

    /// Silence pad, then the recogniser's final text, both under one deadline of `limitMs` (N131: the pad's append
    /// counts too). Only past that (a hung recogniser) is `partial` used, and the recogniser stays out of use until
    /// its pad and finish() return.
    func finalText(_ tr: StreamingTranscriber, limitMs: Int, partial: String) async -> String {
        let pad = tail.padMs > 0 ? [Float](repeating: 0, count: 16 * tail.padMs) : []
        let key = ObjectIdentifier(tr)
        busy.insert(key)
        let first = FirstOf()
        let final: String? = await withCheckedContinuation { (c: CheckedContinuation<String?, Never>) in
            Task { [weak self] in
                let t: String
                if let seg = tr as? PaddedFinishing {
                    // A segmented recogniser takes the pad as end context for its last segment (never a pause).
                    t = await seg.finish(padSamples: pad.count)
                } else {
                    if !pad.isEmpty { await tr.append(pad) }
                    t = await tr.finish()
                }
                await self?.finished(key)
                first.fire { c.resume(returning: t) }
            }
            Task {
                try? await Task.sleep(nanoseconds: UInt64(limitMs) * 1_000_000)
                first.fire { c.resume(returning: nil) }
            }
        }
        if let final { return final }
        finalTimedOut = true
        send(.log(level: .warn, msg: "dictation: \(tr.name) final text not ready in \(limitMs) ms; using the last partial (recogniser held until it finishes)"))
        return partial
    }

    func finished(_ key: ObjectIdentifier) { if !heldByAppend.contains(key) { busy.remove(key) } }

    public func cancel(id: String) async {
        guard let s = session, s.id == id else { return }
        if s.closed { session?.cancelled = true; return }  // the stop is finalizing: it answers, once
        tick?.cancel(); tick = nil
        if await close(limitMs: tail.drainTimeoutMs) {
            // A segmented recogniser just drops its segments (no tail decode for a cancelled dictation).
            if let seg = s.transcriber as? PaddedFinishing { await seg.reset() } else { _ = await s.transcriber?.finish() }
        }
        session = nil
        echo?.end()
        speech?.end(.dictation(id: id))
        if !warmMic && !managedMic { stopMic() }
        send(.dictationStopped(id: id, text: "", durationMs: 0))
    }

    /// The helper ends the dictation itself (microphone revoked / device lost): flush what was heard, tell the host
    /// why, and release the mic even when the warm mic is on (it can't be read any more).
    public func abort(reason: DictationStopReason) async {
        guard let s = session else { stopMic(); return }
        if s.closed { session?.abortReason = reason; return }  // the stop is finalizing: it answers with the reason
        tick?.cancel(); tick = nil
        let idle = await close(limitMs: tail.drainTimeoutMs)
        var text = ""
        if let tr = s.transcriber {
            // N164: the same pad + finish as a release (bounded like it), so an abort keeps the last word too.
            let limit = finalizeLimitMs(tr, audioMs: s.samples / 16, queuedMs: 0)
            text = (idle ? await finalText(tr, limitMs: limit, partial: s.lastPartial) : s.lastPartial).trimmingCharacters(in: .whitespacesAndNewlines)
            for e in s.segmenter.flush(cumulative: text, nowMs: now() - s.startMs) { send(.dictationAsr(id: s.id, event: e)) }
        }
        session = nil
        echo?.end()
        speech?.end(.dictation(id: s.id))
        stopMic()
        let g = await (s.transcriber as? PaddedFinishing)?.segmentStats()
        send(.dictationStopped(id: s.id, text: text, durationMs: Double(s.samples) * 1000 / 16_000, reason: reason,
                               asr: Self.asrInfo(s.transcriber, g)))
    }

    public func insert(id: String, text: String, restoreClipboard: Bool) async {
        guard let inserter else {
            send(.insertResult(id: id, method: .none, ok: false, app: nil, error: "text insertion unsupported on this OS")); return
        }
        let r = await inserter.insert(text, restoreClipboard: restoreClipboard)
        send(.insertResult(id: id, method: r.method, ok: r.ok, app: r.app, error: r.error))
    }

    public func shutdown() { hotkeys?.stop(); stopMic() }
}
