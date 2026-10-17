import Foundation

// Concurrency + permission-revocation rules for one helper (H6/H5). Mirrors packages/core/src/helper/arbiter.ts;
// both are checked against spec/vectors/helper/arbiter.json. Spec: docs/helper-protocol.md.
//
// Decision: MUX. recording.* and dictation.* run side by side and share one mic tap (MicHub). Each kind is
// single-instance: a duplicate start is ignored (logged), never queued.

public struct ArbiterState: Equatable, Sendable {
    public var recording = false
    public var dictating = false
    /// Who reads the shared mic tap ("recording", "dictation"), in subscription order.
    public var mic: [String] = []
    /// Revoked/denied permissions, in the order they were lost.
    public var denied: [OsPermission] = []
    public init(denied: [OsPermission] = []) { self.denied = denied }
}

public enum ArbiterEvent: Equatable, Sendable {
    case cmd(String)  // recording.start|stop, dictation.start|stop|cancel
    case revoked(OsPermission)
    case granted(OsPermission)
    case helperLost
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

    public static func step(_ s: ArbiterState, _ e: ArbiterEvent) -> ArbiterResult {
        func r(_ st: ArbiterState, _ d: String, _ fx: [String] = [], _ reason: String? = nil) -> ArbiterResult {
            ArbiterResult(state: st, decision: d, reason: reason, mux: st.mic.contains("recording") && st.mic.contains("dictation"), effects: fx)
        }
        let micOk = !s.denied.contains(.microphone)
        var n = s
        switch e {
        case .helperLost:
            return r(ArbiterState(denied: s.denied), "accept", s.dictating ? ["dictation.stop:helper-lost"] : [])
        case .granted(let p):
            n.denied.removeAll { $0 == p }
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
        case .cmd(let c):
            switch c {
            case "recording.start":
                if s.recording { return r(s, "ignore", [], "already-recording") }
                n.recording = true
                let sub = micOk && !s.mic.contains("recording")
                if sub { n.mic.append("recording") }
                return r(n, "accept", sub ? ["mic.subscribe:recording"] : [])
            case "recording.stop":
                if !s.recording { return r(s, "ignore", [], "not-recording") }
                n.recording = false
                let unsub = s.mic.contains("recording")
                n.mic.removeAll { $0 == "recording" }
                return r(n, "accept", unsub ? ["mic.unsubscribe:recording"] : [])
            case "dictation.start":
                if s.dictating { return r(s, "ignore", [], "already-dictating") }
                if !micOk { return r(s, "refuse", [], "microphone-denied") }
                n.dictating = true
                n.mic.removeAll { $0 == "dictation" }; n.mic.append("dictation")
                return r(n, "accept", ["mic.subscribe:dictation"])
            case "dictation.stop", "dictation.cancel":
                if !s.dictating { return r(s, "ignore", [], "not-dictating") }
                n.dictating = false
                n.mic.removeAll { $0 == "dictation" }
                return r(n, "accept", ["mic.unsubscribe:dictation"])
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
    private let lock = NSLock()
    private var subs: [(id: String, cb: @Sendable ([Float]) -> Void)] = []
    private var running = false
    public init(source: MicSource) { self.source = source }

    public var subscribers: [String] { lock.lock(); defer { lock.unlock() }; return subs.map(\.id) }
    public var isRunning: Bool { lock.lock(); defer { lock.unlock() }; return running }

    public func subscribe(_ id: String, _ cb: @escaping @Sendable ([Float]) -> Void) throws {
        lock.lock()
        subs.removeAll { $0.id == id }
        subs.append((id, cb))
        let needStart = !running
        if needStart { running = true }
        lock.unlock()
        guard needStart else { return }
        do { try source.start(onSamples: { [weak self] s in self?.fanOut(s) }) } catch {
            lock.lock(); running = false; subs.removeAll { $0.id == id }; lock.unlock()
            throw error
        }
    }

    public func unsubscribe(_ id: String) {
        lock.lock()
        subs.removeAll { $0.id == id }
        let needStop = running && subs.isEmpty
        if needStop { running = false }
        lock.unlock()
        if needStop { source.stop() }
    }

    /// Microphone revoked: every reader loses the tap and the device stops.
    public func dropAll() {
        lock.lock(); subs.removeAll(); let wasRunning = running; running = false; lock.unlock()
        if wasRunning { source.stop() }
    }

    func fanOut(_ s: [Float]) {
        lock.lock(); let cbs = subs.map(\.cb); lock.unlock()
        for cb in cbs { cb(s) }
    }

    /// A MicSource view for one reader, so existing consumers (DictationEngine, capture sources) stay unchanged.
    public func view(_ id: String) -> MicSource { HubMic(hub: self, id: id) }
}

final class HubMic: MicSource, @unchecked Sendable {
    let hub: MicHub; let id: String
    init(hub: MicHub, id: String) { self.hub = hub; self.id = id }
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws { try hub.subscribe(id, onSamples) }
    func stop() { hub.unsubscribe(id) }
}
