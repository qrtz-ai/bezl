// ADR 0133: warming the Laya model at helper start. The warmer answers at once and runs its work on its own
// background-QoS queue (never the caller's thread, never ctrl's request queue), de-duplicates a path in flight,
// refuses a path without the model and its tokenizer, and the tokenizer load path reads the file once. A question
// while warming answers `laya.warming` on macOS (CtrlBackendMac.swift, stub-typechecked). Pure; Linux and macOS.
import Foundation
import XCTest
@testable import QrtzCtrl

private let fixtures = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Fixtures/ctrl").standardized

private final class Box<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var v: T
    init(_ v: T) { self.v = v }
    var value: T { get { lock.lock(); defer { lock.unlock() }; return v } set { lock.lock(); v = newValue; lock.unlock() } }
}

/// A set dir like the installed one: `<dir>/laya.mlmodelc/` and `<dir>/tokenizer.json` (the mini vocabulary).
private func installedSet(tokenizer: Bool = true) throws -> (dir: URL, model: String) {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("ctrl-laya-warm-\(UUID().uuidString)")
    let model = dir.appendingPathComponent("laya_multilingual_e8_L512_options32.mlmodelc")
    try FileManager.default.createDirectory(at: model, withIntermediateDirectories: true)
    if tokenizer {
        try FileManager.default.copyItem(at: fixtures.appendingPathComponent("laya-tokenizer-mini.json"),
                                         to: dir.appendingPathComponent("tokenizer.json"))
    }
    return (dir, model.path)
}

final class CtrlLayaWarmTests: XCTestCase {
    func testWarmsInTheBackgroundOnceAndLoadsTheTokenizer() throws {
        let (dir, model) = try installedSet()
        defer { try? FileManager.default.removeItem(at: dir) }
        let cache = CtrlLayaTokenizerCache()
        let gate = DispatchSemaphore(value: 0)
        let ran = Box<[(path: String, main: Bool)]>([])
        let caller = Thread.current
        let warmer = CtrlLayaWarmer { path in
            gate.wait()
            _ = try? cache.tokenizer(at: CtrlLayaWarmer.tokenizerPath(for: path, env: [:]))
            ran.value.append((path, Thread.current == caller))
        }
        // Answers at once, before the work has run (it's held on the gate).
        XCTAssertEqual(try warmer.start(model, env: [:]), CtrlLayaWarmer.State.started)
        XCTAssertTrue(warmer.isWarming(model))
        XCTAssertEqual(try warmer.start(model, env: [:]), CtrlLayaWarmer.State.warming)
        XCTAssertTrue(ran.value.isEmpty)
        gate.signal()
        warmer.drain()
        XCTAssertFalse(warmer.isWarming(model))
        XCTAssertEqual(ran.value.count, 1)
        XCTAssertEqual(ran.value.first?.path, model)
        XCTAssertEqual(ran.value.first?.main, false)
        XCTAssertEqual(warmer.qos, .background)
        // The tokenizer the first question will use is already read and encodes.
        let tok = dir.appendingPathComponent("tokenizer.json").path
        XCTAssertTrue(cache.cached(tok))
        XCTAssertFalse(try cache.tokenizer(at: tok).encode("close all my tabs").isEmpty)
        // Asked again (a host that restarted, a second tick): nothing runs twice.
        XCTAssertEqual(try warmer.start(model, env: [:]), CtrlLayaWarmer.State.warm)
        warmer.drain()
        XCTAssertEqual(ran.value.count, 1)
    }

