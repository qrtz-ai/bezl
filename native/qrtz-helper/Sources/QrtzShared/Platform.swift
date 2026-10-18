import Foundation

// ADR 0146 M2.5: the platform seams more than one module uses (moved from QrtzHelperCore). The macOS targets provide
// real implementations; tests (and Linux) provide fakes.

public protocol PermissionsProvider: Sendable {
    func status() -> [OsPermission: PermissionState]
    /// Prompts (or opens System Settings) for one permission. Returns after the prompt is shown, not when granted.
    func request(_ p: OsPermission) async
}

/// Microphone only, 16 kHz mono Float32. Kept running between dictations when the warm mic is on.
public protocol MicSource: AnyObject, Sendable {
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws
    func stop()
    /// N138: called once when the input device went away and didn't come back. A device switch the source recovers
    /// from (it re-opens the new input and keeps delivering to the same `onSamples`) is not reported.
    func onDeviceLost(_ cb: @escaping @Sendable (String) -> Void)
    /// ADR 0115: hands what the source's resampler still holds back (its filter's last few ms) to `onSamples` now,
    /// as an end of stream, and goes on capturing. A dictation stop calls it after the drain, before the pad.
    func flush()
}

public extension MicSource {
    func onDeviceLost(_ cb: @escaping @Sendable (String) -> Void) {}
    func flush() {}
}

/// A streaming recogniser that exposes one cumulative transcript.
public protocol StreamingTranscriber: AnyObject, Sendable {
    var name: String { get }
    func prepare() async throws
    func append(_ samples16k: [Float]) async
    func transcript() async -> String
    func finish() async -> String
    func reset() async
    /// False for a model that may take seconds to load (a recording then starts at once and its audio waits).
    var loadsInstantly: Bool { get }
    /// How far the transcript trails the audio fed (one decoding chunk), for segment timestamps.
    var latencyMs: Double { get }
    /// Frees the model (a recording ended; the next one loads it again).
    func unload() async
}

extension StreamingTranscriber {
    public var loadsInstantly: Bool { true }
    public var latencyMs: Double { 0 }
    public func unload() async {}
}

// N143: the system audio echo's dictation gate hears (shared: a running recording, nano's, publishes its system
// channel into the same reference echo reads when both run in one process).

/// What the Mac plays (16 kHz mono). Only started for a dictation when `echoLikely()` and the screen & system
/// audio recording grant is already given (a dictation never prompts). `start` / `stop` are called in order.
public protocol SystemAudioTap: AnyObject, Sendable {
    var name: String { get }
    /// The output is speakers (not headphones) and something is playing.
    func echoLikely() -> Bool
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) async throws
    func stop() async
}

/// The system audio lined up with the mic, for the dictation's gate. Fed by whoever runs a system tap ("recording"
/// or "dictation"); the recording wins when both do, so samples are never counted twice.
public final class EchoReference: @unchecked Sendable {
    private let lock = NSLock()
    private var holders: [String] = []
    /// N152: each attach's token; a detach with a stale token (an older dictation's tap failing late) is a no-op.
    private var tokens: [String: Int] = [:]
    private var nextToken = 0
    private var buf: [Float] = []
    private var last: [Float] = []
    /// System audio more than this ahead of the mic is dropped (the two clocks drift; the gate needs the level only).
    public let maxLagSamples: Int
    public init(maxLagMs: Int = 250) { maxLagSamples = 16 * maxLagMs }

    public var isLive: Bool { lock.withLock { !holders.isEmpty } }
    public func has(_ who: String) -> Bool { lock.withLock { holders.contains(who) } }

    /// Returns the token that `detach(_:token:)` must present.
    @discardableResult
    public func attach(_ who: String) -> Int {
        lock.withLock {
            if !holders.contains(who) { holders.append(who) }
            nextToken += 1; tokens[who] = nextToken
            return nextToken
        }
    }
    /// Detaches `who`; with a `token`, only if it is still that attach (not a later one by the same holder).
    public func detach(_ who: String, token: Int? = nil) {
        lock.withLock {
            if let token, tokens[who] != token { return }
            tokens[who] = nil
            holders.removeAll { $0 == who }
            buf.removeAll(); last.removeAll()  // the other holder (if any) refills it from its own clock
        }
    }

    public func push(_ s: [Float], from who: String) {
        lock.withLock {
            guard let lead = holders.contains("recording") ? "recording" : holders.first, lead == who else { return }
            buf += s
            if buf.count > maxLagSamples { buf.removeFirst(buf.count - maxLagSamples) }
        }
    }

    /// The system audio for the next `n` mic samples, oldest first. Nil when no tap feeds the reference; the last
    /// block again when the tap is a block behind the mic.
    public func take(_ n: Int) -> [Float]? {
        lock.withLock {
            guard !holders.isEmpty else { return nil }
            let k = min(n, buf.count)
            guard k > 0 else { return last }
            last = Array(buf.prefix(k)); buf.removeFirst(k)
            return last
        }
    }
}

