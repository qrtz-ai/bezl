import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// Synthetic AX trees (Fixtures/ax/*.json, same shape as `qrtz-helper --dump-ax`) covering layout variance:
/// Zoom app gallery/speaker ± captions, Zoom web, Meet in Chrome and Safari (grid, presenting, captions on/off,
/// de/fr locales). Each file carries the expected scrape. Replace or extend with real dumps on a Mac. [MAC-VERIFY]
final class AXFixtureTests: XCTestCase {
    static let dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Fixtures/ax")

    static func node(_ v: JSONValue) -> AXNode {
        guard case let .object(o) = v else { return AXNode(role: "AXUnknown") }
        func s(_ k: String) -> String? { if case let .string(x)? = o[k] { return x }; return nil }
        var kids: [AXNode] = []
        if case let .array(a)? = o["children"] { kids = a.map(node) }
        return AXNode(role: s("role") ?? "AXUnknown", subrole: s("subrole"), title: s("title"), value: s("value"), desc: s("desc"), identifier: s("identifier"), children: kids)
    }

    static func str(_ v: JSONValue?) -> String? { if case let .string(x)? = v { return x }; return nil }
    static func strs(_ v: JSONValue?) -> [String]? {
        guard case let .array(a)? = v else { return nil }
        return a.compactMap { str($0) }
    }

    func testAllFixtures() throws {
        let files = try FileManager.default.contentsOfDirectory(at: Self.dir, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "json" }.sorted { $0.lastPathComponent < $1.lastPathComponent }
        XCTAssertGreaterThanOrEqual(files.count, 11)
        for f in files {
            let name = f.lastPathComponent
            let data = try Data(contentsOf: f)
            let root = try JSONDecoder().decode(JSONValue.self, from: data)
            guard case let .object(o) = root, case let .object(e)? = o["expect"], let tree = o["tree"],
                  let platform = MeetingPlatform(rawValue: Self.str(o["platform"]) ?? "") else { XCTFail("\(name): bad fixture"); continue }
            let s = AXScraper.scrape(platform: platform, window: Self.node(tree))
            XCTAssertEqual(s.participants, Self.strs(e["participants"]), "\(name) participants")
            XCTAssertEqual(s.activeSpeaker, Self.str(e["activeSpeaker"]), "\(name) activeSpeaker")
            XCTAssertEqual(s.selfName, Self.str(e["selfName"]), "\(name) selfName")
            var caps: [String] = []
            if case let .array(a)? = e["captions"] {
                for c in a { if let p = Self.strs(c) { caps.append(p.joined(separator: " | ")) } }
            }
            XCTAssertEqual(s.captions.map { "\($0.speaker ?? "") | \($0.text)" }, caps, "\(name) captions")
        }
    }
}
