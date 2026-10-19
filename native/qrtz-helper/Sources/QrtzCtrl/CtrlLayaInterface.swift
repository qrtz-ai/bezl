// Run 46: the Core ML interface ctrl feeds Laya (ADR 0129), checked once when a model loads instead of failing every
// question. The ports are CtrlLayaInputs' names and shapes; `parseMIL` reads a compiled model's `model.mil` main
// signature so the box can check the pinned model.laya 1.0.0 against them without Core ML (Linux has none).
import Foundation

public enum CtrlLayaInterface {
    public struct Port: Equatable, Sendable, CustomStringConvertible {
        public let name: String
        /// The MIL element type: `int32`, `fp32`, `fp16`.
        public let type: String
        public let shape: [Int]
        public init(_ name: String, _ type: String, _ shape: [Int]) { self.name = name; self.type = type; self.shape = shape }
        public var description: String { "\(name) \(type) \(shape)" }
    }

    /// What `CtrlLayaRuntime.choice` sends (CtrlLayaInputs).
    public static let inputs: [Port] = {
        let n = CtrlLayaInputs.names, L = CtrlLayaInputs.length
        return [Port(n.ids, "int32", [1, L]), Port(n.mask, "int32", [1, L]),
                Port(n.markers, "fp32", [1, CtrlLayaInputs.maxOptions, L]), Port(n.type, "fp32", [1, 3])]
    }()
    /// What it reads back.
    public static let output = Port(CtrlLayaInputs.outputName, "fp32", [1, CtrlLayaInputs.maxOptions])

    /// The first name a loaded model lacks (an input ctrl sends or the output it reads), nil when it has them all.
    /// Extra inputs or outputs are fine (the pinned model also returns `logits` and `action_probabilities`).
    public static func missing(inputs have: Set<String>, outputs out: Set<String>) -> String? {
        if let m = inputs.map(\.name).first(where: { !have.contains($0) }) { return m }
        return out.contains(output.name) ? nil : output.name
    }

    /// A MIL program's `main` inputs and returned names, and the shape of each returned tensor; nil when it has no
    /// readable `main`.
    public static func parseMIL(_ text: String) -> (inputs: [Port], outputs: [Port])? {
        guard let start = text.range(of: "func main<"), let open = text[start.upperBound...].firstIndex(of: "("),
              let close = text[open...].range(of: ") {") else { return nil }
        let sig = String(text[text.index(after: open)..<close.lowerBound])
        let ins = ports(in: sig)
        guard !ins.isEmpty, let ret = text.range(of: "} -> (", options: .backwards),
              let end = text[ret.upperBound...].firstIndex(of: ")") else { return nil }
        let names = text[ret.upperBound..<end].split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        let decls = ports(in: text)
        let outs = names.compactMap { n in decls.last(where: { $0.name == n }) }
        guard outs.count == names.count else { return nil }
        return (ins, outs)
    }

    /// Every `tensor<type, [d, …]> name` in `s`.
    static func ports(in s: String) -> [Port] {
        var out: [Port] = []
        var rest = Substring(s)
        while let t = rest.range(of: "tensor<") {
            rest = rest[t.upperBound...]
            guard let comma = rest.firstIndex(of: ","), let lb = rest.firstIndex(of: "["), let rb = rest.firstIndex(of: "]"),
                  comma < lb, lb < rb else { continue }
            let type = rest[..<comma].trimmingCharacters(in: .whitespaces)
            let dims = rest[rest.index(after: lb)..<rb].split(separator: ",").compactMap { Int($0.trimmingCharacters(in: .whitespaces)) }
            var after = rest[rest.index(after: rb)...]
            guard after.hasPrefix(">") else { continue }
            after = after.dropFirst().drop(while: { $0 == " " })
            let name = String(after.prefix(while: { $0.isLetter || $0.isNumber || $0 == "_" }))
            if !name.isEmpty { out.append(Port(name, type, dims)) }
        }
        return out
    }
}
