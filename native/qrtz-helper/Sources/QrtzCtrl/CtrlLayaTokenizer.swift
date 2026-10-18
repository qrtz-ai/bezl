// The Laya tokenizer (ADR 0129): reads the pinned laya-coreml `tokenizer.json` (mmBERT / Gemma vocabulary, a
// byte-fallback BPE model with a Metaspace pre-tokenizer) and produces the token ids the Core ML model takes, the way
// HF `tokenizers` encodes with `add_special_tokens = false`. Foundation only, so it builds and is tested on Linux
// against ids from the reference implementation (Fixtures/ctrl/laya-tokenizer-cases.json). Encoding only.
//
// Two traps of the vocabulary decide the shape of this file: a few keys start with U+FEFF, which Foundation's JSON
// parser may strip, and Swift `String` equality merges canonically equivalent keys (U+4E86 vs U+F9BA). So the file
// is read with a small JSON scanner of its own and every table is keyed by Unicode scalars, never by `String`.
import Foundation

public struct CtrlLayaTokenizerError: Error, Equatable, CustomStringConvertible {
    public let code: String
    public var description: String { code }
}

/// Scalars as a dictionary key: compared and hashed by code point, never by canonical equivalence.
struct ScalarKey: Hashable {
    let v: [UInt32]
    init(_ v: [UInt32]) { self.v = v }
    init(_ s: String) { v = s.unicodeScalars.map(\.value) }
}

public final class CtrlLayaTokenizer {
    public let padId: Int, eosId: Int, bosId: Int, unkId: Int, maskId: Int
    /// laya's `[CLS]` is `<bos>`, its `[SEP]` is `<eos>`, an option's marker is `<mask>`.
    public var clsId: Int { bosId }
    public var sepId: Int { eosId }
    public static let maskToken = "<mask>"
    private static let space: UInt32 = 0x2581

    private struct Added { let scalars: [UInt32]; let id: Int; let lstrip: Bool; let rstrip: Bool }
    private struct Pair: Hashable { let a: Int32; let b: Int32 }
    private let vocab: [ScalarKey: Int32]
    private let merges: [Pair: (rank: Int32, id: Int32)]
    private let bytes: [Int32?]
    private let added: [UInt32: [Added]]
    private let fuseUnk: Bool

    public convenience init(contentsOf url: URL) throws {
        try self.init(data: try Data(contentsOf: url))
    }

