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

    /// Security review: secure input hides the content at once even with every motion value at its max.
    func testSecureInputHideIsImmediateAtMaxValues() {
        var o: [String: JSONValue] = [:]
        for (k, _, hi) in NotchMotion.ranges { o[k] = .number(hi) }
        let m = NotchMotion(json: o)
        XCTAssertEqual(m.fadeOutDuration, 0.1, "fade-out capped at 0.1 s")
        XCTAssertEqual(m.hideContentFade(secure: true), 0)
        XCTAssertLessThanOrEqual(m.hideContentFade(secure: false), 0.1)
        var raw = NotchMotion(); raw.fadeOutDuration = 5  // set in code, past the wire clamp
        XCTAssertEqual(raw.hideContentFade(secure: true), 0)
        XCTAssertEqual(raw.hideContentFade(secure: false), 0.1)
    }

    /// The panel has room for the overshoot at the widest state and max bounce, each side and below.
    func testPanelHoldsMaxBounceOvershootAtWidenMax() {
        for (nw, nh, rest) in [(188.0, 32.0, 32.0), (NotchGeometry.fakeNotchWidth, NotchGeometry.fakeNotchHeight, 0.0)] {
            let panel = NotchGeometry.panelSize(notchWidth: nw, notchHeight: nh)
            let wide = NotchGeometry.expanded(notchWidth: nw, notchHeight: nh, extra: NotchGeometry.widenMax)
            let z = 1 - 0.35  // max bounce -> damping ratio
            let over = exp(-Double.pi * z / (1 - z * z).squareRoot())
            let side = (wide.w - nw) / 2
            XCTAssertLessThanOrEqual(wide.w + 2 * side * over, panel.w, "nw \(nw)")
            XCTAssertLessThanOrEqual(wide.h + (wide.h - rest) * over, panel.h, "nw \(nw)")
        }
    }

    /// path() keeps r <= h - e by itself for the fake tab at the max corner values, so no frame inverts.
    func testFakeTabClampAtMaxCorners() {
        let w = NotchGeometry.fakeNotchWidth + 2 * NotchGeometry.sideGrow + 2 * NotchGeometry.expandedEar
        for h in stride(from: 0.0, through: NotchGeometry.fakeNotchHeight + NotchGeometry.dropGrow, by: 1) {
            let c = NotchShape.clamp(width: w, height: h, radius: 32, ear: 16)
            XCTAssertLessThanOrEqual(c.radius, h - c.ear + 1e-9, "h \(h)")
            XCTAssertLessThanOrEqual(c.ear, h / 2 + 1e-9)
            XCTAssertGreaterThanOrEqual(c.radius, 0)
        }
    }
}
