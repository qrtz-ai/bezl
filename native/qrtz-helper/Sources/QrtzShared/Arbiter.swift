import Foundation

// Concurrency + permission-revocation rules for one helper (H6/H5). Mirrors packages/core/src/helper/arbiter.ts;
// both are checked against spec/vectors/helper/arbiter.json. Spec: docs/helper-protocol.md.
//
// Decision: MUX. recording.* and dictation.* run side by side and share one mic tap (MicHub). Each kind is
// single-instance: a duplicate start is ignored (logged), never queued.
//
// Invariant (checked after every step): a running recording always has a file, `recording` iff
// `activeAudioPath != nil`. A recording.start without a path is refused; when the file cannot be created
// (`recordingFileFailed`) the recording is torn down at once, so busy and path-scoped stops can always name it.

public struct ArbiterState: Equatable, Sendable {
    public var recording = false
    /// The file the running recording writes; nil iff not recording (the invariant).
    public var activeAudioPath: String?
    public var dictating = false
    /// Warm mic (dictation.configure warmMic): the dictation reader stays subscribed between dictations.
    /// Survives a mic revoke; the reader comes back on re-grant.
    public var warm = false
    /// Who reads the shared mic tap ("recording", "dictation"), in subscription order.
    public var mic: [String] = []
    /// Revoked/denied permissions, in the order they were lost.
    public var denied: [OsPermission] = []
    public init(denied: [OsPermission] = []) { self.denied = denied }
}

public enum ArbiterEvent: Equatable, Sendable {
    case cmd(String, audioPath: String? = nil)  // recording.start{audioPath}|stop, dictation.start|stop|cancel
    case configure(warmMic: Bool)
    case revoked(OsPermission)
    case granted(OsPermission)
    case helperLost
    /// The file of the admitted recording could not be created: fail closed (tear the recording down).
    case recordingFileFailed(audioPath: String)
}

public struct ArbiterResult: Equatable, Sendable {
    public var state: ArbiterState
    public var decision: String  // accept | ignore | refuse
    public var reason: String?
    public var mux: Bool
    public var effects: [String]
}

public enum SessionArbiter {
    /// Recording degradations (lifecycle Degradation ids) caused by losing `p`.
    public static func degradations(revoking p: OsPermission, alreadyDenied: [OsPermission]) -> [String] {
        func has(_ x: OsPermission) -> Bool { x != p && !alreadyDenied.contains(x) }
        switch p {
        case .accessibility: return ["ax-unavailable"]
        case .microphone: return [has(.screenRecording) ? "no-mic" : "audio-unavailable"]
        case .screenRecording: return [has(.microphone) ? "mic-only" : "audio-unavailable"]
        }
    }

    /// Why `s` breaks the invariant, or nil. A running recording must have a file (and only then).
    public static func invariantViolation(_ s: ArbiterState) -> String? {
        if s.recording && (s.activeAudioPath ?? "").isEmpty { return "recording without an activeAudioPath" }
        if !s.recording && s.activeAudioPath != nil { return "activeAudioPath set while not recording" }
        return nil
    }

    public static func step(_ s: ArbiterState, _ e: ArbiterEvent) -> ArbiterResult {
        let out = transition(s, e)
        // A bug, never a normal path: crash rather than run a recording that no busy or stop can name. The host
        // supervisor restarts the helper (and the host's own watchdog restarts it if a pathless busy ever arrives).
        if let v = invariantViolation(out.state) { preconditionFailure("helper arbiter invariant broken: \(v)") }
        return out
    }

