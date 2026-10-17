import Foundation

// qrtz talk: push-to-talk dictation. Foundation-only so the logic (pre-roll, insertion fallback order,
// clipboard restore, PCM framing) is tested on Linux; the macOS executable supplies the real seams.

/// Microphone only, 16 kHz mono Float32. Kept running between dictations when the warm mic is on.
public protocol MicSource: AnyObject, Sendable {
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws
    func stop()
}

/// Native key monitor for push-to-talk (Electron global shortcuts only report key-down).
public protocol HotkeyMonitor: AnyObject, Sendable {
    /// key: "fn", "right-option", "right-command", "right-control". Returns false if unsupported or not permitted.
    func start(key: String, onChange: @escaping @Sendable (HotkeyAction) -> Void) -> Bool
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
public final class TextInserter: @unchecked Sendable {
    let backend: InsertionBackend
    let restoreDelayNs: UInt64
    public init(backend: InsertionBackend, restoreDelayMs: Int = 250) {
        self.backend = backend; self.restoreDelayNs = UInt64(restoreDelayMs) * 1_000_000
    }

    public func insert(_ text: String, restoreClipboard: Bool) async -> (method: InsertMethod, ok: Bool, app: String?, error: String?) {
        let app = backend.focusedAppName()
        guard !text.isEmpty else { return (.none, true, app, nil) }
        if backend.axSetSelectedText(text) { return (.axSelectedText, true, app, nil) }
        if backend.axInsertIntoValue(text) { return (.axValue, true, app, nil) }
        let snap = restoreClipboard ? backend.pasteboardSnapshot() : nil
        backend.pasteboardSetString(text)
        let ours = backend.pasteboardChangeCount()
        guard backend.postPaste() else {
            if restoreClipboard { backend.pasteboardRestore(snap) }
            return (.none, false, app, "cannot post Cmd-V (Accessibility permission?)")
        }
        if restoreClipboard {
            // Give the target app time to read the pasteboard, then put the user's clipboard back unless
            // something else wrote to it meanwhile.
            try? await Task.sleep(nanoseconds: restoreDelayNs)
            if backend.pasteboardChangeCount() == ours { backend.pasteboardRestore(snap) }
        }
        return (.paste, true, app, nil)
    }
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
    var preRoll = PreRollBuffer(capacity: 16_000 * 300 / 1000)
    var micRunning = false
    /// Set by HelperController: the session arbiter owns the mic lifecycle (attachMic/detachMic per its
    /// mic.subscribe/unsubscribe:dictation effects) and configure/start/stop never touch the mic themselves.
    let managedMic: Bool
    var micIn: AsyncStream<[Float]>.Continuation?
    var consumer: Task<Void, Never>?

    struct Session { let id: String; let startMs: Double; var transcriber: StreamingTranscriber?; var seq = 0; let segmenter = UtteranceSegmenter(idPrefix: "d"); var samples = 0; var pendingLevel: [Float] = []; var lastLevelMs: Double = 0; var pendingPcm: [Float] = [] }
    var session: Session?
    var tick: Task<Void, Never>?

    public init(mic: MicSource?, transcribers: [StreamingTranscriber], inserter: TextInserter?, hotkeys: HotkeyMonitor?,
                send: @escaping @Sendable (HelperMessage) -> Void, now: @escaping @Sendable () -> Double, managedMic: Bool = false) {
        self.managedMic = managedMic
        self.mic = mic; self.transcribers = transcribers; self.inserter = inserter; self.hotkeys = hotkeys; self.send = send; self.now = now
    }

    public var isActive: Bool { session != nil }
    public var activeId: String? { session?.id }
    public var isMicRunning: Bool { micRunning }
    public var preRollSamples: Int { preRoll.count }

    public func configure(warmMic: Bool, preRollMs: Int, streamAudio: Bool, pushKey: String) {
        self.warmMic = warmMic; self.streamAudio = streamAudio
        preRoll.setCapacity(16_000 * preRollMs / 1000)
        if pushKey != self.pushKey {
            hotkeys?.stop()
            self.pushKey = pushKey
            if !pushKey.isEmpty, let hk = hotkeys {
                let send = self.send
                if !hk.start(key: pushKey, onChange: { send(.hotkey($0)) }) {
                    send(.error(code: "dictation.hotkey", message: "push-to-talk key \(pushKey) unavailable (Input Monitoring / Accessibility?)", fatal: false))
                }
            }
        }
        guard !managedMic else { return }
        if warmMic { startMic() } else if session == nil { stopMic() }
    }

