import Foundation
import XCTest
@testable import QrtzCtrl

/// Run 47: the read-only Mac check finds the installed model.laya and asks it fixed questions.
final class CtrlLayaFirstQuestionTests: XCTestCase {
    private func store() throws -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("laya-first-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: d) }
        return d
    }

    private func model(_ root: URL, _ version: String, _ name: String = "laya_multilingual_e8_L512_options32") throws -> String {
        let m = root.appendingPathComponent("packages/model.laya/\(version)/\(name).mlmodelc")
        try FileManager.default.createDirectory(at: m.appendingPathComponent("weights"), withIntermediateDirectories: true)
        try Data("x".utf8).write(to: m.appendingPathComponent("model.mil"))
        return m.path
    }

    func testNothingInstalledIsNil() throws {
        XCTAssertNil(CtrlLayaFirstQuestion.installedModel(root: try store().path))
    }

    func testTheHighestVersionWithOneCompiledModelWins() throws {
        let root = try store()
        _ = try model(root, "1.0.0")
        let newer = try model(root, "1.10.0")
        _ = try model(root, "1.9.3")
        try FileManager.default.createDirectory(at: root.appendingPathComponent("packages/model.laya/.staging-2.0.0"),
                                                withIntermediateDirectories: true)
        XCTAssertEqual(CtrlLayaFirstQuestion.installedModel(root: root.path), newer, "numeric, not string, order; staging skipped")
    }

    func testAVersionWithTwoModelsOrNoneIsSkipped() throws {
        let root = try store()
        let old = try model(root, "1.0.0")
        _ = try model(root, "2.0.0", "a")
        _ = try model(root, "2.0.0", "b")
        try FileManager.default.createDirectory(at: root.appendingPathComponent("packages/model.laya/3.0.0"),
                                                withIntermediateDirectories: true)
        XCTAssertEqual(CtrlLayaFirstQuestion.installedModel(root: root.path), old)
    }

    func testTheLiveLayoutIsFound() throws {
        // The layout the live install produced (run 45, Fixtures/ctrl/laya-live-layout.json): one .mlmodelc beside
        // tokenizer.json and qrtz.json under packages/model.laya/1.0.0.
        let root = try store()
        let m = try model(root, "1.0.0")
        let dir = URL(fileURLWithPath: m).deletingLastPathComponent()
        try Data("{}".utf8).write(to: dir.appendingPathComponent("tokenizer.json"))
        try Data("{}".utf8).write(to: dir.appendingPathComponent("qrtz.json"))
        XCTAssertEqual(CtrlLayaFirstQuestion.installedModel(root: root.path), m)
        XCTAssertEqual(CtrlLayaWarmer.tokenizerPath(for: m, env: [:]), dir.appendingPathComponent("tokenizer.json").path)
    }

    func testTheQuestionsFitTheModel() {
        for q in CtrlLayaFirstQuestion.questions {
            XCTAssertTrue((1...CtrlLayaInputs.maxOptions).contains(q.options.count), q.question)
            XCTAssertTrue(q.options.indices.contains(q.expect), q.question)
            XCTAssertEqual(q.options, q.options.map { $0.lowercased() })
        }
        XCTAssertEqual(CtrlLayaFirstQuestion.questions[0].state, "task: close all my tabs", "the protocol fixture's example")
    }

    func testTheReportSaysTopExpectedAndErrors() {
        let q = CtrlLayaFirstQuestion.questions[1]
        let r = CtrlLayaFirstQuestion.report(q, probabilities: [0.1, 0.7, 0.1, 0.1], error: nil, ms: 42)
        XCTAssertEqual(r["top"] as? String, "start the screensaver")
        XCTAssertEqual(r["as_expected"] as? Bool, true)
        XCTAssertEqual(r["p"] as? [Double], [0.1, 0.7, 0.1, 0.1])
        XCTAssertEqual(r["ms"] as? Int, 42)
        let wrong = CtrlLayaFirstQuestion.report(q, probabilities: [0.9, 0.05, 0.03, 0.02], error: nil, ms: 1)
        XCTAssertEqual(wrong["as_expected"] as? Bool, false, "reported, not failed")
        XCTAssertEqual(CtrlLayaFirstQuestion.report(q, probabilities: nil, error: "laya.not_read_only", ms: 0)["error"] as? String,
                       "laya.not_read_only")
        XCTAssertEqual(CtrlLayaFirstQuestion.report(q, probabilities: [1], error: nil, ms: 0)["error"] as? String, "laya.bad_model")
    }

    func testItSpawnsAndWritesNothing() throws {
        let src = try String(contentsOf: URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .appendingPathComponent("../../Sources/QrtzCtrl/CtrlLayaFirstQuestion.swift").standardizedFileURL, encoding: .utf8)
        for w in ["Process(", "posix_spawn", "write(to", "createDirectory", "removeItem", "URLSession", "moveItem", "copyItem"] {
            XCTAssertFalse(src.contains(w), w)
        }
    }
}
