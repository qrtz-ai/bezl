import XCTest
@testable import QrtzShared

final class NotchMotionTests: XCTestCase {
    func testDefaultsAreTheSnappySpring() {
        let m = NotchMotion()
        XCTAssertEqual(m.expandDuration, 0.3); XCTAssertEqual(m.expandBounce, 0.15)
        XCTAssertEqual(m.collapseDuration, 0.3); XCTAssertEqual(m.collapseBounce, 0)
    }

    func testJsonClampsAndKeepsDefaultsForMissingOrBadValues() {
        let m = NotchMotion(json: ["expandBounce": .number(2), "shoulder": .number(-1), "fadeInDelay": .string("x"), "other": .number(1)])
        XCTAssertEqual(m.expandBounce, 0.35)
        XCTAssertEqual(m.shoulder, 0)
        XCTAssertEqual(m.fadeInDelay, 0.11)
    }

    func testConfigureCarriesPillMotionRoundTrip() throws {
        var m = NotchMotion(); m.expandDuration = 0.5
        let msg = HostMessage.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "", pillMotion: m)
        guard case .object(let o) = msg.json, let pm = o["pillMotion"]?.object else { return XCTFail("no pillMotion") }
        XCTAssertEqual(NotchMotion(json: pm), m)
    }
}
