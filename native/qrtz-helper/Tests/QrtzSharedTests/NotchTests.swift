import Foundation
import XCTest
@testable import QrtzShared

/// ADR 0218: the notch surface's platform-free part (the shape, the geometry, the arbiter, the wire).
final class NotchTests: XCTestCase {
    func testShapeHasAFlatTopRoundedBottomAndConcaveEars() {
        let ops = NotchShape.path(width: 200, height: 40, radius: 12, ear: 8)
        XCTAssertEqual(ops.count, 10, "the same steps every time, so Core Animation can morph rest <-> expanded")
        XCTAssertEqual(ops[0], .move(0, 0))
        XCTAssertEqual(ops[1], .quad(8, 0, 8, 8), "the left ear curves from the top edge into the side")
        XCTAssertEqual(ops[3], .quad(8, 40, 20, 40), "the bottom-left corner")
        XCTAssertEqual(ops[7], .quad(192, 0, 200, 0), "the right ear back to the top edge")
        XCTAssertEqual(ops[8], .line(0, 0), "the top edge is flat")
        XCTAssertEqual(ops[9], .close)
    }

    func testShapeClampsAtRestAndAtZeroHeight() {
        // A notch-sized rest rect: no ears, the radius capped by the height.
        XCTAssertEqual(NotchShape.path(width: 185, height: 32, radius: 8, ear: 0)[3], .quad(0, 32, 8, 32))
        // The fake tab's rest: zero height, still 10 steps, every point on the top edge.
        let flat = NotchShape.path(width: 180, height: 0, radius: 8, ear: 8)
        XCTAssertEqual(flat.count, 10)
        for op in flat {
            switch op {
            case .move(_, let y), .line(_, let y): XCTAssertEqual(y, 0)
            case .quad(_, let cy, _, let y): XCTAssertEqual(cy, 0); XCTAssertEqual(y, 0)
            case .close: break
            }
        }
        XCTAssertEqual(NotchShape.clamp(width: 10, height: 100, radius: 50, ear: 8).ear, 2.5)
    }

    func testNotchDetection() {
        // M1 Pro 14": 1512 wide, a 32 pt safe top, the notch between the two auxiliary areas.
        let mbp = NotchScreen(frameX: 0, frameMaxY: 982, frameWidth: 1512, safeTop: 32, leftMaxX: 662, rightMinX: 850)
        XCTAssertEqual(mbp.notch?.width, 188)
        XCTAssertEqual(mbp.rest.real, true)
        XCTAssertEqual(mbp.rest.height, 32)
        XCTAssertEqual(mbp.contentTop, 32)
        let p = mbp.panelFrame
        XCTAssertEqual(p.x + p.w / 2, 756, "centered on the notch")
        XCTAssertEqual(p.y + p.h, 982, "flush with the top edge")
        // An external display: no notch, a fake tab that rests at zero height.
        let ext = NotchScreen(frameX: 1512, frameMaxY: 1440, frameWidth: 2560, safeTop: 0, leftMaxX: nil, rightMinX: nil)
        XCTAssertNil(ext.notch)
        XCTAssertEqual(ext.rest.width, 180); XCTAssertEqual(ext.rest.height, 0); XCTAssertEqual(ext.rest.real, false)
        XCTAssertEqual(ext.rest.minX, 1512 + 1190)
        XCTAssertEqual(ext.contentTop, 24)
        // A safe top without the auxiliary areas is not a notch.
        XCTAssertNil(NotchScreen(frameX: 0, frameMaxY: 900, frameWidth: 1440, safeTop: 24, leftMaxX: nil, rightMinX: nil).notch)
    }

    func testExpandedSizeAndWidening() {
        let e = NotchGeometry.expanded(notchWidth: 188, notchHeight: 32)
        XCTAssertEqual(e.w, 188 + 140 + 16); XCTAssertEqual(e.h, 64)
        XCTAssertEqual(NotchGeometry.extra(notchWidth: 188, contentWidth: 160), 0)
        XCTAssertEqual(NotchGeometry.extra(notchWidth: 188, contentWidth: 10_000), NotchGeometry.widenMax)
        let wide = NotchGeometry.expanded(notchWidth: 188, notchHeight: 32, extra: 1_000)
        let panel = NotchGeometry.panelSize(notchWidth: 188, notchHeight: 32)
        XCTAssertLessThanOrEqual(wide.w, panel.w, "the fixed panel holds the widest state")
        XCTAssertLessThan(wide.h, panel.h)
    }

    func testArbiterPriorityAndOwnership() {
        var a = NotchArbiter()
        XCTAssertTrue(a.show(NotchEntry(owner: "module.nano", priority: .module, content: .status(text: "x", tone: .normal), source: "nano")))
        XCTAssertTrue(a.show(NotchEntry(owner: "echo.dictation", priority: .system, content: .level(busy: false))))
        XCTAssertEqual(a.visible?.owner, "echo.dictation", "dictation is drawn over module content")
        XCTAssertFalse(a.update(owner: "module.other", content: .level(busy: true)), "only a holder updates")
        XCTAssertFalse(a.dismiss(owner: "module.other"))
        XCTAssertTrue(a.dismiss(owner: "echo.dictation"))
        XCTAssertEqual(a.visible?.owner, "module.nano")
        XCTAssertFalse(a.show(NotchEntry(owner: "module.x", priority: .module, content: .level(busy: false))), "module content needs its source")
    }

    func testWireRoundTripsAndRefusesBadContent() throws {
        let line = #"{"content":{"kind":"progress","label":"downloading","value":0.5},"owner":"module.nano","priority":"module","source":"nano","type":"notch.show"}"#
        let m = try HostMessage.parse(line: line).get()
        XCTAssertEqual(m, .notchShow(owner: "module.nano", priority: .module, content: .progress(label: "downloading", value: 0.5), source: "nano"))
        XCTAssertEqual(m.json.serialized(), line)
        for bad in [
            #"{"content":{"kind":"status","text":"x","tone":"normal"},"owner":"Bad Owner","priority":"module","type":"notch.show"}"#,
            #"{"content":{"kind":"progress","label":"x","value":2},"owner":"m","priority":"module","type":"notch.show"}"#,
            #"{"content":{"icon":"skull","kind":"icon","label":"x"},"owner":"m","type":"notch.update"}"#,
            #"{"content":{"kind":"action","label":"x"},"owner":"m","type":"notch.update"}"#,
            #"{"content":{"kind":"status","text":"","tone":"normal"},"owner":"m","type":"notch.update"}"#,
            #"{"content":{"kind":"status","text":"x","tone":"normal"},"owner":"m","priority":"root","type":"notch.show"}"#,
        ] {
            XCTAssertThrowsError(try HostMessage.parse(line: bad).get(), bad)
        }
        let long = String(repeating: "a", count: NotchLimits.labelMax + 1)
        XCTAssertThrowsError(try HostMessage.parse(line: #"{"content":{"icon":"check","kind":"icon","label":"\#(long)"},"owner":"m","type":"notch.update"}"#).get())
    }
}
