// ADR 0157 §9 (run 36): a model path from the host is the read-only store package, opened as is.
import Foundation
import XCTest
@testable import QrtzCtrl

final class CtrlLayaLoadTests: XCTestCase {
    private var dir: URL!
    private let fm = FileManager.default

    override func setUpWithError() throws {
        dir = fm.temporaryDirectory.appendingPathComponent("ctrl-laya-load-\(UUID().uuidString)")
        try fm.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        // Give the owner write back so the temp dir can be removed.
        if let walk = fm.enumerator(atPath: dir.path) {
            for case let rel as String in walk {
                try? fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: dir.appendingPathComponent(rel).path)
            }
        }
        try? fm.removeItem(at: dir)
    }

    /// A store-like `.mlmodelc`: a nested file tree, every file at `mode`.
    private func model(_ name: String, mode: Int) throws -> String {
        let m = dir.appendingPathComponent(name)
        try fm.createDirectory(at: m.appendingPathComponent("analytics"), withIntermediateDirectories: true)
        for f in ["coremldata.bin", "model.mil", "analytics/coremldata.bin"] {
            let u = m.appendingPathComponent(f)
            try Data("x".utf8).write(to: u)
            try fm.setAttributes([.posixPermissions: mode], ofItemAtPath: u.path)
        }
        return m.path
    }

    func testTheStorePackagesCompiledModelLoadsAsIs() throws {
        let p = try model("laya_multilingual_e8_L512_options32.mlmodelc", mode: 0o444)
        XCTAssertEqual(CtrlLayaLoad.plan(path: p, fromHost: true), .asIs(p))
        let q = try model("owner-only.mlmodelc", mode: 0o400)
        XCTAssertEqual(CtrlLayaLoad.plan(path: q, fromHost: true), .asIs(q))
    }

    func testAWritableFileFromTheHostIsRefused() throws {
        let p = try model("laya.mlmodelc", mode: 0o444)
        try fm.setAttributes([.posixPermissions: 0o644], ofItemAtPath: p + "/model.mil")
        XCTAssertEqual(CtrlLayaLoad.plan(path: p, fromHost: true), .refuse("laya.not_read_only"))
        let w = try model("group-writable.mlmodelc", mode: 0o464)
        XCTAssertEqual(CtrlLayaLoad.plan(path: w, fromHost: true), .refuse("laya.not_read_only"))
    }

    func testASymlinkInsideTheModelIsRefused() throws {
        let p = try model("linked.mlmodelc", mode: 0o444)
        let outside = dir.appendingPathComponent("outside.bin")
        try Data("y".utf8).write(to: outside)
        try fm.setAttributes([.posixPermissions: 0o444], ofItemAtPath: outside.path)
        try fm.createSymbolicLink(atPath: p + "/weights.bin", withDestinationPath: outside.path)
        XCTAssertEqual(CtrlLayaLoad.plan(path: p, fromHost: true), .refuse("laya.not_read_only"))
    }

    func testAnEmptyModelDirIsRefused() throws {
        let p = dir.appendingPathComponent("empty.mlmodelc").path
        try fm.createDirectory(atPath: p, withIntermediateDirectories: true)
        XCTAssertEqual(CtrlLayaLoad.plan(path: p, fromHost: true), .refuse("laya.not_read_only"))
    }

    func testTheHostNeverGetsACompileIntoTheStore() throws {
        let pkg = try model("laya.mlpackage", mode: 0o444)
        XCTAssertEqual(CtrlLayaLoad.plan(path: pkg, fromHost: true), .refuse("laya.needs_compiled"))
        let file = dir.appendingPathComponent("laya.mlmodel")
        try Data("x".utf8).write(to: file)
        try fm.setAttributes([.posixPermissions: 0o444], ofItemAtPath: file.path)
        XCTAssertEqual(CtrlLayaLoad.plan(path: file.path, fromHost: true), .refuse("laya.needs_compiled"))
    }

    func testOnlyTheDeveloperOverrideMayCompile() throws {
        let pkg = try model("dev.mlpackage", mode: 0o644)
        XCTAssertEqual(CtrlLayaLoad.plan(path: pkg, fromHost: false), .compile(pkg))
        let c = try model("dev.mlmodelc", mode: 0o644)
        XCTAssertEqual(CtrlLayaLoad.plan(path: c, fromHost: false), .asIs(c))
    }

    func testMissingOrForeignPathsAreNoModel() {
        XCTAssertEqual(CtrlLayaLoad.plan(path: "", fromHost: true), .refuse("laya.no_model"))
        XCTAssertEqual(CtrlLayaLoad.plan(path: dir.appendingPathComponent("gone.mlmodelc").path, fromHost: true),
                       .refuse("laya.no_model"))
        XCTAssertEqual(CtrlLayaLoad.plan(path: "/etc/passwd", fromHost: true), .refuse("laya.no_model"))
        XCTAssertEqual(CtrlLayaLoad.plan(path: "/etc/passwd", fromHost: false), .refuse("laya.no_model"))
    }
    // Run 45: the layout of `model.laya` 1.0.0 in the live signed index (Fixtures/ctrl/laya-live-layout.json, the
    // same list crates/qrtz-app/tests/ctrl_live_index.rs checks against the index snapshot), linked 0444 like the
    // store does: the host's path is opened as is, and the tokenizer is the one beside it.
    func testTheLiveModelLayaLayoutLoadsAsIsWithItsTokenizerBeside() throws {
        struct Layout: Decodable { struct F: Decodable { let path: String }; let entry: String; let tokenizer: String; let files: [F] }
        let fixture = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .appendingPathComponent("../../Fixtures/ctrl/laya-live-layout.json").standardized
        let l = try JSONDecoder().decode(Layout.self, from: Data(contentsOf: fixture))
        XCTAssertEqual(l.files.count, 6)
        let pkg = dir.appendingPathComponent("packages/model.laya/1.0.0")
        for f in l.files {
            let u = pkg.appendingPathComponent(f.path)
            try fm.createDirectory(at: u.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data("stand-in for \(f.path)".utf8).write(to: u)
            try fm.setAttributes([.posixPermissions: 0o444], ofItemAtPath: u.path)
        }
        let model = pkg.appendingPathComponent(l.entry).path
        XCTAssertEqual(CtrlLayaLoad.plan(path: model, fromHost: true), .asIs(model))
        XCTAssertEqual(CtrlLayaWarmer.tokenizerPath(for: model, env: [:]), pkg.appendingPathComponent(l.tokenizer).path)
        XCTAssertTrue(CtrlLayaWarmer.eligible(model, env: [:]))
        // The weights alone changed in place: refused, the heuristic answers.
        try fm.setAttributes([.posixPermissions: 0o644], ofItemAtPath: model + "/weights/weight.bin")
        XCTAssertEqual(CtrlLayaLoad.plan(path: model, fromHost: true), .refuse("laya.not_read_only"))
    }

    /// The real install (`ctrl_live_index.rs -- --ignored` with QRTZ_CTRL_LIVE_DIR): QRTZ_CTRL_LAYA_PACKAGE = the
    /// installed `.mlmodelc`. Read-only checks only; nothing is loaded (no Core ML on Linux).
    func testARealInstalledModelLayaLoadsAsIsWhenGiven() throws {
        guard let p = ProcessInfo.processInfo.environment["QRTZ_CTRL_LAYA_PACKAGE"], !p.isEmpty else { throw XCTSkip("no package") }
        XCTAssertEqual(CtrlLayaLoad.plan(path: p, fromHost: true), .asIs(p))
        XCTAssertTrue(CtrlLayaWarmer.eligible(p, env: [:]))
    }
}
