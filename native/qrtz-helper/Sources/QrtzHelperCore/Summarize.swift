import Foundation

// ADR 0075: the summarize service. Its wire types live in QrtzShared (SummarizeTypes.swift).

/// The platform model (FoundationModels on macOS 26+; fakes in tests).
public protocol OnDeviceSummarizer: Sendable {
    func availability() async -> SummarizeAvailability
    /// Must return promptly with `cancelled` when its task is cancelled.
    func summarize(stage: SummarizeStage, prompt: String) async -> Result<SummaryOutput, SummarizeFailure>
}

/// Runs `summarize` requests one at a time off the controller's message queue (a model call takes tens of
/// seconds; recording and dictation messages must not wait behind it). `summarize.cancel` stops one.
public actor SummarizeService {
    private let model: OnDeviceSummarizer?
    private let send: @Sendable (HelperMessage) -> Void
    private var tasks: [String: Task<Void, Never>] = [:]
    private var tail: Task<Void, Never>?

    public init(model: OnDeviceSummarizer?, send: @escaping @Sendable (HelperMessage) -> Void) {
        self.model = model; self.send = send
    }

    public func check(id: String) async {
        guard let model else { send(.summarizeStatus(id: id, available: false, reason: .unsupported, contextTokens: nil)); return }
        let a = await model.availability()
        send(.summarizeStatus(id: id, available: a.available, reason: a.available ? nil : (a.reason ?? .unsupported), contextTokens: a.contextTokens))
    }

    /// Queues the request and returns at once.
    public func start(id: String, stage: SummarizeStage, prompt: String) {
        guard let model else { send(.summarizeError(id: id, failure: .unavailable(.unsupported))); return }
        guard tasks[id] == nil else { send(.summarizeError(id: id, failure: .init(.busy, "duplicate id"))); return }
        let prev = tail
        let send = self.send
        let t = Task { [weak self] in
            await prev?.value
            let r: Result<SummaryOutput, SummarizeFailure> = Task.isCancelled
                ? .failure(.init(.cancelled, "cancelled"))
                : await model.summarize(stage: stage, prompt: prompt)
            switch r {
            case .success(let out): send(.summarizeResult(id: id, output: out.capped))
            case .failure(let f): send(.summarizeError(id: id, failure: f))
            }
            await self?.finished(id)
        }
        tasks[id] = t
        tail = t
    }

    public func cancel(id: String) { tasks[id]?.cancel() }

    private func finished(_ id: String) { tasks[id] = nil }

    /// Tests: wait for everything queued so far.
    public func drain() async { await tail?.value }
}
