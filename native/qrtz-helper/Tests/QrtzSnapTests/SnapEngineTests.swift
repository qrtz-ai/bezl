import XCTest
@testable import QrtzSnap

/// A window system with no windows of its own: a fake frontmost window, optionally with a minimum size or a grid.
final class FakeWindows: SnapWindowSystem {
    typealias Element = Int
    var app = "com.example.editor"
    var rects: [Int: SnapRect] = [1: SnapRect(x: 100, y: 125, width: 600, height: 400)]
    var focused: Int? = 1
    var sheet = false
    var minWidth = 0.0
    var grid = 1.0
    var sets: [SnapRect] = []
    var screens: [SnapScreen] = [SnapScreen(frame: SnapRect(x: 0, y: 0, width: 1440, height: 900),
                                            visibleFrame: SnapRect(x: 0, y: 0, width: 1440, height: 875))]
    var mainScreen: SnapScreen? { screens.first }
    var primaryHeight: Double { 900 }

    func frontmost() -> SnapFrontmost<Int>? { focused.map { SnapFrontmost(bundleId: app, window: $0) } }
    func rect(of e: Int) -> SnapRect? { rects[e] }
    func isSheetOrDialog(_ e: Int) -> Bool { sheet }
    func setRect(_ r: SnapRect, of e: Int) {
        sets.append(r)
        var x = r
        x.width = max(minWidth, (r.width / grid).rounded(.down) * grid)
        rects[e] = x
    }
}

final class SnapEngineTests: XCTestCase {
    func r(_ x: Double, _ y: Double, _ w: Double, _ h: Double) -> SnapRect { SnapRect(x: x, y: y, width: w, height: h) }

