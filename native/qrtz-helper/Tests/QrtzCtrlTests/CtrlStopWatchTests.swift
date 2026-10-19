// ctrl do's stop key (ADR 0214): Esc stops, any other physical input stops too, ctrl's own posted events are
// ignored, nothing counts before the watcher is armed, and a tap the system turned off stops the run.
import Foundation
import XCTest
@testable import QrtzCtrl

final class CtrlStopWatchTests: XCTestCase {
    func testEscStopsAndOtherInputTakesOver() {
        let w = CtrlStopWatch()
        w.record(.keyDown(code: CtrlKeys.escape, marker: 0))
        XCTAssertFalse(w.flags.stopped, "not armed yet")
        w.arm(started: true)
        w.record(.keyDown(code: CtrlKeys.code("l")!, marker: CtrlStopWatch.ctrlMarker))
        w.record(.mouse(marker: CtrlStopWatch.ctrlMarker))
        XCTAssertEqual(w.flags.userInput, false, "ctrl's own events don't count")
        w.record(.keyDown(code: CtrlKeys.escape, marker: 0))
        XCTAssertEqual(w.flags.stopped, true)
        w.arm(started: true)
        XCTAssertEqual(w.flags.stopped, false, "re-arming resets")
        w.record(.scroll(marker: 0))
        XCTAssertEqual(w.flags.userInput, true)
    }

    func testNoTapMeansNotArmedAndADisabledTapStops() {
        let w = CtrlStopWatch()
        w.arm(started: false)
        XCTAssertFalse(w.flags.armed)
        w.arm(started: true)
        w.record(.tapDisabled)
        XCTAssertTrue(w.flags.stopped)
        w.disarm()
        XCTAssertFalse(w.flags.armed)
    }
}
