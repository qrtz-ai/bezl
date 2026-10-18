import Foundation

/// ADR 0075: on-device meeting summaries: the wire types (QrtzShared; the service is nano's). The host builds and N69-fences every prompt; the helper runs it through
/// the platform model with fixed per-stage instructions and guided generation, and returns capped data only.
/// The model call has no tools, no network and no file access: its only input is the prompt line.

public enum SummarizeStage: String, Sendable, CaseIterable { case single, map, reduce }

public enum SummarizeUnavailable: String, Sendable, CaseIterable {
    case appleIntelligenceOff = "apple-intelligence-off", deviceNotEligible = "device-not-eligible",
         modelNotReady = "model-not-ready", osUnsupported = "os-unsupported", unsupported
}

public enum SummarizeErrorCode: String, Sendable, CaseIterable { case unavailable, context, guardrail, busy, cancelled, failed }

/// zod `SUMMARIZE_LIMITS` (packages/core/src/helper/protocol.ts); the Rust host re-checks them.
public enum SummarizeLimits {
    public static let promptMax = 40_000
    public static let summaryMax = 4_000
    public static let actionItemsMax = 12
    public static let itemTextMax = 400
    public static let nameMax = 80
    public static let qaMax = 8
    public static let questionMax = 300
    public static let answerMax = 600
    public static let messageMax = 300
}

public struct SummaryActionItem: Equatable, Sendable {
    public var text: String
    public var owner: String?
    public init(text: String, owner: String? = nil) { self.text = text; self.owner = owner }
}

public struct SummaryQa: Equatable, Sendable {
    public var question: String
    public var answer: String
    public var askedBy: String?
    public init(question: String, answer: String, askedBy: String? = nil) { self.question = question; self.answer = answer; self.askedBy = askedBy }
}

public struct SummaryOutput: Equatable, Sendable {
    public var summary: String
    public var actionItems: [SummaryActionItem]
    public var qa: [SummaryQa]
    public init(summary: String, actionItems: [SummaryActionItem] = [], qa: [SummaryQa] = []) {
        self.summary = summary; self.actionItems = actionItems; self.qa = qa
    }

    /// One line, no control characters, at most `max` UTF-16 units (cut on whole scalars).
    public static func clean(_ s: String, _ max: Int) -> String {
        var out = String.UnicodeScalarView()
        var n = 0
        var lastSpace = true
        for u in s.unicodeScalars {
            let bad = u.properties.generalCategory == .control || u.properties.generalCategory == .format
                || u.value == 0x2028 || u.value == 0x2029
            let space = bad || u.properties.isWhitespace
            if space { if lastSpace { continue } }
            let c: Unicode.Scalar = space ? " " : u
            let w = c.utf16.count
            if n + w > max { break }
            out.append(c); n += w; lastSpace = space
        }
        return String(out).trimmingCharacters(in: .whitespaces)
    }

    private static func name(_ s: String?) -> String? {
        guard let s else { return nil }
        let c = clean(s, SummarizeLimits.nameMax)
        return c.isEmpty ? nil : c
    }

    /// Everything within the protocol's bounds (the line always validates on the host).
    public var capped: SummaryOutput {
        SummaryOutput(
            summary: Self.clean(summary, SummarizeLimits.summaryMax),
            actionItems: actionItems.map { SummaryActionItem(text: Self.clean($0.text, SummarizeLimits.itemTextMax), owner: Self.name($0.owner)) }
                .filter { !$0.text.isEmpty }.prefix(SummarizeLimits.actionItemsMax).map { $0 },
            qa: qa.map { SummaryQa(question: Self.clean($0.question, SummarizeLimits.questionMax), answer: Self.clean($0.answer, SummarizeLimits.answerMax), askedBy: Self.name($0.askedBy)) }
                .filter { !$0.question.isEmpty && !$0.answer.isEmpty }.prefix(SummarizeLimits.qaMax).map { $0 })
    }
}

public struct SummarizeFailure: Error, Equatable, Sendable {
    public var code: SummarizeErrorCode
    public var reason: SummarizeUnavailable?
    public var message: String
    public init(_ code: SummarizeErrorCode, reason: SummarizeUnavailable? = nil, _ message: String) {
        self.code = code; self.reason = reason; self.message = message
    }
    public static func unavailable(_ r: SummarizeUnavailable) -> SummarizeFailure { .init(.unavailable, reason: r, "on-device model unavailable: \(r.rawValue)") }
}

public struct SummarizeAvailability: Equatable, Sendable {
    public var available: Bool
    public var reason: SummarizeUnavailable?
    public var contextTokens: Int?
    public init(available: Bool, reason: SummarizeUnavailable? = nil, contextTokens: Int? = nil) {
        self.available = available; self.reason = reason; self.contextTokens = contextTokens
    }
}
