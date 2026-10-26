import Foundation

// Experimental (Jared, 2026-10-26): Phonon-2.5 English as an opportunistic second recogniser beside Parakeet.
// Parakeet stays the default and is never unloaded or replaced; the secondary is used for a clip only when it is
// fully loaded AND the clip is 3...35 s (the community Core ML build's single-shot range). Anything else, or a
// secondary error, decodes that clip on Parakeet. Foundation-only, so the routing is tested on Linux.

/// Which engine heard a clip, and why the primary was used when the secondary was enabled.
public struct EngineRoute: Sendable, Equatable {
    public enum Fallback: String, Sendable { case notLoaded = "not-loaded", tooShort = "too-short", tooLong = "too-long", error }
    public var engine: String
    public var fallback: Fallback?
    public init(engine: String, fallback: Fallback? = nil) { self.engine = engine; self.fallback = fallback }

    public static let minSeconds = 3.0
    public static let maxSeconds = 35.0

    /// The pure rule. `enabled` false: always the primary, no fallback reason (the toggle is off, nothing fell back).
    public static func decide(enabled: Bool, secondaryReady: Bool, seconds: Double,
                              primary: String, secondary: String) -> EngineRoute {
        guard enabled else { return EngineRoute(engine: primary) }
        guard secondaryReady else { return EngineRoute(engine: primary, fallback: .notLoaded) }
        if seconds < minSeconds { return EngineRoute(engine: primary, fallback: .tooShort) }
        if seconds > maxSeconds { return EngineRoute(engine: primary, fallback: .tooLong) }
        return EngineRoute(engine: secondary)
    }
}

/// A `BatchRecognizer` that decodes on `primary` unless the rule above picks the enabled, ready `secondary`.
/// `prepare` loads the primary only; the secondary loads in the background on `setSecondary(enabled: true)` and is
/// unloaded (memory freed) on `false`. The route of every decode is stamped on the result (`engine`, `engineFallback`).
public final class RoutedRecognizer: BatchRecognizer, @unchecked Sendable {
    public let primary: BatchRecognizer
    public let secondary: BatchRecognizer
    public var name: String { primary.name }
    public enum SecondaryState: String, Sendable { case off, loading, ready, error }

    private let lock = NSLock()
    private var enabled = false
    private var state: SecondaryState = .off
    private var generation = 0
    private var loadTask: Task<Void, Never>?
    public private(set) var lastRoute: EngineRoute?
    public var onState: (@Sendable (SecondaryState, String?) -> Void)?

    public init(primary: BatchRecognizer, secondary: BatchRecognizer) { self.primary = primary; self.secondary = secondary }

    public var secondaryState: SecondaryState { lock.withLock { state } }

    public func prepare() async throws { try await primary.prepare() }

    /// Turn the secondary on (background load) or off (unload). Returns once the off unload is done; an on load
    /// keeps running in `loadTask` (awaitable with `waitForSecondary` in tests).
    public func setSecondary(enabled on: Bool) async {
        let (gen, old): (Int, Task<Void, Never>?) = lock.withLock {
            enabled = on; generation += 1; state = on ? .loading : .off
            let o = loadTask; loadTask = nil; return (generation, o)
        }
        old?.cancel()
        onState?(on ? .loading : .off, nil)
        guard on else { await secondary.unload(); return }
        let task = Task { [weak self] in
            guard let self else { return }
            do {
                try await self.secondary.prepare()
                let ok = self.lock.withLock { () -> Bool in
                    guard self.generation == gen else { return false }
                    self.state = .ready; return true
                }
                if ok { self.onState?(.ready, nil) } else { await self.secondary.unload() }
            } catch {
                let ok = self.lock.withLock { () -> Bool in
                    guard self.generation == gen else { return false }
                    self.state = .error; return true
                }
                if ok { self.onState?(.error, "\(error)") }
            }
        }
        lock.withLock { if generation == gen { loadTask = task } }
    }

    public func waitForSecondary() async { let t = lock.withLock { loadTask }; await t?.value }

    public func decode(_ samples16k: [Float]) async throws -> BatchResult {
        let (on, ready) = lock.withLock { (enabled, state == .ready) }
        var route = EngineRoute.decide(enabled: on, secondaryReady: ready, seconds: Double(samples16k.count) / 16_000,
                                       primary: primary.name, secondary: secondary.name)
        var r: BatchResult
        if route.engine == secondary.name && route.fallback == nil {
            do { r = try await secondary.decode(samples16k) } catch {
                route = EngineRoute(engine: primary.name, fallback: .error)
                r = try await primary.decode(samples16k)
            }
        } else {
            r = try await primary.decode(samples16k)
        }
        r.engine = route.engine; r.engineFallback = route.fallback?.rawValue
        lock.withLock { lastRoute = route }
        return r
    }

    public func unload() async {
        await setSecondary(enabled: false)
        await primary.unload()
    }
}
