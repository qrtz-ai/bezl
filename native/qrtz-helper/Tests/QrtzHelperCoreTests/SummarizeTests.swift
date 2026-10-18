import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// A scripted on-device model: records prompts, returns `out` (or `fail`), optionally waits until cancelled.
final class FakeSummarizer: OnDeviceSummarizer, @unchecked Sendable {
    let out: SummaryOutput; let fail: SummarizeFailure?; let hang: Bool
    let prompts = Box<[String]>([])
    init(out: SummaryOutput = SummaryOutput(summary: "ok"), fail: SummarizeFailure? = nil, hang: Bool = false) {
        self.out = out; self.fail = fail; self.hang = hang
    }
    func availability() async -> SummarizeAvailability {
        fail?.code == .unavailable ? SummarizeAvailability(available: false, reason: fail?.reason) : SummarizeAvailability(available: true, contextTokens: 4096)
    }
    func summarize(stage: SummarizeStage, prompt: String) async -> Result<SummaryOutput, SummarizeFailure> {
        prompts.mutate { $0.append("\(stage.rawValue):\(prompt)") }
        if hang {
            while !Task.isCancelled { try? await Task.sleep(nanoseconds: 5_000_000) }
            return .failure(.init(.cancelled, "cancelled"))
        }
        if let fail { return .failure(fail) }
        return .success(out)
    }
}

final class SummarizeTests: XCTestCase {
    func controller(_ s: OnDeviceSummarizer?, _ out: Box<[HelperMessage]>) -> HelperController {
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "t", features: [])
        deps.summarizer = s
        return HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 })
    }

    func testResultIsCappedDataOnTheWire() async throws {
        let long = String(repeating: "a", count: 5000)
        let items = (0..<20).map { SummaryActionItem(text: "do \($0)\u{0007}\n", owner: $0 == 0 ? "  " : "dev") }
        let fake = FakeSummarizer(out: SummaryOutput(summary: long + "\u{202E}", actionItems: items, qa: [SummaryQa(question: "q?", answer: "")]))
        let out = Box<[HelperMessage]>([])
        let c = controller(fake, out)
        await c.handle(.summarize(id: "s1", stage: .single, prompt: "p"))
        await c.summaries.drain()
        guard case .summarizeResult(let id, let o) = out.value.last else { return XCTFail("\(out.value)") }
        XCTAssertEqual(id, "s1")
        XCTAssertEqual(o.summary.utf16.count, SummarizeLimits.summaryMax)
        XCTAssertEqual(o.actionItems.count, SummarizeLimits.actionItemsMax)
        XCTAssertEqual(o.actionItems[0], SummaryActionItem(text: "do 0", owner: nil))
        XCTAssertEqual(o.qa, [])
        // The encoded line parses back (inside the protocol's bounds).
        let line = HelperMessage.summarizeResult(id: id, output: o).line
        XCTAssertEqual(try HelperMessage(json: try JSONValue.parse(line)), .summarizeResult(id: id, output: o))
        XCTAssertEqual(fake.prompts.value, ["single:p"])
    }

    func testNoModelAnswersUnsupported() async {
        let out = Box<[HelperMessage]>([])
        let c = controller(nil, out)
        await c.handle(.summarize(id: "s1", stage: .map, prompt: "p"))
        XCTAssertEqual(out.value.last, .summarizeError(id: "s1", failure: .unavailable(.unsupported)))
    }

    func testUnavailableAndContextErrorsPassThrough() async {
        let out = Box<[HelperMessage]>([])
        let c = controller(FakeSummarizer(fail: .unavailable(.appleIntelligenceOff)), out)
        await c.handle(.summarize(id: "s1", stage: .single, prompt: "p"))
        await c.handle(.summarizeCheck(id: "c1"))
        await c.summaries.drain()
        try? await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertTrue(out.value.contains(.summarizeError(id: "s1", failure: .unavailable(.appleIntelligenceOff))))
        XCTAssertTrue(out.value.contains(.summarizeStatus(id: "c1", available: false, reason: .appleIntelligenceOff, contextTokens: nil)))
    }

    /// A long model call never blocks the controller's other messages, and cancel ends it.
    func testCancelAndTheQueueKeepsMoving() async throws {
        let out = Box<[HelperMessage]>([])
        let c = controller(FakeSummarizer(hang: true), out)
        await c.handle(.summarize(id: "s1", stage: .single, prompt: "p"))
        await c.handle(.ping(id: "x"))
        XCTAssertTrue(out.value.contains(.pong(id: "x")))
        await c.handle(.summarizeCancel(id: "s1"))
        await c.summaries.drain()
        XCTAssertEqual(out.value.last, .summarizeError(id: "s1", failure: .init(.cancelled, "cancelled")))
    }

    func testHostMessageBounds() {
        guard case .failure = HostMessage.parse(line: #"{"type":"summarize","id":"s","stage":"single","prompt":""}"#) else { return XCTFail() }
        guard case .failure = HostMessage.parse(line: #"{"type":"summarize","id":"s","stage":"other","prompt":"p"}"#) else { return XCTFail() }
        let big = String(repeating: "x", count: SummarizeLimits.promptMax + 1)
        guard case .failure = HostMessage.parse(line: #"{"type":"summarize","id":"s","stage":"map","prompt":"\#(big)"}"#) else { return XCTFail() }
    }
}