    static func transition(_ s: ArbiterState, _ e: ArbiterEvent) -> ArbiterResult {
        func r(_ st: ArbiterState, _ d: String, _ fx: [String] = [], _ reason: String? = nil) -> ArbiterResult {
            // N5: mux = two live readers; a warm idle mic keeps the dictation reader without dictating.
            ArbiterResult(state: st, decision: d, reason: reason, mux: st.mic.contains("recording") && st.dictating, effects: fx)
        }
        let micOk = !s.denied.contains(.microphone)
        let hasDict = s.mic.contains("dictation")
        var n = s
        switch e {
        case .helperLost:
            var fx: [String] = []
            if s.dictating { fx.append("dictation.stop:helper-lost") }
            if s.recording { fx.append("recording.lost") }
            fx += s.mic.map { "mic.unsubscribe:\($0)" }
            return r(ArbiterState(denied: s.denied), "accept", fx)
        case .recordingFileFailed(let path):
            if !s.recording { return r(s, "ignore", [], "not-recording") }
            if s.activeAudioPath != path { return r(s, "ignore", [], "other-recording") }
            let unsub = s.mic.contains("recording")
            n.recording = false; n.activeAudioPath = nil
            n.mic.removeAll { $0 == "recording" }
            return r(n, "accept", ["recording.lost"] + (unsub ? ["mic.unsubscribe:recording"] : []))
        case .configure(let warm):
            n.warm = warm
            if warm && micOk && !hasDict { n.mic.append("dictation"); return r(n, "accept", ["mic.subscribe:dictation"]) }
            if !warm && hasDict && !s.dictating { n.mic.removeAll { $0 == "dictation" }; return r(n, "accept", ["mic.unsubscribe:dictation"]) }
            return r(n, "accept")
        case .granted(let p):
            n.denied.removeAll { $0 == p }
            if p == .microphone && !micOk && s.warm && !hasDict { n.mic.append("dictation"); return r(n, "accept", ["mic.subscribe:dictation"]) }
            return r(n, "accept")
        case .revoked(let p):
            if s.denied.contains(p) { return r(s, "ignore", [], "unchanged") }
            var fx: [String] = []
            n.denied.append(p)
            if p == .microphone {
                if s.dictating { fx.append("dictation.stop:permission-revoked"); n.dictating = false }
                for who in s.mic { fx.append("mic.unsubscribe:\(who)") }
                n.mic = []
            }
            if s.recording { fx += degradations(revoking: p, alreadyDenied: s.denied).map { "degrade:\($0)" } }
            return r(n, "accept", fx)
        case .cmd(let c, let path):
            switch c {
            case "recording.start":
                if s.recording { return r(s, "ignore", [], "already-recording") }
                guard let path, !path.isEmpty else { return r(s, "refuse", [], "no-audio-path") }
                n.recording = true; n.activeAudioPath = path
                let sub = micOk && !s.mic.contains("recording")
                if sub { n.mic.append("recording") }
                return r(n, "accept", sub ? ["mic.subscribe:recording"] : [])
            case "recording.stop":
                if !s.recording { return r(s, "ignore", [], "not-recording") }
                n.recording = false; n.activeAudioPath = nil
                let unsub = s.mic.contains("recording")
                n.mic.removeAll { $0 == "recording" }
                return r(n, "accept", unsub ? ["mic.unsubscribe:recording"] : [])
            case "recording.abort":
                // N2: scoped by file; another recording (or one that already stopped) is untouched.
                if !s.recording { return r(s, "ignore", [], "not-recording") }
                guard let path, path == s.activeAudioPath else { return r(s, "ignore", [], "other-recording") }
                n.recording = false; n.activeAudioPath = nil
                let unsub = s.mic.contains("recording")
                n.mic.removeAll { $0 == "recording" }
                return r(n, "accept", unsub ? ["mic.unsubscribe:recording"] : [])
            case "dictation.start":
                if s.dictating { return r(s, "ignore", [], "already-dictating") }
                if !micOk { return r(s, "refuse", [], "microphone-denied") }
                n.dictating = true
                if hasDict { return r(n, "accept") }  // reuse the warm reader
                n.mic.append("dictation")
                return r(n, "accept", ["mic.subscribe:dictation"])
            case "dictation.stop", "dictation.cancel":
                if !s.dictating { return r(s, "ignore", [], "not-dictating") }
                n.dictating = false
                if s.warm && hasDict { return r(n, "accept") }  // warm mic keeps the reader
                n.mic.removeAll { $0 == "dictation" }
                return r(n, "accept", hasDict ? ["mic.unsubscribe:dictation"] : [])
            default:
                return r(s, "ignore", [], "unknown-command")
            }
        }
    }
}

/// One microphone, many readers: the shared tap behind meeting capture and dictation (H6 mux).
/// The device starts on the first subscriber and stops on the last; every subscriber gets every block.
public final class MicHub: @unchecked Sendable {
    let source: MicSource
    /// NSCondition so readers that join an in-flight source.start can wait for its outcome.
    private let lock = NSCondition()
    private var subs: [(id: String, cb: @Sendable ([Float]) -> Void)] = []
    private var running = false
    /// source.start is in flight (called outside the lock). Subscribes during it join and wait for its outcome:
    /// they return once the device runs, or rethrow the start's error (and are detached) if it never opened.
    /// Unsubscribes during it leave the device to the post-start check, which stops it again if nobody is
    /// subscribed any more.
    private var starting = false
    /// Generation of the latest source.start, and the error it failed with (so joiners of that start rethrow it).
    private var startGen = 0
    private var startFailure: (gen: Int, error: Error)?
    /// Gated readers may only attach while the arbiter has subscribed them (permit). A gated view whose start is
    /// not permitted is skipped, so a recording started with the mic denied never opens the device.
    private var gated: Set<String> = []
    private var permits: Set<String> = []
    private var lostListeners: [@Sendable (String) -> Void] = []
    public init(source: MicSource) {
        self.source = source
        // N138: the readers stay subscribed across a device switch (the source re-opens the new input and keeps
        // calling fanOut); a device that doesn't come back is reported to the listeners (the controller).
        source.onDeviceLost { [weak self] reason in
            guard let self else { return }
            self.lock.lock(); let ls = self.lostListeners; self.lock.unlock()
            for l in ls { l(reason) }
        }
    }

