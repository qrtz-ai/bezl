// The Laya tokenizer and inputs (ADR 0129) against ids from HF `tokenizers` on the pinned laya-coreml tokenizer.json
// (Fixtures/ctrl/laya-tokenizer-cases.json; the vocabulary is a tiny derived subset, laya-tokenizer-mini.json). With
// QRTZ_CTRL_LAYA_TOKENIZER_FULL set to the real 34 MB file the same cases also run on it. Pure; Linux and macOS.
import Foundation
import XCTest
@testable import QrtzCtrl

private let fixtures = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Fixtures/ctrl").standardized

private struct Cases: Decodable {
    struct Enc: Decodable { let text: String; let ids: [Int] }
    struct Seq: Decodable { let state: String; let type: String; let question: String; let options: [String]; let ids: [Int]; let markers: [Int] }
    let encode: [Enc]
    let sequences: [Seq]
}

/// JSONDecoder may drop a leading U+FEFF from a string, so case texts are re-read with the tokenizer's own scanner.
private func load(_ url: URL = fixtures.appendingPathComponent("laya-tokenizer-cases.json")) throws -> (Cases, [String]) {
    let data = try Data(contentsOf: url)
    let c = try JSONDecoder().decode(Cases.self, from: data)
    guard case let .object(top) = try CtrlJSON.parse(data), case let .array(enc)? = top.first(where: { $0.0 == "encode" })?.1 else { throw CtrlFailure("fixture") }
    let texts: [String] = enc.compactMap { if case let .object(o) = $0, case let .string(s)? = o.first(where: { $0.0 == "text" })?.1 { return s }; return nil }
    return (c, texts)
}

final class CtrlLayaTokenizerTests: XCTestCase {
    func tokenizers() throws -> [(String, CtrlLayaTokenizer)] {
        var out = [("mini", try CtrlLayaTokenizer(contentsOf: fixtures.appendingPathComponent("laya-tokenizer-mini.json")))]
        if let full = ProcessInfo.processInfo.environment["QRTZ_CTRL_LAYA_TOKENIZER_FULL"], !full.isEmpty {
            out.append(("full", try CtrlLayaTokenizer(contentsOf: URL(fileURLWithPath: full))))
        }
        return out
    }

    func testEncodeMatchesReferenceIds() throws {
        let (c, texts) = try load()
        XCTAssertEqual(texts.count, c.encode.count)
        XCTAssertTrue(texts.contains("\u{FEFF}bom"), "the BOM case survives the fixture read")
        for (name, t) in try tokenizers() {
            XCTAssertEqual([t.padId, t.eosId, t.bosId, t.unkId, t.maskId], [0, 1, 2, 3, 4], name)
            for (text, e) in zip(texts, c.encode) {
                XCTAssertEqual(t.encode(text), e.ids, "\(name): \(text.debugDescription)")
            }
        }
    }

    /// A bigger sweep for the real file only: QRTZ_CTRL_LAYA_CASES = a cases file made by the fixture generator.
    func testFullSweepWhenGiven() throws {
        let env = ProcessInfo.processInfo.environment
        guard let full = env["QRTZ_CTRL_LAYA_TOKENIZER_FULL"], let sweep = env["QRTZ_CTRL_LAYA_CASES"] else { throw XCTSkip("no sweep") }
        let t = try CtrlLayaTokenizer(contentsOf: URL(fileURLWithPath: full))
        let (c, texts) = try load(URL(fileURLWithPath: sweep))
        var bad = 0
        for (text, e) in zip(texts, c.encode) where t.encode(text) != e.ids { bad += 1; XCTFail(text.debugDescription) }
        print("laya tokenizer sweep: \(texts.count - bad)/\(texts.count) match")
    }

    func testSequencesMatchLayaBuildSequence() throws {
        let (c, _) = try load()
        for (name, t) in try tokenizers() {
            for s in c.sequences {
                let x = try XCTUnwrap(CtrlLayaInputs.build(tokenizer: t, state: s.state, question: s.question, options: s.options))
                XCTAssertEqual(x.ids, s.ids, "\(name): \(s.question)")
                XCTAssertEqual(x.markers, s.markers, name)
                XCTAssertTrue(x.markers.allSatisfy { x.ids[$0] == t.maskId })
                XCTAssertLessThanOrEqual(x.ids.count, CtrlLayaInputs.length)
            }
        }
    }