    func testRefusesWithoutTheModelOrItsTokenizerAndNeverRuns() throws {
        let ran = Box(0)
        let warmer = CtrlLayaWarmer { _ in ran.value += 1 }
        let (dir, model) = try installedSet(tokenizer: false)
        defer { try? FileManager.default.removeItem(at: dir) }
        for path in ["", "/nonexistent/laya.mlmodelc", dir.appendingPathComponent("tokenizer.json").path, model] {
            XCTAssertThrowsError(try warmer.start(path, env: [:])) { XCTAssertEqual($0 as? CtrlFailure, CtrlFailure("laya.no_model"), path) }
        }
        // A file that isn't a Core ML model kind, even with a tokenizer beside it.
        let (dir2, _) = try installedSet()
        defer { try? FileManager.default.removeItem(at: dir2) }
        XCTAssertThrowsError(try warmer.start(dir2.appendingPathComponent("tokenizer.json").path, env: [:]))
        // QRTZ_CTRL_LAYA_TOKENIZER (the developer override) is honoured.
        let override = ["QRTZ_CTRL_LAYA_TOKENIZER": fixtures.appendingPathComponent("laya-tokenizer-mini.json").path]
        XCTAssertEqual(try warmer.start(model, env: override), CtrlLayaWarmer.State.started)
        warmer.drain()
        XCTAssertEqual(ran.value, 1)
    }

    func testABadTokenizerFileIsNoTokenizerAndIsNotCached() throws {
        let (dir, _) = try installedSet(tokenizer: false)
        defer { try? FileManager.default.removeItem(at: dir) }
        let bad = dir.appendingPathComponent("tokenizer.json")
        try Data(#"{"model":{"type":"WordPiece"}}"#.utf8).write(to: bad)
        let cache = CtrlLayaTokenizerCache()
        XCTAssertThrowsError(try cache.tokenizer(at: bad.path)) { XCTAssertEqual($0 as? CtrlFailure, CtrlFailure("laya.no_tokenizer")) }
        XCTAssertFalse(cache.cached(bad.path))
    }

    func testTheWarmRequestRepliesWhileTheWorkIsStillRunning() throws {
        let (dir, model) = try installedSet()
        defer { try? FileManager.default.removeItem(at: dir) }
        let gate = DispatchSemaphore(value: 0)
        final class Warming: CtrlBackend {
            let warmer: CtrlLayaWarmer
            init(_ w: CtrlLayaWarmer) { warmer = w }
            func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], nil) }
            func tree(app: String, scope: String?, depth: Int) throws -> (root: CtrlTreeNode, focused: String?) { throw CtrlFailure("x") }
            func menuShortcuts(app: String) throws -> [CtrlMenuShortcut] { [] }
            func act(_ action: CtrlAction, sig: String?) throws {}
            func observe(arm: Bool?) -> CtrlObserved { CtrlObserved() }
            func laya(model: String?, state: String, question: String, options: [String]) throws -> [Double] { throw CtrlFailure("laya.no_model") }
            func warmLaya(model: String) throws -> CtrlLayaWarmer.State { try warmer.start(model, env: [:]) }
        }
        let warmer = CtrlLayaWarmer { _ in gate.wait() }
        let lines = Box<[String]>([])
        let io = CtrlStdio(backend: Warming(warmer), write: { l in lines.value.append(l) })
        io.submit(#"{"type":"ctrl.laya.warm","id":"w0","model":"\#(model)"}"#)
        io.submit(#"{"type":"ctrl.catalog","id":"c1"}"#)
        io.drain()
        // Both replies are out while the warm-up is still held: the request queue never waited on it.
        XCTAssertTrue(warmer.isWarming(model))
        XCTAssertEqual(lines.value.count, 2)
        XCTAssertTrue(lines.value[0].contains(#""state":"started""#), lines.value[0])
        XCTAssertTrue(lines.value[1].contains("ctrl.catalog.result"))
        gate.signal()
        warmer.drain()
        XCTAssertFalse(warmer.isWarming(model))
        // Off macOS there is nothing to warm, answered at once; a request without a model is a bad request.
        let off = CtrlServer(backend: UnsupportedCtrlBackend())
        XCTAssertTrue(off.handle(line: #"{"type":"ctrl.laya.warm","id":"w1","model":"\#(model)"}"#).contains("platform.unsupported"))
        XCTAssertTrue(off.handle(line: #"{"type":"ctrl.laya.warm","id":"w2"}"#).contains("helper.bad_request"))
    }
}
