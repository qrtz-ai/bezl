// Warming the Laya model at helper start (ADR 0133). Foundation only, so the queueing, the de-duplication and the
// tokenizer load are tested on Linux; the macOS runtime passes its Core ML load as the `work`.
//
// The host sends `ctrl.laya.warm {model}` only with the user's consent and the installed model on disk (ADR 0116).
// `start` checks the files, answers at once and runs `work` on its own background-QoS serial queue: never on ctrl's
// request queue, never on the stdin reader, never ahead of echo/nano. While a path is warming, a question for it
// answers `laya.warming` (the host's heuristic takes that question) instead of waiting behind the load. Nothing
// here downloads anything or touches the network.
import Foundation

public final class CtrlLayaWarmer: @unchecked Sendable {
    public typealias Work = @Sendable (_ modelPath: String) -> Void
    public enum State: String { case started, warming, warm }

    static let kinds: Set<String> = ["mlmodelc", "mlpackage", "mlmodel"]
    private let lock = NSLock()
    private var inflight: Set<String> = []
    private var done: Set<String> = []
    private let queue: DispatchQueue
    private let work: Work

    public init(queue: DispatchQueue = DispatchQueue(label: "ai.qrtz.helper.ctrl.laya-warm", qos: .background),
                work: @escaping Work) {
        self.queue = queue
        self.work = work
    }

    /// The tokenizer for a model: `QRTZ_CTRL_LAYA_TOKENIZER` if set, else `tokenizer.json` beside the model (ADR 0129).
    public static func tokenizerPath(for modelPath: String,
                                     env: [String: String] = ProcessInfo.processInfo.environment) -> String {
        if let e = env["QRTZ_CTRL_LAYA_TOKENIZER"], !e.isEmpty { return e }
        return URL(fileURLWithPath: modelPath).deletingLastPathComponent().appendingPathComponent("tokenizer.json").path
    }

    /// A model the runtime could load: a Core ML model kind that exists, with its tokenizer file.
    public static func eligible(_ modelPath: String, env: [String: String] = ProcessInfo.processInfo.environment) -> Bool {
        let fm = FileManager.default
        return !modelPath.isEmpty && kinds.contains(URL(fileURLWithPath: modelPath).pathExtension)
            && fm.fileExists(atPath: modelPath) && fm.fileExists(atPath: tokenizerPath(for: modelPath, env: env))
    }

    /// Queue the warm-up of `modelPath` and return at once. `laya.no_model` when the files aren't there.
    public func start(_ modelPath: String, env: [String: String] = ProcessInfo.processInfo.environment) throws -> State {
        guard Self.eligible(modelPath, env: env) else { throw CtrlFailure("laya.no_model") }
        lock.lock()
        if done.contains(modelPath) { lock.unlock(); return .warm }
        if inflight.contains(modelPath) { lock.unlock(); return .warming }
        inflight.insert(modelPath)
        lock.unlock()
        queue.async { [self] in
            work(modelPath)
            lock.lock(); inflight.remove(modelPath); done.insert(modelPath); lock.unlock()
        }
        return .started
    }

    /// The warm-up queue's QoS (`.background` unless a test passes its own queue).
    public var qos: DispatchQoS { queue.qos }

    /// True while `modelPath` is being warmed.
    public func isWarming(_ modelPath: String) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return inflight.contains(modelPath)
    }

    /// Wait for queued warm-ups (tests).
    public func drain() { queue.sync {} }
}

/// The tokenizers read so far, one per path (the macOS runtime's cache; tested on Linux through the warmer).
public final class CtrlLayaTokenizerCache: @unchecked Sendable {
    private let lock = NSLock()
    private var tokenizers: [String: CtrlLayaTokenizer] = [:]
    public init() {}

    /// Read once per path; `laya.no_tokenizer` when the file is missing or isn't the Laya tokenizer.
    public func tokenizer(at path: String) throws -> CtrlLayaTokenizer {
        lock.lock()
        if let t = tokenizers[path] { lock.unlock(); return t }
        lock.unlock()
        // Read outside the lock: the real file is 34 MB, and a question for another path shouldn't wait on it.
        guard let t = try? CtrlLayaTokenizer(contentsOf: URL(fileURLWithPath: path)) else { throw CtrlFailure("laya.no_tokenizer") }
        lock.lock(); defer { lock.unlock() }
        if let first = tokenizers[path] { return first }
        tokenizers[path] = t
        return t
    }

    public func cached(_ path: String) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return tokenizers[path] != nil
    }
}
