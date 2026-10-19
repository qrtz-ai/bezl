#if os(macOS)
import XCTest
@testable import QrtzSnap

/// The Mac checks. Reading the displays is harmless; anything that moves a real window or registers global hotkeys
/// runs only with QRTZ_SNAP_MAC_ACTIVE=1, so a plain `swift test` on a Mac never touches the user's windows.
final class SnapMacTests: XCTestCase {
    private func requireActive() throws {
        guard ProcessInfo.processInfo.environment["QRTZ_SNAP_MAC_ACTIVE"] == "1" else {
            throw XCTSkip("active Mac check: set QRTZ_SNAP_MAC_ACTIVE=1 (moves the frontmost window, then undoes it)")
        }
    }

    func testReadsTheDisplaysWithoutTouchingAnything() {
        let s = AXSnapWindowSystem()
        XCTAssertGreaterThan(s.primaryHeight, 0)
        XCTAssertFalse(s.screens.isEmpty)
        for d in s.screens { XCTAssertTrue(d.frame.contains(d.visibleFrame) || d.frame == d.visibleFrame) }
    }

    func testMovesTheFrontmostWindowLeftAndBack() throws {
        try requireActive()
        let s = AXSnapWindowSystem(), e = SnapEngine(system: s)
        guard let front = s.frontmost(), let before = s.rect(of: front.window) else { throw XCTSkip("no focused window") }
        guard case .moved = e.perform(.leftHalf) else { return XCTFail("left half didn't move") }
        guard case .moved(let back) = e.perform(.undo) else { return XCTFail("undo didn't move") }
        XCTAssertEqual(back, before)
    }

    func testRegistersAndDropsTheDefaultHotkeys() throws {
        try requireActive()
        let b = MacSnapBackend(send: { _ in })
        let failed = b.register(SnapShortcuts.resolve(overrides: [:]).shortcuts)
        print("[snap] hotkeys another app holds: \(failed)")
        XCTAssertEqual(b.register([:]), [:])
    }
}
#endif