    public init(data: Data) throws {
        let bad = { (c: String) in CtrlLayaTokenizerError(code: "laya.tokenizer.\(c)") }
        let root: CtrlJSON
        do { root = try CtrlJSON.parse(data) } catch { throw bad("json") }
        guard case let .object(top) = root, let model = top.first(where: { $0.0 == "model" })?.1,
              case let .object(m) = model else { throw bad("no_model") }
        func field(_ k: String) -> CtrlJSON? { m.first(where: { $0.0 == k })?.1 }
        guard case .string("BPE")? = field("type"), case .bool(true)? = field("byte_fallback"),
              case let .object(vocabList)? = field("vocab"), case let .array(mergeList)? = field("merges")
        else { throw bad("not_byte_fallback_bpe") }
        if case .string? = field("continuing_subword_prefix") { throw bad("unsupported") }
        if case .string? = field("end_of_word_suffix") { throw bad("unsupported") }
        if case let .bool(b)? = field("fuse_unk") { fuseUnk = b } else { fuseUnk = false }

        var vocab = [ScalarKey: Int32](minimumCapacity: vocabList.count)
        for (k, v) in vocabList {
            guard case let .number(n) = v else { throw bad("vocab") }
            vocab[ScalarKey(k)] = Int32(n)
        }
        guard vocab.count == vocabList.count else { throw bad("vocab_collision") }
        var addedBy: [UInt32: [Added]] = [:]
        if case let .array(list)? = top.first(where: { $0.0 == "added_tokens" })?.1 {
            for e in list {
                guard case let .object(o) = e else { continue }
                func f(_ k: String) -> CtrlJSON? { o.first(where: { $0.0 == k })?.1 }
                guard case let .string(content)? = f("content"), case let .number(id)? = f("id"), !content.isEmpty else { continue }
                if case .bool(true)? = f("normalized") { throw bad("unsupported") }
                let lstrip: Bool; if case .bool(true)? = f("lstrip") { lstrip = true } else { lstrip = false }
                let rstrip: Bool; if case .bool(true)? = f("rstrip") { rstrip = true } else { rstrip = false }
                let s = content.unicodeScalars.map(\.value)
                addedBy[s[0], default: []].append(Added(scalars: s, id: Int(id), lstrip: lstrip, rstrip: rstrip))
                if vocab[ScalarKey(s)] == nil { vocab[ScalarKey(s)] = Int32(id) }
            }
        }
        for k in addedBy.keys { addedBy[k]!.sort { $0.scalars.count > $1.scalars.count } }
        self.added = addedBy

        var merges = [Pair: (rank: Int32, id: Int32)](minimumCapacity: mergeList.count)
        for (rank, e) in mergeList.enumerated() {
            let a: String, b: String
            switch e {
            case let .array(p):
                guard p.count == 2, case let .string(x) = p[0], case let .string(y) = p[1] else { throw bad("merges") }
                a = x; b = y
            case let .string(s):
                let parts = s.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: false)
                guard parts.count == 2 else { throw bad("merges") }
                a = String(parts[0]); b = String(parts[1])
            default: throw bad("merges")
            }
            let ka = ScalarKey(a), kb = ScalarKey(b)
            guard let ia = vocab[ka], let ib = vocab[kb], let ic = vocab[ScalarKey(ka.v + kb.v)] else { throw bad("merge_not_in_vocab") }
            let p = Pair(a: ia, b: ib)
            if merges[p] == nil { merges[p] = (Int32(rank), ic) }
        }
        self.merges = merges
        self.vocab = vocab
        bytes = (0..<256).map { vocab[ScalarKey(String(format: "<0x%02X>", $0))] }
        func need(_ t: String) throws -> Int {
            guard let id = vocab[ScalarKey(t)] else { throw bad("missing_\(t.trimmingCharacters(in: CharacterSet(charactersIn: "<>")))") }
            return Int(id)
        }
        padId = try need("<pad>"); eosId = try need("<eos>"); bosId = try need("<bos>")
        unkId = try need("<unk>"); maskId = try need(Self.maskToken)
    }

    /// Token ids for `text` without any special-token template (`<bos>` / `<eos>` are the caller's).
    public func encode(_ text: String) -> [Int] {
        let s = text.unicodeScalars.map(\.value)
        var ids: [Int] = []
        var seg: [UInt32] = []
        var i = 0
        while i < s.count {
            if let cands = added[s[i]], let t = cands.first(where: { tok in
                i + tok.scalars.count <= s.count && Array(s[i..<(i + tok.scalars.count)]) == tok.scalars
            }) {
                if t.lstrip { while let l = seg.last, Self.isSpace(l) { seg.removeLast() } }
                segment(seg, into: &ids)
                seg.removeAll(keepingCapacity: true)
                ids.append(t.id)
                i += t.scalars.count
                if t.rstrip { while i < s.count, Self.isSpace(s[i]) { i += 1 } }
                continue
            }
            seg.append(s[i])
            i += 1
        }
        segment(seg, into: &ids)
        return ids
    }

    static func isSpace(_ v: UInt32) -> Bool {
        Unicode.Scalar(v)?.properties.isWhitespace ?? false
    }

    /// Normalizer (space → ▁), Metaspace (prepend ▁ always, split before each ▁), then BPE per piece.
    private func segment(_ seg: [UInt32], into ids: inout [Int]) {
        guard !seg.isEmpty else { return }
        var n = seg.map { $0 == 0x20 ? Self.space : $0 }
        if n.first != Self.space { n.insert(Self.space, at: 0) }
        var piece: [UInt32] = []
        for v in n {
            if v == Self.space, !piece.isEmpty { bpe(piece, into: &ids); piece.removeAll(keepingCapacity: true) }
            piece.append(v)
        }
        if !piece.isEmpty { bpe(piece, into: &ids) }
    }

    private func bpe(_ piece: [UInt32], into ids: inout [Int]) {
        if let whole = vocab[ScalarKey(piece)], piece.count == 1 { ids.append(Int(whole)); return }
        // Symbols: a scalar in the vocabulary, else its UTF-8 bytes as <0xNN> (byte fallback), else <unk>.
        var sym: [Int32] = []
        sym.reserveCapacity(piece.count)
        for v in piece {
            if let id = vocab[ScalarKey([v])] { sym.append(id); continue }
            let utf8 = Array(String(Unicode.Scalar(v).map(Character.init) ?? "\u{FFFD}").utf8)
            let bs = utf8.compactMap { bytes[Int($0)] }
            if bs.count == utf8.count { sym.append(contentsOf: bs) }
            else if fuseUnk, sym.last == Int32(unkId) { continue }
            else { sym.append(Int32(unkId)) }
        }
        // Merge the lowest-ranked adjacent pair (leftmost on a tie) until none is left.
        while sym.count > 1 {
            var best: (rank: Int32, at: Int, id: Int32)? = nil
            for j in 0..<(sym.count - 1) {
                if let m = merges[Pair(a: sym[j], b: sym[j + 1])], best == nil || m.rank < best!.rank {
                    best = (m.rank, j, m.id)
                }
            }
            guard let b = best else { break }
            sym[b.at] = b.id
            sym.remove(at: b.at + 1)
        }
        ids.append(contentsOf: sym.map(Int.init))
    }
}

/// A minimal JSON reader that keeps strings exactly as written (no BOM stripping) and objects as ordered pairs.
enum CtrlJSON {
    case object([(String, CtrlJSON)]), array([CtrlJSON]), string(String), number(Double), bool(Bool), null

