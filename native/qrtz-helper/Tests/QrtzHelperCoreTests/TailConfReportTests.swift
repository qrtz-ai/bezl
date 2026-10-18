import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// ADR 0132 (review §12.39): FluidAudio's placeholder confidences aren't reported (N170), the timing line prints
/// three places (P5), and `dictation.stopped` carries `tailConf` / `tailDropped` for talk's latency line.
final class TailConfReportTests: XCTestCase {
    func testFluidAudioPlaceholdersAreNotConfidences() {
        // 0.1: FluidAudio's value for an empty transcription; blank after trimming counts as empty.
        XCTAssertNil(BatchResult.reportedConfidence(0.1, text: "", timedTokens: 0))
        XCTAssertNil(BatchResult.reportedConfidence(0.1, text: " \n\t", timedTokens: 2))
        // 0.5: token confidences missing or not one per token, i.e. no token timings.
        XCTAssertNil(BatchResult.reportedConfidence(0.5, text: "you", timedTokens: 0))
        XCTAssertNil(BatchResult.reportedConfidence(.nan, text: "you", timedTokens: 1))
        XCTAssertNil(BatchResult.reportedConfidence(.infinity, text: "you", timedTokens: 1))
    }

    func testARealConfidenceIsReportedAsIs() {
        XCTAssertEqual(BatchResult.reportedConfidence(0.5, text: "you", timedTokens: 1), 0.5, "a real 0.5 with timings stays")
        XCTAssertEqual(BatchResult.reportedConfidence(0.1, text: " oh ", timedTokens: 1), 0.1)
        XCTAssertEqual(BatchResult.reportedConfidence(0.873, text: "king and shelby.", timedTokens: 4), 0.873)
        XCTAssertEqual(BatchResult.reportedConfidence(1.2, text: "a", timedTokens: 1), 1)
        XCTAssertEqual(BatchResult.reportedConfidence(-0.1, text: "a", timedTokens: 1), 0)
    }

    func testTheTimingLinePrintsThreePlaces() {
        var st = SegmentStats(); st.segments = 2; st.tailConf = 0.595; st.tailDropped = true
        var t = DictationTiming(); t.segments = st
        XCTAssertTrue(t.logLine.contains(" tail_conf=0.595 tail_dropped=1"), t.logLine)
    }

    func testStoppedCarriesTheTail() {
        let tr = SegmentedTranscriber(ConfBatch(tail: 0.4), rule: SegmentRule())
        var st = SegmentStats(); st.segments = 3; st.tailDecodeMs = 71.24; st.tailConf = 0.41234; st.tailDropped = true
        let dropped = DictationEngine.asrInfo(tr, st)
        XCTAssertEqual(dropped?.tailConf, 0.412, "rounded to 3 places")
        XCTAssertEqual(dropped?.tailDropped, true)
        st.tailConf = 0.9876; st.tailDropped = false
        let kept = DictationEngine.asrInfo(tr, st)
        XCTAssertEqual(kept?.tailConf, 0.988)
        XCTAssertNil(kept?.tailDropped, "only a dropped tail says so")
        st.tailConf = nil
        let none = DictationEngine.asrInfo(tr, st)
        XCTAssertNil(none?.tailConf); XCTAssertNil(none?.tailDropped)
        XCTAssertNil(DictationEngine.asrInfo(tr, nil)?.tailConf)
    }

    func testTheWireFieldsRoundTripAndAreValidated() throws {
        let m = HelperMessage.dictationStopped(id: "d", text: "one two", durationMs: 1, asr:
            DictationAsrInfo(asr: "parakeet-tdt-v2", segments: 2, tailConf: 0.412, tailDropped: true))
        XCTAssertTrue(m.line.contains(#""tailConf":0.412"#) && m.line.contains(#""tailDropped":true"#), m.line)
        XCTAssertEqual(try HelperMessage(json: try JSONValue.parse(m.line)), m)
        let old = HelperMessage.dictationStopped(id: "d", text: "", durationMs: 1, asr: DictationAsrInfo(asr: "p", segments: 1))
        XCTAssertFalse(old.line.contains("tailConf") || old.line.contains("tailDropped"), old.line)
        for bad in [#""tailConf":1.5"#, #""tailConf":-0.1"#, #""tailConf":"x""#, #""tailDropped":"yes""#] {
            let l = #"{"asr":"p","durationMs":1,"id":"d",\#(bad),"text":"","type":"dictation.stopped"}"#
            XCTAssertThrowsError(try HelperMessage(json: try JSONValue.parse(l)), l)
        }
    }
}