    public func addLostListener(_ cb: @escaping @Sendable (String) -> Void) { lock.lock(); lostListeners.append(cb); lock.unlock() }

    public var subscribers: [String] { lock.lock(); defer { lock.unlock() }; return subs.map(\.id) }
    public var isRunning: Bool { lock.lock(); defer { lock.unlock() }; return running }

    public func permit(_ id: String) { lock.lock(); permits.insert(id); lock.unlock() }
    public func revokePermit(_ id: String) { lock.lock(); permits.remove(id); lock.unlock() }
    func mayAttach(_ id: String) -> Bool { lock.lock(); defer { lock.unlock() }; return !gated.contains(id) || permits.contains(id) }

    public func subscribe(_ id: String, _ cb: @escaping @Sendable ([Float]) -> Void) throws {
        lock.lock()
        subs.removeAll { $0.id == id }
        subs.append((id, cb))
        if running { lock.unlock(); return }
        if starting {
            // Join the start in flight; never report success for a device that has not opened.
            let gen = startGen
            while starting && startGen == gen { lock.wait() }
            let failure = startFailure.flatMap { $0.gen == gen ? $0.error : nil }
            lock.unlock()
            if let failure { throw failure }
            return
        }
        starting = true
        startGen += 1
        let gen = startGen
        startFailure = nil
        lock.unlock()
        do { try source.start(onSamples: { [weak self] s in self?.fanOut(s) }) } catch {
            lock.lock()
            starting = false
            // Every reader here subscribed for this start (subs is empty whenever the device is off and idle), so
            // none of them may stay attached to a device that never opened; joiners rethrow the same error.
            subs.removeAll()
            startFailure = (gen, error)
            lock.broadcast()
            lock.unlock()
            throw error
        }
        lock.lock()
        starting = false
        let orphan = subs.isEmpty  // everyone left (or the tap was dropped) while the device was starting
        running = !orphan
        lock.broadcast()
        lock.unlock()
        if orphan { source.stop() }
    }

    public func unsubscribe(_ id: String) {
        lock.lock()
        subs.removeAll { $0.id == id }
        let needStop = running && subs.isEmpty
        if needStop { running = false }
        lock.unlock()
        if needStop { source.stop() }
    }

    /// ADR 0115: the device's resampler hands over its held-back tail (to every reader, in order); capture goes on.
    public func flush() {
        lock.lock(); let on = running; lock.unlock()
        if on { source.flush() }
    }

    /// Microphone revoked: every reader loses the tap and the device stops.
    public func dropAll() {
        lock.lock(); subs.removeAll(); permits.removeAll(); let wasRunning = running; running = false; lock.unlock()
        if wasRunning { source.stop() }
    }

    func fanOut(_ s: [Float]) {
        lock.lock(); let cbs = subs.map(\.cb); lock.unlock()
        for cb in cbs { cb(s) }
    }

    /// A MicSource view for one reader, so existing consumers (DictationEngine, capture sources) stay unchanged.
    /// `gated`: the view only attaches while permitted (the controller permits "recording" on mic.subscribe:recording).
    public func view(_ id: String, gated: Bool = false) -> MicSource {
        if gated { lock.lock(); self.gated.insert(id); lock.unlock() }
        return HubMic(hub: self, id: id)
    }
}

final class HubMic: MicSource, @unchecked Sendable {
    let hub: MicHub; let id: String
    init(hub: MicHub, id: String) { self.hub = hub; self.id = id }
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws {
        guard hub.mayAttach(id) else { return }  // arbiter did not subscribe this reader (mic denied): skip
        try hub.subscribe(id, onSamples)
    }
    func stop() { hub.unsubscribe(id) }
    func flush() { hub.flush() }
}