    func testLeftHalfMovesInAXSpace() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        // Cocoa (0, 0, 720, 875) on a 900-tall primary with a 25pt menu bar is AX (0, 25, 720, 875).
        XCTAssertEqual(e.perform(.leftHalf), .moved(r(0, 25, 720, 875)))
        XCTAssertEqual(f.rects[1], r(0, 25, 720, 875))
    }

    func testPressingAgainCyclesThroughTheEngine() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        _ = e.perform(.leftHalf)
        XCTAssertEqual(e.perform(.leftHalf), .moved(r(0, 25, 960, 875)))
        XCTAssertEqual(e.perform(.leftHalf), .moved(r(0, 25, 480, 875)))
    }

    func testUndoAndRedoWalkTheAppsHistory() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        let original = f.rects[1]!
        _ = e.perform(.leftHalf)
        _ = e.perform(.fullscreen)
        XCTAssertEqual(e.perform(.undo), .moved(r(0, 25, 720, 875)))
        XCTAssertEqual(e.perform(.undo), .moved(original))
        XCTAssertEqual(e.perform(.undo), .failed("nothing to undo or redo"))
        XCTAssertEqual(e.perform(.redo), .moved(r(0, 25, 720, 875)))
        XCTAssertEqual(e.perform(.redo), .moved(r(0, 25, 1440, 875)))
        XCTAssertEqual(e.perform(.redo), .failed("nothing to undo or redo"))
    }

    func testANewMoveAfterUndoDropsTheRedoChain() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        _ = e.perform(.leftHalf)
        _ = e.perform(.fullscreen)
        _ = e.perform(.undo)
        _ = e.perform(.rightHalf)
        XCTAssertEqual(e.perform(.redo), .failed("nothing to undo or redo"))
    }

    func testHistoriesArePerApp() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        _ = e.perform(.leftHalf)
        f.app = "com.example.other"
        XCTAssertEqual(e.perform(.undo), .failed("nothing to undo or redo"))
    }

    func testHistoryKeepsFiftyItems() {
        let h = SnapHistory<Int>()
        for i in 0..<60 { h.add(element: i, rect: r(Double(i), 0, 1, 1)) }
        XCTAssertEqual(h.count, 50)
        var steps = 0
        while h.previous() != nil { steps += 1 }
        XCTAssertEqual(steps, 49)
    }

    func testNoMoveIsAFailure() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        _ = e.perform(.fullscreen)
        XCTAssertEqual(e.perform(.fullscreen), .failed("already there"))
        f.sheet = true
        XCTAssertEqual(e.perform(.leftHalf), .failed("sheets and dialogs don't move"))
        f.focused = nil
        XCTAssertEqual(e.perform(.leftHalf), .failed("no focused window"))
    }

    func testDisabledAndAlwaysDisabledAppsAreLeftAlone() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        e.disabledApps = ["com.example.editor"]
        XCTAssertEqual(e.perform(.leftHalf), .failed("snap is off for com.example.editor"))
        f.app = "com.adobe.Photoshop"
        e.disabledApps = []
        XCTAssertEqual(e.perform(.leftHalf), .failed("snap is off for com.adobe.Photoshop"))
        XCTAssertTrue(f.sets.isEmpty)
    }

    func testOneDisplayCantMoveToTheNext() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        XCTAssertEqual(e.perform(.nextDisplay), .failed("only one display"))
    }

    func testNextAndPreviousDisplayWalkTheConsistentOrderAndWrap() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        let right = SnapScreen(frame: r(1440, 0, 1920, 1080), visibleFrame: r(1440, 0, 1920, 1080))
        let left = SnapScreen(frame: r(-1920, 100, 1920, 1080), visibleFrame: r(-1920, 100, 1920, 1055))
        f.screens.append(contentsOf: [left, right])
        XCTAssertEqual(SnapScreens.consistentOrder(f.screens).map(\.frame.x), [0, 1440, -1920])
        // Cocoa center of (600x400) in the right display: (2100, 340) → AX y = 900 - 740 = 160.
        XCTAssertEqual(e.perform(.nextDisplay), .moved(r(2100, 160, 600, 400)))
        // From the right display, next wraps on to the left one: Cocoa (-1260, 428) → AX y = 72.
        XCTAssertEqual(e.perform(.nextDisplay), .moved(r(-1260, 72, 600, 400)))
        // And previous goes back to the right one.
        XCTAssertEqual(e.perform(.previousDisplay), .moved(r(2100, 160, 600, 400)))
    }

    func testTheDisplayHoldingMostOfTheWindowIsTheSource() {
        let main = SnapScreen(frame: r(0, 0, 1440, 900), visibleFrame: r(0, 0, 1440, 875))
        let right = SnapScreen(frame: r(1440, 0, 1920, 1080), visibleFrame: r(1440, 0, 1920, 1080))
        // AX (1300, 100, 600, 400): 140pt on main, 460pt on the right.
        let s = SnapScreens.containing(r(1300, 100, 600, 400), screens: [main, right], main: main, primaryHeight: 900)
        XCTAssertEqual(s, right)
        let whole = SnapScreens.containing(r(1500, 100, 600, 400), screens: [main, right], main: main, primaryHeight: 900)
        XCTAssertEqual(whole, right)
        let nowhere = SnapScreens.containing(r(9000, 9000, 10, 10), screens: [main, right], main: main, primaryHeight: 900)
        XCTAssertEqual(nowhere, main)
    }

    func testAMinimumWidthWindowIsPushedBackOnScreen() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        f.minWidth = 900
        _ = e.perform(.rightHalf)
        // Asked for x 720 width 720; the app kept 900, so best effort slides it left to stay inside.
        XCTAssertEqual(f.rects[1]?.maxX, 1440)
        XCTAssertEqual(f.rects[1]?.width, 900)
    }

    func testAGridWindowIsCenteredOnTheAskedFrame() {
        let f = FakeWindows(), e = SnapEngine(system: f)
        f.grid = 7 // a terminal that sizes to whole 7pt columns
        _ = e.perform(.leftHalf)
        let got = f.rects[1]!
        XCTAssertLessThanOrEqual(got.width, 720)
        XCTAssertEqual(got.x, ((720 - got.width) / 2).rounded(.down))
    }
}
