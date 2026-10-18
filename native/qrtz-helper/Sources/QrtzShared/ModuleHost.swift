import Foundation

// ADR 0146 M2.5: the host side of a helper app, shared by the module apps (qrtz-echo, qrtz-nano) and the monolith
// (qrtz-helper, which runs both parts in one). It answers what every app answers (hello, ping, permissions, the
// speech attach, the heartbeat, shutdown), owns the app's H6/H5 session arbiter and TCC polling, and hands each
// other command to the module part that owns it. Each helper has its own arbiter: in split, echo's admits
// dictations and nano's admits recordings, both against qrtz's TCC grants (the host spawns them, so qrtz stays their
// responsible process).

/// One module's part of a helper app (echo's dictation, nano's recordings).
public protocol ModulePart: AnyObject, Sendable {
    /// The host commands this part carries out.
    func owns(_ m: HostMessage) -> Bool
    /// After hello (preloads, meeting detection).
    func start() async
    func handle(_ m: HostMessage) async
    /// An arbiter effect for this part (`mic.subscribe:dictation`, `dictation.stop:permission-revoked`, …).
    func apply(effect: String) async
    /// The microphone grant was revoked and the shared tap dropped.
    func micRevoked() async
    /// N138: the mic's device went away and didn't come back.
    func micDeviceLost(_ reason: String) async
    /// The app is going away: no new sessions (a recording is finished here so its WAV is complete).
    func beginShutdown() async
    func shutdown() async
    /// The running dictation's id (for `dictation.busy`).
    func activeDictationId() async -> String?
    /// Every line this app writes, just before / just after it is written (the pill, the push key's arm).
    func willSend(_ m: HelperMessage)
    func didSend(_ m: HelperMessage)
    /// Every host line, as it arrives (the pill).
    func observe(_ m: HostMessage)
    /// The recording-relative clock, when this part keeps one (nano).
    func clockMs() -> Double?
}

public extension ModulePart {
    func start() async {}
    func apply(effect: String) async {}
    func micRevoked() async {}
    func micDeviceLost(_ reason: String) async {}
    func beginShutdown() async {}
    func shutdown() async {}
    func activeDictationId() async -> String? { nil }
    func willSend(_ m: HelperMessage) {}
    func didSend(_ m: HelperMessage) {}
    func observe(_ m: HostMessage) {}
    func clockMs() -> Double? { nil }
}

/// What the host part of an app needs.
public struct HostDeps: Sendable {
    public var permissions: PermissionsProvider
    /// H6: the app's shared mic tap (nil: no managed mic).
    public var micHub: MicHub?
    /// H5: how often TCC state is re-read while capturing or dictating (0 = never poll; tests call checkPermissions()).
    public var permissionPollMs: Int
    /// ADR 0046 §1 / N55: where the speech module's audio fds arrive (nil = the host passed no speech socket).
    public var speechFds: FdReceiver?
    public var helperVersion: String
    public var os: String
    public var features: [String]
    /// Names the app in its own logs ("echo", "nano", "helper").
    public var module: String
    public init(permissions: PermissionsProvider, micHub: MicHub? = nil, permissionPollMs: Int = 2000, speechFds: FdReceiver? = nil,
                helperVersion: String, os: String, features: [String], module: String) {
        self.permissions = permissions; self.micHub = micHub; self.permissionPollMs = permissionPollMs; self.speechFds = speechFds
        self.helperVersion = helperVersion; self.os = os; self.features = features; self.module = module
    }
}

/// How a part reaches its app: the line writer (every part sees each line) and the arbiter. Made before the parts,
/// bound to the host once it is built (until then the arbiter refuses: a part never takes the mic unadmitted).
public final class HostLink: @unchecked Sendable {
    private let raw: @Sendable (HelperMessage) -> Void
    private let lock = NSLock()
    private var parts: [Weak] = []
    private weak var host: ModuleHost?
    private struct Weak { weak var part: (any ModulePart)? }

    public init(send raw: @escaping @Sendable (HelperMessage) -> Void) { self.raw = raw }

    public func send(_ m: HelperMessage) {
        let ps = lock.withLock { parts.compactMap(\.part) }
        for p in ps { p.willSend(m) }
        raw(m)
        for p in ps { p.didSend(m) }
    }

    /// `send` as a closure, for code that takes one.
    public var sender: @Sendable (HelperMessage) -> Void { { [self] m in self.send(m) } }

    func bind(_ h: ModuleHost, _ ps: [any ModulePart]) {
        lock.withLock { host = h; parts = ps.map { Weak(part: $0) } }
    }

    private var bound: ModuleHost? { lock.withLock { host } }

    /// Runs a command through the app's arbiter (see `ModuleHost.admit`). False when refused, ignored or unbound.
    public func admit(_ e: ArbiterEvent, id: String? = nil, audioPath: String? = nil) async -> Bool {
        guard let h = bound else { return false }
        return await h.admit(e, id: id, audioPath: audioPath)
    }

    /// An event whose decision doesn't matter to the caller (a released admission): only its effects are applied.
    public func step(_ e: ArbiterEvent) async { await bound?.step(e) }

    /// H5: start polling TCC if anything now holds the mic.
    public func watchPermissions() async { await bound?.watchPermissions() }
}