    func testModelInputsHaveTheModelsShapes() throws {
        let t = try CtrlLayaTokenizer(contentsOf: fixtures.appendingPathComponent("laya-tokenizer-mini.json"))
        let x = try XCTUnwrap(CtrlLayaInputs.build(tokenizer: t, state: "task: every friday empty the trash",
                                                   question: "what kind of task is this?", options: ["clean a folder", "empty the trash"]))
        let L = CtrlLayaInputs.length
        let ids = x.inputIds(pad: t.padId)
        XCTAssertEqual(ids.count, L)
        XCTAssertEqual(ids.first, Int32(t.clsId))
        XCTAssertEqual(ids[x.ids.count - 1], Int32(t.sepId))
        XCTAssertTrue(ids[x.ids.count...].allSatisfy { $0 == Int32(t.padId) })
        XCTAssertEqual(x.attentionMask.count, L)
        XCTAssertEqual(x.attentionMask.reduce(0, +), Int32(x.ids.count))
        let mm = x.markerMap
        XCTAssertEqual(mm.count, 32 * L)
        XCTAssertEqual(mm.reduce(0, +), 2)
        for (k, p) in x.markers.enumerated() { XCTAssertEqual(mm[k * L + p], 1) }
        XCTAssertEqual(x.questionTypeVector, [1, 0, 0])
        XCTAssertEqual(CtrlLayaInputs.names.ids, "input_ids")
        XCTAssertEqual(CtrlLayaInputs.names.markers, "marker_map")
        XCTAssertEqual(CtrlLayaInputs.outputName, "probabilities")
        // no options / too many / mask text in a prompt
        XCTAssertNil(CtrlLayaInputs.build(tokenizer: t, state: "s", question: "q", options: []))
        XCTAssertNil(CtrlLayaInputs.build(tokenizer: t, state: "s", question: "q", options: Array(repeating: "x", count: 33)))
        let m = try XCTUnwrap(CtrlLayaInputs.build(tokenizer: t, state: "a<mask>b", question: "q<mask>", options: ["<mask>x"]))
        XCTAssertEqual(m.ids.filter { $0 == t.maskId }.count, 1, "only the option marker is a <mask>")
    }

    func testOptionProbabilities() {
        let p = CtrlLayaInputs.optionProbabilities([0.6, 0.2, 0, 0], count: 2)!
        XCTAssertEqual(p[0], 0.75, accuracy: 1e-12)
        XCTAssertEqual(p[1], 0.25, accuracy: 1e-12)
        XCTAssertNil(CtrlLayaInputs.optionProbabilities([0.5], count: 2))
        XCTAssertNil(CtrlLayaInputs.optionProbabilities([.nan, 0.5], count: 2))
        XCTAssertNil(CtrlLayaInputs.optionProbabilities([0, 0], count: 2))
    }

    func testRefusesAFileThatIsNotTheLayaTokenizer() {
        XCTAssertThrowsError(try CtrlLayaTokenizer(data: Data("{}".utf8)))
        XCTAssertThrowsError(try CtrlLayaTokenizer(data: Data("{\"model\":{\"type\":\"WordPiece\"}}".utf8)))
        XCTAssertThrowsError(try CtrlLayaTokenizer(data: Data("not json".utf8)))
    }

    func testJSONScannerKeepsScalarsExactly() throws {
        let v = try CtrlJSON.parse(Data("{\"a\":\"\\ufeffx\\ud83d\\uddd1\",\"\u{4E86}\":1,\"\u{F9BA}\":2}".utf8))
        guard case let .object(o) = v, case let .string(s) = o[0].1 else { return XCTFail() }
        XCTAssertEqual(s.unicodeScalars.map(\.value), [0xFEFF, 0x78, 0x1F5D1])
        XCTAssertEqual(o.count, 3)
        XCTAssertNotEqual(ScalarKey(o[1].0), ScalarKey(o[2].0), "canonically equivalent keys stay distinct")
    }
}
