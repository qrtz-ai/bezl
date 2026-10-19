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
}
