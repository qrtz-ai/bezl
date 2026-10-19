#if os(macOS)
import EventKit
import Foundation
import QrtzNano
import QrtzShared

// ADR 0217: nano's EventKit store. Read-only (no save/remove call anywhere), full access asked for only through
// `requestAccess` (the host sends calendar.request after the user turned the calendar on). Only the kept fields
// leave this file: an event's notes and location are scanned for a meeting link and dropped.

public final class EventKitCalendarStore: CalendarStore, @unchecked Sendable {
    private let store = EKEventStore()
    private let lock = NSLock()
    private var token: NSObjectProtocol?

    public init() {}

    public func access() -> CalendarAccessState {
        Self.map(EKEventStore.authorizationStatus(for: .event))
    }

    static func map(_ s: EKAuthorizationStatus) -> CalendarAccessState {
        switch s {
        case .notDetermined: return .notDetermined
        case .restricted: return .restricted
        case .denied: return .denied
        case .fullAccess: return .granted
        case .writeOnly: return .writeOnly
        @unknown default: return .denied
        }
    }

    public func requestAccess() async -> CalendarAccessState {
        do {
            let ok = try await store.requestFullAccessToEvents()
            store.reset()  // a store made before the grant sees no calendars until it is reset
            return ok ? .granted : access()
        } catch {
            return access()
        }
    }

    public func calendars() -> [CalendarInfo] {
        store.calendars(for: .event).map { CalendarInfo(id: $0.calendarIdentifier, title: CalendarText.clean($0.title)) }
    }

    public func events(fromMs: Int64, toMs: Int64) -> [CalendarEventInfo] {
        let from = Date(timeIntervalSince1970: Double(fromMs) / 1000)
        let to = Date(timeIntervalSince1970: Double(toMs) / 1000)
        let pred = store.predicateForEvents(withStart: from, end: to, calendars: nil)
        return store.events(matching: pred).compactMap { e -> CalendarEventInfo? in
            guard let start = e.startDate, let end = e.endDate, e.status != .canceled else { return nil }
            let startMs = Int64((start.timeIntervalSince1970 * 1000).rounded())
            // Occurrences of a recurring event share an identifier: the start makes each one distinct.
            let base = e.calendarItemIdentifier
            let url = CalendarText.meetingURL(e.url?.absoluteString)
                ?? CalendarText.firstMeetingURL(in: e.location) ?? CalendarText.firstMeetingURL(in: e.notes)
            return CalendarEventInfo(
                id: String("\(base)@\(startMs)".prefix(CalendarLimits.idMax)),
                calendarId: e.calendar?.calendarIdentifier ?? "",
                title: CalendarText.clean(e.title ?? ""),
                startMs: startMs, endMs: Int64((end.timeIntervalSince1970 * 1000).rounded()),
                allDay: e.isAllDay, attendees: e.attendees?.count ?? 0, meetingUrl: url)
        }
    }

    public func observe(_ onChange: @escaping @Sendable () -> Void) {
        lock.lock(); defer { lock.unlock() }
        guard token == nil else { return }
        token = NotificationCenter.default.addObserver(forName: .EKEventStoreChanged, object: store, queue: nil) { _ in onChange() }
    }

    public func stopObserving() {
        lock.lock(); let t = token; token = nil; lock.unlock()
        if let t { NotificationCenter.default.removeObserver(t) }
    }
}
#endif
