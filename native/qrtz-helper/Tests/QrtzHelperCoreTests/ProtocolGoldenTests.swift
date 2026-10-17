import Foundation
import XCTest
@testable import QrtzHelperCore

/// Shared contract with packages/core/src/helper/protocol-golden.test.ts (zod validates the same files).
final class ProtocolGoldenTests: XCTestCase {
    static let dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Fixtures/protocol").standardized

    func lines(_ f: String) throws -> [String] {
        try String(contentsOf: Self.dir.appendingPathComponent(f), encoding: .utf8).split(separator: "\n").map(String.init).filter { !$0.isEmpty }
    }

    func testHelperToHostRoundTripsByteForByte() throws {
        let ls = try lines("helper-to-host.jsonl")
        XCTAssertGreaterThan(ls.count, 15)
        for l in ls {
            let m = try HelperMessage(json: try JSONValue.parse(l))
            XCTAssertEqual(m.line, l)
        }
    }

    func testHostToHelperRoundTrips() throws {
        for l in try lines("host-to-helper.jsonl") {
            guard case .success(let m) = HostMessage.parse(line: l) else { return XCTFail("parse \(l)") }
            XCTAssertEqual(m.json.serialized(), l)
        }
    }

    func testInvalidRejected() throws {
        for l in try lines("invalid-helper-to-host.jsonl") {
            XCTAssertThrowsError(try HelperMessage(json: try JSONValue.parse(l)), l)
        }
    }

    func testUnknownHostMessageIsAFailureNotACrash() {
        guard case .failure = HostMessage.parse(line: #"{"type":"future.thing"}"#) else { return XCTFail() }
        guard case .failure = HostMessage.parse(line: "garbage") else { return XCTFail() }
    }

    func testLineSplitterBuffersPartialLines() {
        var s = LineSplitter()
        XCTAssertEqual(s.feed(Data("{\"a\":".utf8)), [])
        XCTAssertEqual(s.feed(Data("1}\n{\"b\"".utf8)), ["{\"a\":1}"])
        XCTAssertEqual(s.feed(Data(":2}\n".utf8)), ["{\"b\":2}"])
    }

    func testCaptureDegradedReasonIsCappedOnEncode() throws {
        // N35: a long error text is cut to 200 UTF-16 units on whole scalars, so the line still validates.
        let long = String(repeating: "é", count: 150) + String(repeating: "🎤", count: 60)
        let l = HelperMessage.captureDegraded(degraded: [.noMic], reason: long).line
        guard case .captureDegraded(let d, let r) = try HelperMessage(json: try JSONValue.parse(l)) else { return XCTFail(l) }
        XCTAssertEqual(d, [.noMic])
        XCTAssertLessThanOrEqual(r.utf16.count, 200)
        XCTAssertEqual(r.utf16.count, 150 + 2 * 25)
        XCTAssertTrue(long.hasPrefix(r))
    }

    func testPermissionsStatusAlwaysExhaustive() {
        let l = HelperMessage.permissionsStatus([.accessibility: .granted]).line
        XCTAssertEqual(l, #"{"status":{"accessibility":"granted","microphone":"not-determined","screen-recording":"not-determined"},"type":"permissions.status"}"#)
    }
}
