import XCTest
@testable import QrtzEcho

/// Oct 18 (Jared: "holding fn does nothing"): the `echo timing:` line says how loud the mic was (its peak, a level
/// only), so a dictation that heard no words shows whether macOS delivered digital silence.
final class PeakTimingTests: XCTestCase {
    func testPeakOnTheTimingLine() {
        var t = DictationTiming()
        XCTAssertFalse(t.logLine.contains("peak_dbfs"), "no audio, no peak")
        t.peak = 0
        XCTAssertTrue(t.logLine.contains(" peak_dbfs=silent"), t.logLine)
        t.peak = 0.5
        XCTAssertTrue(t.logLine.contains(" peak_dbfs=-6.0"), t.logLine)
        XCTAssertEqual(t.json["peak"] as? Double, 0.5)
    }
}
