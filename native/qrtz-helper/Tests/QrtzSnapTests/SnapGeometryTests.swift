import XCTest
@testable import QrtzSnap

/// ADR 0159: every Spectacle action and cycle, against hand-computed frames (Cocoa space). The display's visible
/// frame has an odd height so Spectacle's `height % 2` corrections show.
final class SnapGeometryTests: XCTestCase {
    let v = SnapRect(x: 0, y: 0, width: 1440, height: 875)
    let loose = SnapRect(x: 100, y: 100, width: 600, height: 400)

    func r(_ x: Double, _ y: Double, _ w: Double, _ h: Double) -> SnapRect { SnapRect(x: x, y: y, width: w, height: h) }
    func calc(_ a: SnapAction, _ w: SnapRect, dest: SnapRect? = nil) -> SnapRect? {
        SnapGeometry.calculate(a, window: w, source: v, destination: dest ?? v)
    }
    /// Presses `a` repeatedly from `start` and returns each frame.
    func presses(_ a: SnapAction, from start: SnapRect, _ n: Int) -> [SnapRect] {
        var w = start, out: [SnapRect] = []
        for _ in 0..<n { w = calc(a, w)!; out.append(w) }
        return out
    }

    func testCenterRoundsHalvesUpLikeJavaScript() {
        XCTAssertEqual(calc(.center, r(10, 10, 501, 400)), r(470, 238, 501, 400))
        XCTAssertEqual(calc(.center, loose), r(420, 238, 600, 400))
        let off = r(1440, -200, 1920, 1055)
        XCTAssertEqual(calc(.center, loose, dest: off), r(2100, 128, 600, 400))
    }

    func testFullscreenIsTheVisibleFrame() {
        XCTAssertEqual(calc(.fullscreen, loose), v)
    }

    func testLeftHalfCyclesHalfTwoThirdsOneThird() {
        XCTAssertEqual(presses(.leftHalf, from: loose, 4),
                       [r(0, 0, 720, 875), r(0, 0, 960, 875), r(0, 0, 480, 875), r(0, 0, 720, 875)])
    }

    func testRightHalfCyclesAnchoredRight() {
        XCTAssertEqual(presses(.rightHalf, from: loose, 4),
                       [r(720, 0, 720, 875), r(480, 0, 960, 875), r(960, 0, 480, 875), r(720, 0, 720, 875)])
    }

    func testTopHalfCyclesHeightsAnchoredTopWithTheOddPoint() {
        XCTAssertEqual(presses(.topHalf, from: loose, 4),
                       [r(0, 438, 1440, 437), r(0, 292, 1440, 583), r(0, 584, 1440, 291), r(0, 438, 1440, 437)])
    }

    func testBottomHalfCyclesHeights() {
        XCTAssertEqual(presses(.bottomHalf, from: loose, 4),
                       [r(0, 0, 1440, 437), r(0, 0, 1440, 583), r(0, 0, 1440, 291), r(0, 0, 1440, 437)])
    }

    func testCornersCycleWidths() {
        XCTAssertEqual(presses(.upperLeft, from: loose, 4),
                       [r(0, 438, 720, 437), r(0, 438, 960, 437), r(0, 438, 480, 437), r(0, 438, 720, 437)])
        XCTAssertEqual(presses(.lowerLeft, from: loose, 4),
                       [r(0, 0, 720, 437), r(0, 0, 960, 437), r(0, 0, 480, 437), r(0, 0, 720, 437)])
        XCTAssertEqual(presses(.upperRight, from: loose, 4),
                       [r(720, 438, 720, 437), r(480, 438, 960, 437), r(960, 438, 480, 437), r(720, 438, 720, 437)])
        XCTAssertEqual(presses(.lowerRight, from: loose, 4),
                       [r(720, 0, 720, 437), r(480, 0, 960, 437), r(960, 0, 480, 437), r(720, 0, 720, 437)])
    }

    func testAHalfOnTheOtherRowStartsAtTheHalfAgain() {
        // A window in the top half pressed "left half": not on the left column's row, so plain left half.
        XCTAssertEqual(calc(.leftHalf, r(0, 438, 1440, 437)), r(0, 0, 720, 875))
        // A lower-left quarter pressed "left half": different midY, so the half, not a cycle.
        XCTAssertEqual(calc(.leftHalf, r(0, 0, 720, 437)), r(0, 0, 720, 875))
    }

    func testCyclingWorksOnAnOffsetDisplay() {
        let off = r(1440, -200, 1920, 1055)
        var w = calc(.rightHalf, loose, dest: off)!
        XCTAssertEqual(w, r(2400, -200, 960, 1055))
        w = calc(.rightHalf, w, dest: off)!
        XCTAssertEqual(w, r(2080, -200, 1280, 1055))
        w = calc(.rightHalf, w, dest: off)!
        XCTAssertEqual(w, r(2720, -200, 640, 1055))
    }

