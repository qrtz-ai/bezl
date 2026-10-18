import Foundation

/// N138: what a microphone does when its device changes under it (AirPods connect or drop, a USB mic, the input
/// picked in Control Center). Apple's engine stops itself on such a change; this re-opens the input on the new
/// device, trying a few times over about a second while the new device comes up. A newer change supersedes the
/// tries of an older one. `done` gets the time it took, or the last error once every try failed (the device didn't
/// come back: the caller reports it once).
public final class MicRestart: @unchecked Sendable {
    public static let defaultDelaysMs = [0, 100, 250, 500, 1_000]
    private let delaysMs: [Int]
    private let queue: DispatchQueue
    private let lock = NSLock()
    private var gen = 0

    public init(delaysMs: [Int] = MicRestart.defaultDelaysMs, queue: DispatchQueue = DispatchQueue(label: "qrtz.mic.restart")) {
        self.delaysMs = delaysMs.isEmpty ? [0] : delaysMs
        self.queue = queue
    }

    /// `attempt` re-opens the input: nil on success, else why not. Runs on the restart queue, one at a time.
    public func deviceChanged(attempt: @escaping @Sendable () -> String?, done: @escaping @Sendable (Result<Int, MicLost>) -> Void) {
        let g: Int = lock.withLock { gen += 1; return gen }
        step(0, g, Date(), attempt, done)
    }

    private func step(_ i: Int, _ g: Int, _ t0: Date, _ attempt: @escaping @Sendable () -> String?,
                      _ done: @escaping @Sendable (Result<Int, MicLost>) -> Void) {
        queue.asyncAfter(deadline: .now() + .milliseconds(delaysMs[i])) { [weak self] in
            guard let self, self.lock.withLock({ self.gen == g }) else { return }
            if let err = attempt() {
                if i + 1 < self.delaysMs.count { self.step(i + 1, g, t0, attempt, done) } else { done(.failure(MicLost(reason: err))) }
            } else {
                done(.success(Int(Date().timeIntervalSince(t0) * 1000)))
            }
        }
    }

    /// Drops the pending tries (the mic was stopped).
    public func cancel() { lock.withLock { gen += 1 } }

    /// Waits for the restart queue to run what is queued now (tests).
    public func sync() { queue.sync {} }
}

public struct MicLost: Error, Sendable, Equatable { public let reason: String }
