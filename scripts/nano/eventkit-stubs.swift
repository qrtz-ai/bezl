// ADR 0217: just enough of EventKit's API (macOS 14 SDK shapes) to typecheck QrtzNanoMac/Calendar.swift on Linux.
// Only what the file calls; a signature drift from the real SDK is caught by the Mac build, not here.
import Foundation

public enum EKEntityType { case event, reminder }
public enum EKAuthorizationStatus: Int { case notDetermined, restricted, denied, fullAccess, writeOnly }
public enum EKEventStatus: Int { case none, confirmed, tentative, canceled }
public class EKCalendar { public var calendarIdentifier: String = ""; public var title: String = "" }
public class EKParticipant {}
public class EKEvent {
    public var startDate: Date!
    public var endDate: Date!
    public var status: EKEventStatus = .none
    public var calendarItemIdentifier: String = ""
    public var url: URL?
    public var location: String?
    public var notes: String?
    public var calendar: EKCalendar!
    public var title: String!
    public var isAllDay: Bool = false
    public var attendees: [EKParticipant]?
}
public class EKEventStore {
    public init() {}
    public class func authorizationStatus(for: EKEntityType) -> EKAuthorizationStatus { .notDetermined }
    public func requestFullAccessToEvents() async throws -> Bool { false }
    public func reset() {}
    public func calendars(for: EKEntityType) -> [EKCalendar] { [] }
    public func predicateForEvents(withStart: Date, end: Date, calendars: [EKCalendar]?) -> NSPredicate { NSPredicate(value: true) }
    public func events(matching: NSPredicate) -> [EKEvent] { [] }
}
public extension Notification.Name { static let EKEventStoreChanged = Notification.Name("EKEventStoreChanged") }
