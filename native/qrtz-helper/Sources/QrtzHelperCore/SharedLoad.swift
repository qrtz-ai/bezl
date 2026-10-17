import Foundation

/// One in-flight load shared by every caller of a model instance: the startup preload, a dictation that starts
/// while it runs (it joins this load, never starts a second one), a later prepare (returns the loaded value at
/// once). A failed load is forgotten, so the next caller tries again; `clear()` (unload) forgets the value.
public final class SharedLoad<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var task: Task<T, Error>?
    private var value: T?
    public init() {}

    public var loaded: T? { lock.withLock { value } }

    public func get(_ make: @escaping @Sendable () async throws -> T) async throws -> T {
        let t: Task<T, Error> = lock.withLock {
            if let t = task { return t }
            let t = Task { try await make() }
            task = t
            return t
        }
        do {
            let v = try await t.value
            lock.withLock { if task == t { value = v } }
            return v
        } catch {
            lock.withLock { if task == t { task = nil } }
            throw error
        }
    }

    /// Forgets the loaded value (and an in-flight load); returns the value so the caller can release it.
    @discardableResult
    public func clear() -> T? { lock.withLock { let v = value; value = nil; task = nil; return v } }
}
