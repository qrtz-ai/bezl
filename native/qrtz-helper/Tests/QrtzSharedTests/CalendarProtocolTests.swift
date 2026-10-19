import Foundation
import XCTest
@testable import QrtzShared

/// ADR 0217: the calendar wire caps and cleans on encode.
final class CalendarProtocolTests: XCTestCase {
    func testTitlesAreCleanedAndCapped() {
        XCTAssertEqual(CalendarText.clean("weekly\u{7} \t sync\u{E0041}\u{202E}\u{200B}"), "weekly sync")
        XCTAssertEqual(CalendarText.clean(String(repeating: "x", count: 300)).count, CalendarLimits.titleMax)
        XCTAssertLessThanOrEqual(CalendarText.clean(String(repeating: "🎤", count: 150)).utf16.count, CalendarLimits.titleMax)
    }

    func testOnlyKnownHttpsMeetingLinksSurvive() {
        XCTAssertEqual(CalendarText.meetingURL("https://us02web.zoom.us/j/1?pwd=x#f"), "https://us02web.zoom.us/j/1")
        // M2: no query or fragment ever leaves the helper (Zoom pwd=, Meet authuser=).
        XCTAssertEqual(CalendarText.meetingURL("https://zoom.us/j/9#pwd=y"), "https://zoom.us/j/9")
        XCTAssertEqual(CalendarText.meetingURL("https://meet.google.com/abc?authuser=1"), "https://meet.google.com/abc")
        XCTAssertEqual(CalendarText.firstMeetingURL(in: "join: https://zoom.us/j/5?pwd=abc now"), "https://zoom.us/j/5")
        XCTAssertNotNil(CalendarText.meetingURL("https://meet.google.com/abc-defg-hij"))
        for bad in ["http://zoom.us/j/1", "https://zoom.us.evil.example/j", "https://u@zoom.us/j", "javascript:x", "https://evil.example"] {
            XCTAssertNil(CalendarText.meetingURL(bad), bad)
        }
        XCTAssertEqual(CalendarText.firstMeetingURL(in: "join: <https://teams.microsoft.com/l/x> or call"), "https://teams.microsoft.com/l/x")
        XCTAssertNil(CalendarText.firstMeetingURL(in: "room 4b, https://example.com/x"))
    }

    func testEventsEncodeCleanedAndCapped() throws {
        let evs = (0..<(CalendarLimits.eventsMax + 5)).map {
            CalendarEventInfo(id: "e\($0)", calendarId: "c", title: "t\u{7}", startMs: 1, endMs: 2, allDay: false,
                              attendees: 99_999, meetingUrl: "https://evil.example")
        }
        let l = HelperMessage.calendarEvents(id: "r", calendars: [CalendarInfo(id: "c", title: "Work")], events: evs).line
        guard case .calendarEvents(_, let cals, let got) = try HelperMessage(json: try JSONValue.parse(l)) else { return XCTFail() }
        XCTAssertEqual(cals.count, 1)
        XCTAssertEqual(got.count, CalendarLimits.eventsMax)
        XCTAssertEqual(got[0].title, "t")
        XCTAssertEqual(got[0].attendees, 10_000)
        XCTAssertNil(got[0].meetingUrl)
    }
}
