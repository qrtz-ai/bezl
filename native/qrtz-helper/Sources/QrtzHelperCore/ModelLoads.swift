import Foundation

/// N137: one model load at a time per kind ("asr", "diarizer"), shared by `models.warm` and recordings.
///
/// A warm-up that is already compiling a model when a recording starts can't be interrupted (N127 cancels it only
/// between models). Without this the recording's own load of the same model ran next to it: two cold compiles
/// competing for the Neural Engine and two instances resident. Through the gate the recording waits for the
/// in-flight load (which unloads its instance before leaving the gate), then loads from the compiled cache: it
/// reuses that compile instead of competing with it, and only one instance of the model is ever resident.
public actor ModelLoads {
    private var busy: Set<String> = []
    private var waiters: [String: [CheckedContinuation<Void, Never>]] = [:]
    public init() {}

    func acquire(_ kind: String) async {
        if busy.insert(kind).inserted { return }
        await withCheckedContinuation { waiters[kind, default: []].append($0) }  // handed over by release, still busy
    }

    func release(_ kind: String) {
        if var w = waiters[kind], !w.isEmpty {
            let next = w.removeFirst()
            waiters[kind] = w.isEmpty ? nil : w
            next.resume()
        } else {
            busy.remove(kind)
        }
    }

    /// Runs `body` with the `kind` gate held (FIFO).
    public nonisolated func load<T: Sendable>(_ kind: String, _ body: @Sendable () async throws -> T) async throws -> T {
        await acquire(kind)
        do {
            let v = try await body()
            await release(kind)
            return v
        } catch {
            await release(kind)
            throw error
        }
    }
}
