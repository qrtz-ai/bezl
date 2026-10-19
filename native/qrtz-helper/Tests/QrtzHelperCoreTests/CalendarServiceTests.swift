import Foundation
import XCTest
import QrtzNano
@testable import QrtzShared

/// ADR 0217: nano's calendar reader against a stubbed store (EventKit is macOS only).
final class StubCalendarStore: CalendarStore, @unchecked Sendable {
    let lock = NSLock()
    var state: CalendarAccessState
    var afterPrompt: CalendarAccessState
    var prompts = 0
    var onChange: (@Sendable () -> Void)?
    var evs: [CalendarEventInfo] = []
    var reads: [(Int64, Int64)] = []
    init(_ s: CalendarAccessState, afterPrompt: CalendarAccessState = .granted) { state = s; self.afterPrompt = afterPrompt }
    func access() -> CalendarAccessState { lock.withLock { state } }
    func requestAccess() async -> CalendarAccessState { lock.withLock { prompts += 1; state = afterPrompt; return state } }
    func calendars() -> [CalendarInfo] { [CalendarInfo(id: "w", title: "Work")] }
    func events(fromMs: Int64, toMs: Int64) -> [CalendarEventInfo] { lock.withLock { reads.append((fromMs, toMs)); return evs } }
    func observe(_ f: @escaping @Sendable () -> Void) { lock.withLock { onChange = f } }
    func stopObserving() { lock.withLock { onChange = nil } }
}

final class Sent: @unchecked Sendable {
    let lock = NSLock()
    var msgs: [HelperMessage] = []
    func add(_ m: HelperMessage) { lock.withLock { msgs.append(m) } }
    var all: [HelperMessage] { lock.withLock { msgs } }
}

func cev(_ id: String, _ s: Int64, _ e: Int64) -> CalendarEventInfo {
    CalendarEventInfo(id: id, calendarId: "w", title: "t \(id)", startMs: s, endMs: e, allDay: false, attendees: 2, meetingUrl: nil)
}

final class CalendarServiceTests: XCTestCase {
    func testCheckNeverPromptsAndRequestPromptsOnce() async {
        let st = StubCalendarStore(.notDetermined), out = Sent()
        let svc = CalendarService(store: st, send: out.add)
        await svc.handle(.calendarCheck(id: "a"))
        XCTAssertEqual(st.prompts, 0)
        XCTAssertEqual(out.all, [.calendarAccess(id: "a", state: .notDetermined)])
        await svc.handle(.calendarRequest(id: "b"))
        await svc.handle(.calendarRequest(id: "c"))
        XCTAssertEqual(st.prompts, 1, "only not-determined shows the prompt")
        XCTAssertEqual(out.all.suffix(2), [.calendarAccess(id: "b", state: .granted), .calendarAccess(id: "c", state: .granted)])
    }

    func testReadWithoutAccessAnswersTheState() async {
        for s in [CalendarAccessState.denied, .writeOnly, .restricted, .notDetermined] {
            let st = StubCalendarStore(s), out = Sent()
            await CalendarService(store: st, send: out.add).handle(.calendarRead(id: "r", fromMs: 0, toMs: 10))
            XCTAssertEqual(out.all, [.calendarAccess(id: "r", state: s)])
            XCTAssertTrue(st.reads.isEmpty)
            XCTAssertEqual(st.prompts, 0, "a read never prompts")
        }
    }

    func testReadSortsClipsAndRefusesWideWindows() async {
        let st = StubCalendarStore(.granted), out = Sent()
        st.evs = [cev("b", 50, 60), cev("a", 10, 20), cev("gone", -20, -10), cev("", 1, 2)]
        let svc = CalendarService(store: st, send: out.add, now: { 50 })
        await svc.handle(.calendarRead(id: "r", fromMs: 0, toMs: 100))
        XCTAssertEqual(out.all, [.calendarEvents(id: "r", calendars: [CalendarInfo(id: "w", title: "Work")], events: [cev("a", 10, 20), cev("b", 50, 60)])])
        await svc.handle(.calendarRead(id: "w", fromMs: 0, toMs: CalendarLimits.windowMaxMs + 1))
        XCTAssertEqual(out.all.last, .calendarEvents(id: "w", calendars: [], events: []))
        XCTAssertEqual(st.reads.count, 1)
    }

    func testReadRefusesWindowsAwayFromNow() async {
        let h: Int64 = 3_600_000, t: Int64 = 1_790_000_000_000
        let st = StubCalendarStore(.granted), out = Sent()
        let svc = CalendarService(store: st, send: out.add, now: { t })
        await svc.handle(.calendarRead(id: "ok", fromMs: t - 24 * h, toMs: t + 14 * 24 * h))
        XCTAssertEqual(st.reads.count, 1, "the host's own window passes")
        await svc.handle(.calendarRead(id: "old", fromMs: t - 30 * 24 * h, toMs: t - 29 * 24 * h))
        await svc.handle(.calendarRead(id: "far", fromMs: t + 20 * 24 * h, toMs: t + 21 * 24 * h))
        await svc.handle(.calendarRead(id: "edge", fromMs: t - 26 * h, toMs: t))
        XCTAssertEqual(st.reads.count, 1, "past or far-future windows never reach the store")
        XCTAssertEqual(out.all.last, .calendarEvents(id: "edge", calendars: [], events: []))
    }

    func testChangesAreReportedAndAccessChangesAnnounced() async {
        let st = StubCalendarStore(.granted), out = Sent()
        let svc = CalendarService(store: st, send: out.add)
        await svc.handle(.calendarCheck(id: "a"))
        XCTAssertNotNil(st.onChange, "observes once granted")
        st.onChange?()
        XCTAssertEqual(out.all.last, .calendarChanged)
        st.state = .denied
        st.onChange?()
        XCTAssertEqual(Array(out.all.suffix(2)), [.calendarAccess(id: nil, state: .denied), .calendarChanged])
        svc.shutdown()
        XCTAssertNil(st.onChange)
    }

    func testNoStoreIsUnsupported() async {
        let out = Sent()
        let svc = CalendarService(store: nil, send: out.add)
        await svc.handle(.calendarRequest(id: "x"))
        await svc.handle(.calendarRead(id: "y", fromMs: 0, toMs: 1))
        XCTAssertEqual(out.all, [.calendarAccess(id: "x", state: .unsupported), .calendarAccess(id: "y", state: .unsupported)])
    }
}
