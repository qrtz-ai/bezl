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
        XCTAssertEqual(w.flags.stopped, true, "a re-arm inside the run keeps the stop (C1)")
        w.disarm(); w.arm(started: true)
        XCTAssertEqual(w.flags.stopped, false, "a new run starts clean")
        w.record(.scroll(marker: 0))
        XCTAssertEqual(w.flags.userInput, true)
    }

    /// Review C1: Esc pressed between two actions (the host re-arms before each) still stops the second.
    func testEscBetweenTwoActionsIsNotLost() {
        let w = CtrlStopWatch()
        w.arm(started: true)
        XCTAssertTrue(w.mayAct)            // action 1
        w.record(.keyDown(code: CtrlKeys.escape, marker: 0))
        w.arm(started: true)               // the guard before action 2
        XCTAssertTrue(w.flags.stopped)
        XCTAssertFalse(w.mayAct)
    }

    /// Review M6 / (d): a disabled tap is unarmed at once and a re-arm in the run can't revive it; secure input
    /// counts as unarmed.
    func testDisabledTapStaysUnarmedAndSecureInputUnarms() {
        let w = CtrlStopWatch()
        w.arm(started: true)
        w.record(.tapDisabled)
        w.arm(started: true)
        XCTAssertFalse(w.flags.armed); XCTAssertFalse(w.mayAct)
        let v = CtrlStopWatch()
        v.arm(started: true); v.lost()
        XCTAssertFalse(v.mayAct)
    }

    func testNoTapMeansNotArmedAndADisabledTapStops() {
        let w = CtrlStopWatch()
        w.arm(started: false)
        XCTAssertFalse(w.flags.armed)
        w.arm(started: true)
        XCTAssertFalse(w.flags.armed, "a run that started unwatched stays unwatched")
        w.disarm(); w.arm(started: true)
        w.record(.tapDisabled)
        XCTAssertTrue(w.flags.stopped)
        w.disarm()
        XCTAssertFalse(w.flags.armed)
    }
}
