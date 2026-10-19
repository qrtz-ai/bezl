// Run 46: the pinned model.laya 1.0.0's Core ML interface (its model.mil main signature, fixture from the pinned HF
// file) is exactly what CtrlLayaRuntime sends and reads, and a model missing any of it is named at load.
// QRTZ_CTRL_LAYA_MIL=<path to an installed model.mil> checks the real file as well.
import Foundation
import XCTest
@testable import QrtzCtrl

private let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

final class CtrlLayaInterfaceTests: XCTestCase {
    private func check(_ mil: String, file: StaticString = #filePath, line: UInt = #line) throws {
        let p = try XCTUnwrap(CtrlLayaInterface.parseMIL(mil), "no main", file: file, line: line)
        XCTAssertEqual(p.inputs.sorted { $0.name < $1.name }, CtrlLayaInterface.inputs.sorted { $0.name < $1.name },
                       file: file, line: line)
        XCTAssertTrue(p.outputs.contains(CtrlLayaInterface.output), "\(p.outputs)", file: file, line: line)
        XCTAssertNil(CtrlLayaInterface.missing(inputs: Set(p.inputs.map(\.name)), outputs: Set(p.outputs.map(\.name))),
                     file: file, line: line)
    }

    func testThePinnedModelsSignatureIsWhatCtrlSends() throws {
        let mil = try String(contentsOf: root.appendingPathComponent("Fixtures/ctrl/laya-mil-signature.txt"), encoding: .utf8)
        try check(mil)
        let p = try XCTUnwrap(CtrlLayaInterface.parseMIL(mil))
        XCTAssertEqual(p.outputs.map(\.name), ["logits", "probabilities", "action_probabilities"])
        XCTAssertEqual(CtrlLayaInterface.inputs.map(\.description),
                       ["input_ids int32 [1, 512]", "attention_mask int32 [1, 512]", "marker_map fp32 [1, 32, 512]",
                        "question_type fp32 [1, 3]"])
    }

    func testAModelMissingAnInputOrTheOutputIsNamed() {
        let all = Set(CtrlLayaInterface.inputs.map(\.name))
        XCTAssertNil(CtrlLayaInterface.missing(inputs: all.union(["extra"]), outputs: ["probabilities", "logits"]))
        XCTAssertEqual(CtrlLayaInterface.missing(inputs: all.subtracting(["marker_map"]), outputs: ["probabilities"]), "marker_map")
        XCTAssertEqual(CtrlLayaInterface.missing(inputs: all, outputs: ["logits"]), "probabilities")
        XCTAssertEqual(CtrlLayaInterface.missing(inputs: [], outputs: []), "input_ids")
    }

    func testAnUnreadableOrDifferentProgramIsNotTakenForIt() throws {
        XCTAssertNil(CtrlLayaInterface.parseMIL(""))
        XCTAssertNil(CtrlLayaInterface.parseMIL("program(1.0)\n{\n}\n"))
        let l256 = #"func main<ios17>(tensor<int32, [1, 256]> input_ids, tensor<int32, [1, 256]> attention_mask) {"# +
            "\n tensor<fp32, [1, 8]> probabilities = x\n } -> (probabilities);\n"
        let p = try XCTUnwrap(CtrlLayaInterface.parseMIL(l256))
        XCTAssertNotEqual(p.inputs, CtrlLayaInterface.inputs)
        XCTAssertFalse(p.outputs.contains(CtrlLayaInterface.output), "an 8-slot head is not the 32-slot one")
        XCTAssertEqual(CtrlLayaInterface.missing(inputs: Set(p.inputs.map(\.name)), outputs: Set(p.outputs.map(\.name))),
                       "marker_map")
    }

    func testTheInstalledModelMil() throws {
        guard let path = ProcessInfo.processInfo.environment["QRTZ_CTRL_LAYA_MIL"] else {
            throw XCTSkip("set QRTZ_CTRL_LAYA_MIL to an installed model.mil")
        }
        try check(try String(contentsOfFile: path, encoding: .utf8))
    }

    /// The Mac runtime checks the interface once, at load, and remembers a refused model.
    func testTheMacRuntimeChecksTheInterfaceAtLoad() throws {
        let src = try String(contentsOf: root.appendingPathComponent("Sources/QrtzCtrl/mac/CtrlBackendMac.swift"), encoding: .utf8)
        let load = try XCTUnwrap(src.range(of: "func load(_ model: String?)"))
        let body = String(src[load.upperBound...].prefix(2500))
        XCTAssertTrue(body.contains("CtrlLayaInterface.missing("))
        XCTAssertTrue(body.contains("inputDescriptionsByName") && body.contains("outputDescriptionsByName"))
        XCTAssertTrue(body.contains("failed.insert(path)"))
    }
}
