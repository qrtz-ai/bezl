import Foundation

/// Echo's half of the helper (ADR 0146 M2): the dictation engine and the push key's armed mic
/// (docs/echo/dictation-tuning.md item 2). The mic itself is admitted by the wrapper's arbiter (`HelperController`),
/// reached through `admit`; nothing here touches a recording. When echo runs as its own app (qrtz-echo) this actor is
/// its controller, with an arbiter of its own.
public actor EchoController {
    public nonisolated let dictation: DictationEngine
    private let admit: AdmitRelay
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

    init(dictation: DictationEngine, admit: AdmitRelay) {
        self.dictation = dictation; self.admit = admit
    }

    /// Test seam.
    public var isArmed: Bool { armed }

    /// The push key's lines, seen as they are sent (the arm doesn't wait for the host).
    func pushKey(_ a: HotkeyAction) async {
        switch a {
        case .down:
            guard !armed, !configuredWarm, !shuttingDown, await dictation.canArm else { return }
            armed = true; armGen += 1
            await dictation.arm(maxMs: Self.armMaxMs)
            _ = await admit(.configure(warmMic: true))
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
        _ = await admit(.configure(warmMic: configuredWarm))
    }

    /// `dictation.configure` (the pill's flags are the wrapper's).
    func configure(warmMic warm: Bool, preRollMs pre: Int, streamAudio stream: Bool, pushKey key: String,
                   consumeFn consume: Bool?, echoGate: Bool?) async {
        await dictation.setEchoGate(echoGate ?? true)
        configuredWarm = warm
        if warm, armed { armed = false; armHeld = false; await dictation.disarm() }
        await dictation.configure(warmMic: warm, preRollMs: pre, streamAudio: stream, pushKey: key, consumeFn: consume ?? true)
        _ = await admit(.configure(warmMic: warm || armed || armHeld))
    }

    /// `dictation.start`, after the arbiter admitted it.
    func start(id: String) async {
        let fromArm = armed
        if fromArm { armed = false; armHeld = true }
        await dictation.start(id: id)
        // The engine could not start (no source): undo through the arbiter so the reader is released too.
        if await !dictation.isActive {
            _ = await admit(.cmd("dictation.cancel"))
            if armHeld { await releaseArm() }
        }
    }

    func stop(id: String) async {
        guard await dictation.activeId == id else { return }
        await dictation.stop(id: id)
        _ = await admit(.cmd("dictation.stop"))
        if armHeld { await releaseArm() }
    }

    func cancel(id: String) async {
        guard await dictation.activeId == id else { return }
        await dictation.cancel(id: id)
        _ = await admit(.cmd("dictation.cancel"))
        if armHeld { await releaseArm() }
    }

    /// No new arm from here on (the wrapper then stops the recording and calls `shutdown`).
    func beginShutdown() { shuttingDown = true }

    func shutdown() async { await dictation.shutdown() }
}

/// The arbiter as echo reaches it: set to the wrapper's `admit` once the wrapper is built.
final class AdmitRelay: @unchecked Sendable {
    private let lock = NSLock()
    private var f: (@Sendable (ArbiterEvent) async -> Bool)?
    func set(_ g: @escaping @Sendable (ArbiterEvent) async -> Bool) { lock.withLock { f = g } }
    func callAsFunction(_ e: ArbiterEvent) async -> Bool {
        guard let f = lock.withLock({ f }) else { return false }
        return await f(e)
    }
}