public actor ModuleHost {
    private let deps: HostDeps
    private let link: HostLink
    private let now: @Sendable () -> Double  // monotonic ms
    private let startedMs: Double
    nonisolated public let parts: [any ModulePart]
    private var shuttingDown = false

    /// H6/H5 arbiter state (see Arbiter.swift); `lastPerms` is the TCC snapshot revocations are diffed against.
    public private(set) var arbiter: ArbiterState
    private var lastPerms: [OsPermission: PermissionState]
    private var permWatch: Task<Void, Never>?
    /// The speech module's audio sinks (recordings and dictations).
    nonisolated public let speech: SpeechSinks

    public init(deps: HostDeps, link: HostLink, now: @escaping @Sendable () -> Double, speech: SpeechSinks, parts: [any ModulePart]) {
        self.deps = deps; self.link = link; self.now = now; self.speech = speech; self.parts = parts
        self.startedMs = now()
        let st = deps.permissions.status()
        self.lastPerms = st
        self.arbiter = ArbiterState(denied: OsPermission.allCases.filter { st[$0] == .denied })
        link.bind(self, parts)
    }

    private func send(_ m: HelperMessage) { link.send(m) }

    /// ms since recording start (nano's clock), else since the app started. Used for atMs fields.
    nonisolated public func relativeMs() -> Double {
        for p in parts { if let t = p.clockMs() { return t } }
        return now() - startedMs
    }

    public func start() async {
        send(.hello(protocol: helperProtocolVersion, helperVersion: deps.helperVersion, os: deps.os, features: deps.features))
        send(.permissionsStatus(deps.permissions.status()))
        deps.micHub?.addLostListener { [weak self] reason in Task { await self?.micDeviceLost(reason) } }
        for p in parts { await p.start() }
    }

    public func handle(_ msg: HostMessage) async {
        for p in parts { p.observe(msg) }
        switch msg {
        case .ping(let id): send(.pong(id: id))
        case .permissionsCheck: send(.permissionsStatus(deps.permissions.status()))
        case .permissionsRequest(let p):
            await deps.permissions.request(p)
            send(.permissionsStatus(deps.permissions.status()))
        case .shutdown: await shutdown()
        case .speechAttach(let sid, let target, let channels):
            if let why = speech.accept(deps.speechFds, sessionId: sid, target: target, channels: channels) {
                send(.speechAttachRejected(sessionId: sid, reason: why))
            } else {
                send(.speechAttached(sessionId: sid))
            }
        default:
            if let p = parts.first(where: { $0.owns(msg) }) {
                await p.handle(msg)
            } else {
                // The host routes each command to the app that owns it; one landing here is a routing bug.
                send(.log(level: .warn, msg: "\(deps.module): \(Self.name(msg)) isn't this app's; ignored"))
            }
        }
    }

    static func name(_ m: HostMessage) -> String {
        if case .object(let o) = m.json, case .string(let t)? = o["type"] { return t }
        return "command"
    }

    public func heartbeat() { send(.heartbeat(ts: now())) }

    /// Runs an event through the arbiter and carries out its mic effects (the arbiter is enforcing: its `mic`
    /// list is the hub's reader list). An ignored start answers error{recording.busy | dictation.busy} naming both
    /// the ignored start (`id` / `audioPath`) and the session that owns the app (`activeId` / `activeAudioPath`),
    /// so the host can tell its own start apart and clean up an orphan; a refused dictation.start answers
    /// error{dictation.mic}; other ignored commands are logged.
    @discardableResult
    public func admit(_ e: ArbiterEvent, id: String? = nil, audioPath: String? = nil) async -> Bool {
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
                var active: String?
                for p in parts where active == nil { active = await p.activeDictationId() }
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

    /// An event whose decision the caller doesn't need: state and effects only.
    public func step(_ e: ArbiterEvent) async {
        let r = SessionArbiter.step(arbiter, e)
        arbiter = r.state
        await apply(r.effects)
    }

    private func apply(_ effects: [String]) async {
        for fx in effects {
            switch fx {
            case "mic.subscribe:recording": deps.micHub?.permit("recording")
            case "mic.unsubscribe:recording": deps.micHub?.revokePermit("recording"); deps.micHub?.unsubscribe("recording")
            default:
                // Dictation's mic and a revoked dictation are echo's; degrade:* is applied by the host;
                // recording.lost: the caller already tore the recording down.
                for p in parts { await p.apply(effect: fx) }
            }
        }
    }

    /// Anything holding (or wanting) the mic: a recording, a dictation, or the warm mic, even with nothing running,
    /// so a revoke drops the warm reader promptly and a re-grant brings it back.
    private var pollWanted: Bool { arbiter.recording || arbiter.dictating || arbiter.warm }

    /// H5: polls TCC while anything captures or the mic is warm; stops by itself once idle.
    public func watchPermissions() {
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
                    for part in parts { await part.micRevoked() }
                }
            } else if now == .granted, was == .denied {
                await admit(.granted(p))
            }
        }
    }

    /// N138: the mic's device went away and didn't come back. A dictation ends with what was heard
    /// (`dictation.stopped{reason: mic-lost}`); a recording goes on with system audio and says so once
    /// (`capture.degraded{no-mic}`).
    func micDeviceLost(_ reason: String) async {
        send(.log(level: .warn, msg: "mic: input device lost (\(reason))"))
        for p in parts { await p.micDeviceLost(reason) }
    }

    public func shutdown() async {
        guard !shuttingDown else { return }
        shuttingDown = true
        for p in parts { await p.beginShutdown() }
        permWatch?.cancel(); permWatch = nil
        for p in parts { await p.shutdown() }
        speech.endAll()
    }
}