    struct Bad: Error {}

    static func parse(_ data: Data) throws -> CtrlJSON {
        try data.withUnsafeBytes { raw -> CtrlJSON in
            var p = Parser(b: raw.bindMemory(to: UInt8.self))
            let v = try p.value()
            p.ws()
            guard p.i == p.b.count else { throw Bad() }
            return v
        }
    }

    private struct Parser {
        let b: UnsafeBufferPointer<UInt8>
        var i = 0
        init(b: UnsafeBufferPointer<UInt8>) {
            self.b = b
            if b.count >= 3, b[0] == 0xEF, b[1] == 0xBB, b[2] == 0xBF { i = 3 } // a BOM before the document only
        }
        mutating func ws() { while i < b.count, b[i] == 0x20 || b[i] == 0x0A || b[i] == 0x0D || b[i] == 0x09 { i += 1 } }
        mutating func lit(_ s: String) throws { for c in s.utf8 { guard i < b.count, b[i] == c else { throw Bad() }; i += 1 } }
        mutating func value() throws -> CtrlJSON {
            ws()
            guard i < b.count else { throw Bad() }
            switch b[i] {
            case UInt8(ascii: "{"):
                i += 1; var o: [(String, CtrlJSON)] = []; ws()
                if i < b.count, b[i] == UInt8(ascii: "}") { i += 1; return .object(o) }
                while true {
                    ws(); let k = try string(); ws(); try lit(":")
                    o.append((k, try value())); ws()
                    guard i < b.count else { throw Bad() }
                    if b[i] == UInt8(ascii: ",") { i += 1; continue }
                    if b[i] == UInt8(ascii: "}") { i += 1; return .object(o) }
                    throw Bad()
                }
            case UInt8(ascii: "["):
                i += 1; var a: [CtrlJSON] = []; ws()
                if i < b.count, b[i] == UInt8(ascii: "]") { i += 1; return .array(a) }
                while true {
                    a.append(try value()); ws()
                    guard i < b.count else { throw Bad() }
                    if b[i] == UInt8(ascii: ",") { i += 1; continue }
                    if b[i] == UInt8(ascii: "]") { i += 1; return .array(a) }
                    throw Bad()
                }
            case UInt8(ascii: "\""): return .string(try string())
            case UInt8(ascii: "t"): try lit("true"); return .bool(true)
            case UInt8(ascii: "f"): try lit("false"); return .bool(false)
            case UInt8(ascii: "n"): try lit("null"); return .null
            default:
                let s = i
                while i < b.count, "+-0123456789.eE".utf8.contains(b[i]) { i += 1 }
                guard i > s, let d = Double(String(decoding: UnsafeBufferPointer(rebasing: b[s..<i]), as: UTF8.self)) else { throw Bad() }
                return .number(d)
            }
        }
        mutating func hex4() throws -> UInt32 {
            guard i + 4 <= b.count, let v = UInt32(String(decoding: UnsafeBufferPointer(rebasing: b[i..<(i + 4)]), as: UTF8.self), radix: 16) else { throw Bad() }
            i += 4; return v
        }
        mutating func string() throws -> String {
            guard i < b.count, b[i] == UInt8(ascii: "\"") else { throw Bad() }
            i += 1
            var out = String.UnicodeScalarView()
            var run = i
            func flush(_ to: Int) { if to > run { out.append(contentsOf: String(decoding: UnsafeBufferPointer(rebasing: b[run..<to]), as: UTF8.self).unicodeScalars) } }
            while i < b.count {
                let c = b[i]
                if c == UInt8(ascii: "\"") { flush(i); i += 1; return String(out) }
                if c == UInt8(ascii: "\\") {
                    flush(i); i += 1
                    guard i < b.count else { throw Bad() }
                    let e = b[i]; i += 1
                    switch e {
                    case UInt8(ascii: "n"): out.append("\n")
                    case UInt8(ascii: "t"): out.append("\t")
                    case UInt8(ascii: "r"): out.append("\r")
                    case UInt8(ascii: "b"): out.append("\u{08}")
                    case UInt8(ascii: "f"): out.append("\u{0C}")
                    case UInt8(ascii: "u"):
                        var v = try hex4()
                        if (0xD800..<0xDC00).contains(v), i + 6 <= b.count, b[i] == UInt8(ascii: "\\"), b[i + 1] == UInt8(ascii: "u") {
                            i += 2; let lo = try hex4()
                            guard (0xDC00..<0xE000).contains(lo) else { throw Bad() }
                            v = 0x10000 + ((v - 0xD800) << 10) + (lo - 0xDC00)
                        }
                        out.append(Unicode.Scalar(v) ?? "\u{FFFD}")
                    default: out.append(Unicode.Scalar(e))
                    }
                    run = i
                    continue
                }
                i += 1
            }
            throw Bad()
        }
    }
}