    func testThirdsAreColumnsThenRows() {
        XCTAssertEqual(SnapGeometry.thirds(v), [
            r(0, 0, 480, 875), r(480, 0, 480, 875), r(960, 0, 480, 875),
            r(0, 584, 1440, 291), r(0, 293, 1440, 291), r(0, 2, 1440, 291),
        ])
    }

    func testNextThirdWalksAndWraps() {
        let t = SnapGeometry.thirds(v)
        XCTAssertEqual(calc(.nextThird, loose), t[0], "a loose window goes to the first column")
        for i in 0..<6 { XCTAssertEqual(calc(.nextThird, t[i]), t[(i + 1) % 6], "next from \(i)") }
    }

    func testPreviousThirdWalksBackAndWraps() {
        let t = SnapGeometry.thirds(v)
        XCTAssertEqual(calc(.previousThird, loose), t[0])
        for i in 0..<6 { XCTAssertEqual(calc(.previousThird, t[i]), t[(i + 5) % 6], "previous from \(i)") }
    }

    func testMakeLargerAndSmallerGrowAroundTheCenterBy30() {
        XCTAssertEqual(calc(.makeLarger, loose), r(85, 85, 630, 430))
        XCTAssertEqual(calc(.makeSmaller, loose), r(115, 115, 570, 370))
    }

    func testMakeLargerKeepsAnEdgeTheWindowTouches() {
        XCTAssertEqual(calc(.makeLarger, r(0, 100, 600, 400)), r(0, 85, 630, 430))
        XCTAssertEqual(calc(.makeLarger, r(840, 100, 600, 400)), r(810, 85, 630, 430))
        XCTAssertEqual(calc(.makeLarger, r(100, 475, 600, 400)), r(85, 445, 630, 430))
        XCTAssertEqual(calc(.makeLarger, r(100, 0, 600, 400)), r(85, 0, 630, 430))
    }

    func testMakeLargerStopsAtTheDisplay() {
        XCTAssertEqual(calc(.makeLarger, v), v, "fullscreen can't grow (the engine then beeps: no change)")
        XCTAssertEqual(calc(.makeLarger, r(0, 0, 1430, 870)).map(\.width), 1440)
    }

    func testMakeSmallerFromFullscreenShrinksAroundTheCenter() {
        XCTAssertEqual(calc(.makeSmaller, v), r(15, 15, 1410, 845))
    }

    func testMakeSmallerStopsAtAQuarterOfTheDisplay() {
        let small = r(100, 100, 370, 300)
        XCTAssertEqual(calc(.makeSmaller, small), small, "340 ≤ 1440/4: Spectacle keeps the window")
    }

    func testNextDisplayCentersAWindowThatFitsAndFillsOneThatDoesnt() {
        let other = r(1440, 0, 1920, 1055)
        XCTAssertEqual(calc(.nextDisplay, loose, dest: other), r(2100, 328, 600, 400))
        XCTAssertEqual(calc(.previousDisplay, r(0, 0, 2000, 900), dest: other), other)
    }

    func testUndoAndRedoHaveNoFrameMath() {
        XCTAssertNil(calc(.undo, loose))
        XCTAssertNil(calc(.redo, loose))
    }

    func testEveryActionButHistoryHasACalculation() {
        for a in SnapAction.allCases where !a.isHistory { XCTAssertNotNil(calc(a, loose), a.rawValue) }
    }

    func testFlipIsItsOwnInverse() {
        let ax = r(100, 30, 600, 400)
        XCTAssertEqual(ax.flipped(primaryHeight: 900), r(100, 470, 600, 400))
        XCTAssertEqual(ax.flipped(primaryHeight: 900).flipped(primaryHeight: 900), ax)
    }

    func testBestEffortPushesAnOversizedWindowBackInside() {
        // AX space, primary 900 tall, visible frame (0, 0, 1440, 875) in Cocoa space (menu bar on top).
        XCTAssertEqual(SnapGeometry.bestEffort(moved: r(1000, 25, 600, 400), visible: v, primaryHeight: 900),
                       r(840, 25, 600, 400))
        XCTAssertEqual(SnapGeometry.bestEffort(moved: r(-20, 600, 600, 400), visible: v, primaryHeight: 900),
                       r(0, 500, 600, 400))
        XCTAssertEqual(SnapGeometry.bestEffort(moved: r(-20, 10, 600, 400), visible: v, primaryHeight: 900),
                       r(0, 25, 600, 400))
    }

    func testQuantizedCentersAWindowThatStayedBigger() {
        XCTAssertEqual(SnapGeometry.quantizedOrigin(requested: r(0, 0, 720, 875), adjusted: r(0, 0, 716, 875),
                                                    moved: r(0, 0, 714, 875)),
                       r(3, 0, 716, 875))
    }
}
