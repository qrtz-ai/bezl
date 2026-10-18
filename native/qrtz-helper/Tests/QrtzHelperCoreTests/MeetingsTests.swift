import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

final class MeetingsTests: XCTestCase {
    func w(_ b: String, _ t: String, _ u: String? = nil) -> WindowInfo { WindowInfo(pid: 7, bundleId: b, appName: "x", title: t, url: u) }

    func testZoomApp() {
        XCTAssertEqual(MeetingClassifier.classify(w("us.zoom.xos", "Zoom Meeting"))?.key, "zoom-app:7")
        XCTAssertEqual(MeetingClassifier.classify(w("us.zoom.xos", "Zoom Meeting - Design Review"))?.title, "Design Review")
        XCTAssertNil(MeetingClassifier.classify(w("us.zoom.xos", "Zoom Workplace")))
    }

    func testMeetChromeAndSafari() {
        let c = MeetingClassifier.classify(w("com.google.Chrome", "Meet - Weekly Product Sync", "https://meet.google.com/abc-defg-hij?authuser=0"))
        XCTAssertEqual(c, DetectedMeeting(key: "meet:abc-defg-hij", title: "Weekly Product Sync", platform: .googleMeet, app: "Google Meet (Chrome)"))
        // Safari often has no AXURL on the web area: fall back to the code in the title.
        let s = MeetingClassifier.classify(w("com.apple.Safari", "Meet - abc-defg-hij"))
        XCTAssertEqual(s?.key, "meet:abc-defg-hij")
        XCTAssertEqual(s?.title, "Google Meet")
        XCTAssertNil(MeetingClassifier.classify(w("com.google.Chrome", "Google Meet", "https://meet.google.com/landing")))
        XCTAssertNil(MeetingClassifier.classify(w("com.google.Chrome", "Inbox", "https://mail.google.com/")))
    }

    func testZoomWeb() {
        let m = MeetingClassifier.classify(w("com.apple.Safari", "Zoom", "https://app.zoom.us/wc/81234567890/join?fromPWA=1"))
        XCTAssertEqual(m?.key, "zoom-web:81234567890")
        XCTAssertEqual(m?.platform, .zoom)
        XCTAssertEqual(MeetingClassifier.classify(w("com.google.Chrome", "Zoom", "https://us02web.zoom.us/wc/join/81234567890"))?.app, "Zoom (Chrome)")
    }

    func testTrackerGraceAndDedupe() {
        let t = MeetingTracker(graceMs: 1000)
        let meet = w("com.google.Chrome", "Meet - abc-defg-hij", "https://meet.google.com/abc-defg-hij")
        XCTAssertEqual(t.update(windows: [meet, meet], nowMs: 0).count, 1)
        XCTAssertEqual(t.update(windows: [meet], nowMs: 500).count, 0)
        XCTAssertEqual(t.update(windows: [], nowMs: 1000).count, 0)  // within grace
        XCTAssertEqual(t.update(windows: [], nowMs: 1600), [.meetingEnded(key: "meet:abc-defg-hij")])
    }
}
