import Foundation
import QrtzShared

// ADR 0217: nano's Apple Calendar reader, Foundation-only (the EventKit store is QrtzNanoMac's EventKitCalendarStore;
// tests stub it). Read-only: the store has no write call at all.

/// What nano needs from the calendar database.
public protocol CalendarStore: AnyObject, Sendable {
    /// The current authorization, without asking.
    func access() -> CalendarAccessState
    /// Asks for full (read) access: the macOS prompt the first time, else the stored answer.
    func requestAccess() async -> CalendarAccessState
    func calendars() -> [CalendarInfo]
    /// Timed and all-day events overlapping [fromMs, toMs), with only the kept fields.
    func events(fromMs: Int64, toMs: Int64) -> [CalendarEventInfo]
    /// Calls `onChange` on every calendar database change (EKEventStoreChanged) until `stopObserving`.
    func observe(_ onChange: @escaping @Sendable () -> Void)
    func stopObserving()
}

/// Serves `calendar.check` / `calendar.request` / `calendar.read` and reports `calendar.changed`, plus an unasked
/// `calendar.access` when the authorization changes (System Settings). No store (Linux, an older SDK) = unsupported.
public final class CalendarService: @unchecked Sendable {
    private let store: CalendarStore?
    private let send: @Sendable (HelperMessage) -> Void
    private let lock = NSLock()
    private var observing = false
    private var last: CalendarAccessState?

    public init(store: CalendarStore?, send: @escaping @Sendable (HelperMessage) -> Void) {
        self.store = store; self.send = send
    }

    public static func owns(_ m: HostMessage) -> Bool {
        switch m {
        case .calendarCheck, .calendarRequest, .calendarRead: return true
        default: return false
        }
    }

    public func handle(_ m: HostMessage) async {
        guard let store else {
            switch m {
            case .calendarCheck(let id), .calendarRequest(let id), .calendarRead(let id, _, _):
                send(.calendarAccess(id: id, state: .unsupported))
            default: break
            }
            return
        }
        switch m {
        case .calendarCheck(let id):
            answer(id, store.access())
        case .calendarRequest(let id):
            // Only `not-determined` shows the prompt; any other state is the stored answer.
            let st = store.access() == .notDetermined ? await store.requestAccess() : store.access()
            answer(id, st)
        case .calendarRead(let id, let from, let to):
            let st = store.access()
            guard st == .granted else { return answer(id, st) }
            noteAccess(st)
            guard to > from, to - from <= CalendarLimits.windowMaxMs else {
                send(.log(level: .warn, msg: "calendar.read: window \(from)..\(to) refused"))
                send(.calendarEvents(id: id, calendars: [], events: []))
                return
            }
            let evs = store.events(fromMs: from, toMs: to)
                .filter { $0.endMs > from && $0.startMs < to && !$0.id.isEmpty }
                .sorted { ($0.startMs, $0.id) < ($1.startMs, $1.id) }
            send(.calendarEvents(id: id, calendars: store.calendars(), events: Array(evs.prefix(CalendarLimits.eventsMax))))
        default: break
        }
    }

    public func shutdown() {
        lock.lock(); let was = observing; observing = false; lock.unlock()
        if was { store?.stopObserving() }
    }

    private func answer(_ id: String, _ st: CalendarAccessState) {
        noteAccess(st)
        send(.calendarAccess(id: id, state: st))
    }

    /// Remembers the state and starts watching the database once access is granted.
    private func noteAccess(_ st: CalendarAccessState) {
        lock.lock()
        last = st
        let start = st == .granted && !observing
        if start { observing = true }
        lock.unlock()
        if start {
            store?.observe { [weak self] in self?.changed() }
        }
    }

    private func changed() {
        guard let store else { return }
        let st = store.access()
        lock.lock()
        let moved = last != nil && last != st
        last = st
        lock.unlock()
        if moved { send(.calendarAccess(id: nil, state: st)) }
        send(.calendarChanged)
    }
}
