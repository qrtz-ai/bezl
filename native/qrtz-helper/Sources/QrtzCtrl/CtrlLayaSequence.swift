// The Laya prompt and the Core ML inputs (ADR 0129), pure and Foundation-only so it is tested on Linux. A port of
// laya's `build_sequence` as the pinned laya-coreml config describes it:
//   [CLS] <type> question: <instructions> [SEP] ([MASK] <option>)* [SEP] <state> [SEP]
// Options are capped at 48 tokens and share a 256-token head budget with the instructions; the rest of the bucket
// goes to the state, cut on the right. The model (laya_multilingual_e8_L512_options32) takes
//   input_ids int32 [1, L], attention_mask int32 [1, L], marker_map float32 [1, 32, L], question_type float32 [1, 3]
// and answers logits / probabilities float32 [1, 32] (option slots past the options are masked) and
// action_probabilities float32 [1, 2].
import Foundation

public enum CtrlLayaQuestionType: Int {
    case choice = 0, score = 1, noul = 2
    var name: String { ["choice", "score", "noul"][rawValue] }
}

public struct CtrlLayaInputs: Equatable {
    public static let length = 512
    public static let maxOptions = 32
    public static let headMaxLength = 256
    public static let optionTokenCap = 48
    public static let names = (ids: "input_ids", mask: "attention_mask", markers: "marker_map", type: "question_type")
    public static let outputName = "probabilities"

    /// The sequence before padding, and where each option's `<mask>` sits.
    public let ids: [Int]
    public let markers: [Int]
    public let questionType: CtrlLayaQuestionType

    /// `input_ids`, padded with `<pad>` to `length`.
    public func inputIds(pad: Int) -> [Int32] { (ids + Array(repeating: pad, count: Self.length - ids.count)).map { Int32($0) } }
    /// `attention_mask`: 1 over the sequence, 0 over the padding.
    public var attentionMask: [Int32] { Array(repeating: 1, count: ids.count) + Array(repeating: 0, count: Self.length - ids.count) }
    /// `marker_map` row-major [32, L]: row k is one-hot at option k's `<mask>`; rows past the options stay 0.
    public var markerMap: [Float] {
        var m = [Float](repeating: 0, count: Self.maxOptions * Self.length)
        for (k, p) in markers.enumerated() { m[k * Self.length + p] = 1 }
        return m
    }
    /// `question_type` one-hot over choice / score / noul.
    public var questionTypeVector: [Float] { (0..<3).map { $0 == questionType.rawValue ? 1 : 0 } }

    /// Build the inputs; `nil` when there are no options, more than 32, or the head leaves an option out of the window.
    public static func build(tokenizer t: CtrlLayaTokenizer, type: CtrlLayaQuestionType = .choice, state: String,
                             question: String, options: [String]) -> CtrlLayaInputs? {
        guard !options.isEmpty, options.count <= maxOptions else { return nil }
        let strip = { (s: String) in s.replacingOccurrences(of: CtrlLayaTokenizer.maskToken, with: " ") }
        var head = t.encode("\(type.name) question: \(strip(question))")
        var opts = options.map { [t.maskId] + Array(t.encode(" " + strip($0)).prefix(optionTokenCap)) }
        var budget = headMaxLength - opts.reduce(0) { $0 + $1.count }
        if budget < 16 {
            let per = max(4, (headMaxLength - 16) / max(1, opts.count))
            opts = opts.map { Array($0.prefix(per)) }
            budget = headMaxLength - opts.reduce(0) { $0 + $1.count }
        }
        head = Array(head.prefix(max(8, budget)))
        var ids = [t.clsId] + head + [t.sepId]
        var markers: [Int] = []
        for o in opts { markers.append(ids.count); ids += o }
        ids.append(t.sepId)
        let st = t.encode(strip(state))
        let room = max(0, length - ids.count - 1)
        ids = Array((ids + st.prefix(room) + [t.sepId]).prefix(length))
        guard markers.allSatisfy({ $0 < length }) else { return nil }
        return CtrlLayaInputs(ids: ids, markers: markers, questionType: type)
    }

    /// The model's `probabilities` for the first `count` slots, renormalised over the real options. `nil` on a bad
    /// output (too short, not finite, negative, or no mass), which the runtime answers as `laya.bad_model`.
    public static func optionProbabilities(_ raw: [Double], count: Int) -> [Double]? {
        guard count > 0, raw.count >= count else { return nil }
        let p = Array(raw.prefix(count))
        guard p.allSatisfy({ $0.isFinite && $0 >= 0 }) else { return nil }
        let s = p.reduce(0, +)
        guard s > 0 else { return nil }
        return p.map { $0 / s }
    }
}
