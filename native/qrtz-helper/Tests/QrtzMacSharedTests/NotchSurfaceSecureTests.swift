#if os(macOS)
import AppKit
import XCTest
@testable import QrtzMacShared
import QrtzShared

/// Security review (Bozniak): secure input turning on while module content is shown hides that content through the
/// secure path, synchronously, whatever the motion settings.
@MainActor
final class NotchSurfaceSecureTests: XCTestCase {
    func testSecureInputMidShowTakesTheSecurePathAndZeroesContentAtOnce() {
        _ = NSApplication.shared
        let s = NotchSurface(fontName: nil)
        var o: [String: JSONValue] = [:]
        for (k, _, hi) in NotchMotion.ranges { o[k] = .number(hi) }  // slowest fades and springs allowed
        s.motion = NotchMotion(json: o)
        var secure = false
        s.isSecureInput = { secure }
        s.show(NotchEntry(owner: "module.nano", priority: .module, content: .status(text: "syncing", tone: .normal), source: "nano"))
        XCTAssertEqual(s.contentOpacity, 1, "module content drawn")
        XCTAssertNil(s.lastRetractSecure)

        secure = true
        s.recheckSecureInput()  // the poll's tick
        XCTAssertEqual(s.lastRetractSecure, true, "hidden through retract(secure: true)")
        XCTAssertEqual(s.contentOpacity, 0, "content opacity 0 before the call returns")
    }

    func testAPlainDismissIsNotTheSecurePath() {
        _ = NSApplication.shared
        let s = NotchSurface(fontName: nil)
        s.isSecureInput = { false }
        s.show(NotchEntry(owner: "module.nano", priority: .module, content: .status(text: "x", tone: .normal), source: "nano"))
        s.dismiss(owner: "module.nano")
        XCTAssertEqual(s.lastRetractSecure, false)
    }

    private func module(_ text: String = "syncing") -> NotchEntry {
        NotchEntry(owner: "module.nano", priority: .module, content: .status(text: text, tone: .normal), source: "nano")
    }

    func testSecureOnMidShowZeroesContentWithinOnePollTick() {
        _ = NSApplication.shared
        let s = NotchSurface(fontName: nil)
        var secure = false
        s.isSecureInput = { secure }
        s.show(module())
        XCTAssertEqual(s.contentOpacity, 1)
        XCTAssertTrue(s.secureWatchActive)
        secure = true  // no explicit recheck: the poll (or an observer) must catch it
        // Spin the main run loop until the poll fires. A loaded CI VM can delay a 50 ms timer past two intervals, so
        // the deadline is generous; the assertion is still that the poll alone (no explicit recheck) hides it.
        let deadline = Date().addingTimeInterval(2)
        while s.lastRetractSecure == nil, Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(NotchSurface.securePollInterval))
        }
        XCTAssertEqual(s.lastRetractSecure, true)
        XCTAssertEqual(s.contentOpacity, 0)
    }

    func testWithSecureOnShowNeverDrawsModuleContent() {
        _ = NSApplication.shared
        let s = NotchSurface(fontName: nil)
        s.isSecureInput = { true }
        s.show(module())
        XCTAssertEqual(s.contentOpacity, 0, "nothing drawn")
        XCTAssertFalse(s.panelVisible, "the panel never ordered front")
        XCTAssertNil(s.lastRetractSecure, "never drawn, so nothing to retract")
        s.update(owner: "module.nano", content: .status(text: "still", tone: .normal))
        XCTAssertEqual(s.contentOpacity, 0)
        XCTAssertTrue(s.secureWatchActive, "watching so it can show once secure input ends")
    }

    func testWatchStopsWhenEmptyAndNothingLeaks() {
        _ = NSApplication.shared
        weak var weakSurface: NotchSurface?
        autoreleasepool {
            let s = NotchSurface(fontName: nil)
            weakSurface = s
            s.isSecureInput = { false }
            s.show(module())
            XCTAssertTrue(s.secureWatchActive)
            s.dismiss(owner: "module.nano")
            XCTAssertFalse(s.secureWatchActive, "poll and activation observer gone once the notch is empty")
            XCTAssertFalse(s.focusObserverAttached, "AX observer detached")
            s.show(module())  // leave the watch running: deinit must tear it down
        }
        RunLoop.main.run(until: Date().addingTimeInterval(0.6))  // let the retract's completion and timers drain
        XCTAssertNil(weakSurface, "no retain cycle through the timer, observers or animations")
    }
}
#endif