    func startMic() {
        guard !micRunning, let mic else { return }
        // Mic callbacks arrive on an audio thread; keep their order with one stream into the actor.
        let (blocks, input) = AsyncStream<[Float]>.makeStream(bufferingPolicy: .bufferingNewest(400))
        consumer = Task { [weak self] in for await b in blocks { await self?.onSamples(b) } }
        micIn = input
        do {
            try mic.start(onSamples: { input.yield($0) })
            micRunning = true
        } catch {
            input.finish(); consumer?.cancel(); consumer = nil; micIn = nil
            send(.error(code: "dictation.mic", message: "\(error)", fatal: false))
        }
    }

    /// Arbiter effect mic.subscribe:dictation.
    public func attachMic() { startMic() }
    /// Arbiter effect mic.unsubscribe:dictation.
    public func detachMic() { stopMic() }
    /// The shared tap was dropped under us (microphone revoked): forget the reader so the next attach really
    /// re-subscribes. Without this a warm mic stays `micRunning` with no reader and dictation hears nothing.
    public func micDropped() { stopMic() }

    func stopMic() {
        guard micRunning else { return }
        mic?.stop(); micRunning = false; _ = preRoll.drain()
        micIn?.finish(); micIn = nil; consumer = nil
    }

    public func start(id: String) async {
        if session != nil { send(.log(level: .warn, msg: "dictation.start while dictating; ignored")); return }
        guard mic != nil else { send(.error(code: "dictation.mic", message: "no microphone source", fatal: false)); return }
        var s = Session(id: id, startMs: now())
        for t in transcribers {
            do { try await t.prepare(); await t.reset(); s.transcriber = t; break }
            catch { send(.log(level: .warn, msg: "dictation asr \(t.name) unavailable: \(error)")) }
        }
        let pre = preRoll.drain()
        session = s
        send(.dictationStarted(id: id, preRollMs: Double(pre.count) * 1000 / 16_000))
        if !managedMic { startMic() }
        if !pre.isEmpty { await feed(pre) }
        tick = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 100_000_000)
                await self?.poll()
            }
        }
    }

    public func onSamples(_ s: [Float]) async {
        guard !s.isEmpty else { return }
        if session == nil { preRoll.append(s); return }
        await feed(s)
    }

    func feed(_ s: [Float]) async {
        guard var sess = session else { return }
        sess.samples += s.count
        sess.pendingLevel += s
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
        await sess.transcriber?.append(s)
    }

    func poll() async {
        guard let s = session, let tr = s.transcriber else { return }
        let text = await tr.transcript()
        for e in s.segmenter.update(cumulative: text, nowMs: now() - s.startMs) { send(.dictationAsr(id: s.id, event: e)) }
    }

    public func stop(id: String) async {
        guard var s = session, s.id == id else { return }
        tick?.cancel(); tick = nil
        if streamAudio, !s.pendingPcm.isEmpty {
            send(.dictationAudio(id: s.id, seq: s.seq, pcm16: PCM16.base64(s.pendingPcm)))
            s.seq += 1; s.pendingPcm.removeAll()
        }
        var text = ""
        if let tr = s.transcriber {
            text = await tr.finish().trimmingCharacters(in: .whitespacesAndNewlines)
            for e in s.segmenter.flush(cumulative: text, nowMs: now() - s.startMs) { send(.dictationAsr(id: s.id, event: e)) }
        }
        session = nil
        if !warmMic && !managedMic { stopMic() }
        send(.dictationStopped(id: id, text: text, durationMs: Double(s.samples) * 1000 / 16_000))
    }

    public func cancel(id: String) async {
        guard let s = session, s.id == id else { return }
        tick?.cancel(); tick = nil
        _ = await s.transcriber?.finish()
        session = nil
        if !warmMic && !managedMic { stopMic() }
        send(.dictationStopped(id: id, text: "", durationMs: 0))
    }

    /// The helper ends the dictation itself (microphone revoked / device lost): flush what was heard, tell the host
    /// why, and release the mic even when the warm mic is on (it can't be read any more).
    public func abort(reason: DictationStopReason) async {
        guard let s = session else { stopMic(); return }
        tick?.cancel(); tick = nil
        var text = ""
        if let tr = s.transcriber {
            text = await tr.finish().trimmingCharacters(in: .whitespacesAndNewlines)
            for e in s.segmenter.flush(cumulative: text, nowMs: now() - s.startMs) { send(.dictationAsr(id: s.id, event: e)) }
        }
        session = nil
        stopMic()
        send(.dictationStopped(id: s.id, text: text, durationMs: Double(s.samples) * 1000 / 16_000, reason: reason))
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
