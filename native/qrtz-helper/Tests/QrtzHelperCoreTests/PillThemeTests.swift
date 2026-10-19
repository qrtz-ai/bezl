import Foundation
import XCTest
@testable import QrtzEcho

/// ADR 0211: the pill's theme comes from the host's theme file (qrtz's own setting), never guessed.
final class PillThemeTests: XCTestCase {
    private func tmpFile(_ contents: String?) throws -> String {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("pill-theme-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: dir) }
        let f = dir.appendingPathComponent("theme")
        if let contents { try Data(contents.utf8).write(to: f) }
        return f.path
    }

    func testParsesTheThreeSettingsOnly() {
        XCTAssertEqual(PillTheme.parse("dark"), .dark)
        XCTAssertEqual(PillTheme.parse("light\n"), .light)
        XCTAssertEqual(PillTheme.parse(" system "), .system)
        for bad in ["", "Dark", "auto", "dark light", "darkAqua"] { XCTAssertNil(PillTheme.parse(bad), bad) }
    }

    func testReadsTheHostsFile() throws {
        XCTAssertEqual(PillTheme.read(path: try tmpFile("light")), .light)
        XCTAssertEqual(PillTheme.read(path: try tmpFile("system\n")), .system)
        XCTAssertNil(PillTheme.read(path: try tmpFile(nil)), "missing")
        XCTAssertNil(PillTheme.read(path: try tmpFile("nope")), "not a theme")
        XCTAssertNil(PillTheme.read(path: try tmpFile(String(repeating: " ", count: 20) + "dark")), "too long")
    }

    func testTheEnvironmentNamesTheFile() throws {
        let f = try tmpFile("dark")
        XCTAssertEqual(PillTheme.fromEnvironment([PillTheme.fileEnv: f]), .dark)
        XCTAssertEqual(PillTheme.file([PillTheme.fileEnv: f]), f)
        // No file named (an older host): no theme, the pill follows macOS as before.
        XCTAssertNil(PillTheme.fromEnvironment([:]))
        XCTAssertNil(PillTheme.file([PillTheme.fileEnv: ""]))
    }

    /// The host (crates/qrtz-app pill_theme.rs) writes exactly these words.
    func testTheHostsWordsAreTheSettingsValues() {
        XCTAssertEqual(PillTheme.allCases.map(\.rawValue), ["dark", "light", "system"])
    }

    func testASymlinkIsNeverFollowed() throws {
        let target = try tmpFile("light")
        let link = (target as NSString).deletingLastPathComponent + "/link"
        try FileManager.default.createSymbolicLink(atPath: link, withDestinationPath: target)
        XCTAssertNil(PillTheme.read(path: link), "symlink")
        XCTAssertEqual(PillTheme.read(path: target), .light)
    }

    func testAFifoIsRefusedWithoutBlocking() throws {
        let f = try tmpFile(nil)
        guard mkfifo(f, 0o600) == 0 else { throw XCTSkip("mkfifo unavailable") }
        XCTAssertNil(PillTheme.read(path: f), "fifo")
    }

    func testADirectoryIsNotATheme() throws {
        XCTAssertNil(PillTheme.read(path: (try tmpFile(nil) as NSString).deletingLastPathComponent))
    }
}
