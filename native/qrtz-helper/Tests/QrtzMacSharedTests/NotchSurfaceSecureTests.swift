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
}
#endif
