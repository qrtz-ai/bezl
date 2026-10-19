import Foundation

// ADR 0217: nano's Apple Calendar wire types (zod `CalendarAccessState`, `CALENDAR_LIMITS`, `calendar.events`).

/// EventKit's authorization, as nano sees it. `writeOnly` (macOS 14+) can't read events: nano treats it as denied.
public enum CalendarAccessState: String, Sendable, CaseIterable {
    case notDetermined = "not-determined", granted, denied, restricted, writeOnly = "write-only", unsupported
}

public enum CalendarLimits {
    public static let titleMax = 200
    public static let urlMax = 500
    public static let idMax = 512
    public static let calendarsMax = 200
    public static let eventsMax = 2000
    /// The widest window one `calendar.read` may ask for: 15 days.
    public static let windowMaxMs: Int64 = 15 * 24 * 3_600_000
    /// L1: how far before now a read may start (the host asks for one day back; an hour of slack).
    public static let backMaxMs: Int64 = 25 * 3_600_000
    /// L1: how far after now a read may end (the host asks for 14 days ahead).
    public static let aheadMaxMs: Int64 = 15 * 24 * 3_600_000
}

/// One calendar the user can pick in nano's settings.
public struct CalendarInfo: Equatable, Sendable {
    public let id: String
    public let title: String
    public init(id: String, title: String) { self.id = id; self.title = title }
}

/// One event with only the fields nano keeps: no notes, location, attendee names or addresses.
public struct CalendarEventInfo: Equatable, Sendable {
    public let id: String
    public let calendarId: String
    public let title: String
    public let startMs: Int64
    public let endMs: Int64
    public let allDay: Bool
    public let attendees: Int
    public let meetingUrl: String?
    public init(id: String, calendarId: String, title: String, startMs: Int64, endMs: Int64, allDay: Bool,
                attendees: Int, meetingUrl: String?) {
        self.id = id; self.calendarId = calendarId; self.title = title; self.startMs = startMs; self.endMs = endMs
        self.allDay = allDay; self.attendees = attendees; self.meetingUrl = meetingUrl
    }
}

public enum CalendarText {
    /// Drops control, format (tag, bidi, zero-width) and private-use characters, collapses whitespace, cuts every
    /// URL in the text to scheme + host + path (M2: a 'sync https://zoom.us/j/1?pwd=…' title keeps no passcode),
    /// caps at `max` characters (whole scalars). Titles are third-party text.
    /// ADR 0217 M2: every space-separated word holding "://" loses its query and fragment.
    public static func stripURLQueries(_ s: String) -> String {
        s.split(separator: " ", omittingEmptySubsequences: false).map { w -> String in
            guard w.range(of: "://") != nil, let cut = w.firstIndex(where: { $0 == "?" || $0 == "#" }) else { return String(w) }
            return String(w[..<cut])
        }.joined(separator: " ")
    }

    public static func clean(_ s: String, max: Int = CalendarLimits.titleMax) -> String {
        var out = String.UnicodeScalarView()
        var space = false
        for u in s.unicodeScalars {
            let v = u.value
            if u.properties.isWhitespace { space = !out.isEmpty; continue }
            let drop = u.properties.generalCategory == .control || u.properties.generalCategory == .format
                || u.properties.generalCategory == .privateUse || (0xE0000...0xE007F).contains(v)
            if drop { continue }
            if space { out.append(" "); space = false }
            out.append(u)
        }
        var r = stripURLQueries(String(out))
        if r.count > max { r = String(r.prefix(max)) }
        // Keep the wire bound in UTF-16 units too (zod counts those).
        while r.utf16.count > max { r.removeLast() }
        return r
    }

    /// Video-call hosts whose links nano keeps (https only; a subdomain counts: us02web.zoom.us).
    public static let meetingHosts = ["zoom.us", "meet.google.com", "teams.microsoft.com", "teams.live.com", "webex.com",
                                      "whereby.com", "facetime.apple.com"]

    /// An https link to a known video-call host, cut to scheme + host + path (no query, no fragment: a Zoom
    /// `?pwd=` passcode never leaves the helper; ADR 0217 M2) and at most `urlMax` long; else nil.
    public static func meetingURL(_ raw: String?) -> String? {
        guard let raw = raw?.trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty,
              raw.utf16.count <= CalendarLimits.urlMax,
              !raw.unicodeScalars.contains(where: { $0.properties.isWhitespace || $0.properties.generalCategory == .control }),
              raw.lowercased().hasPrefix("https://") else { return nil }
        let rest = raw.dropFirst("https://".count)
        let authority = rest.split(whereSeparator: { $0 == "/" || $0 == "?" || $0 == "#" }).first.map(String.init) ?? ""
        guard !authority.isEmpty, !authority.contains("@") else { return nil }
        let host = (authority.split(separator: ":").first.map(String.init) ?? "").lowercased()
        guard meetingHosts.contains(where: { host == $0 || host.hasSuffix("." + $0) }) else { return nil }
        let cut = raw.firstIndex(where: { $0 == "?" || $0 == "#" }).map { String(raw[..<$0]) } ?? raw
        return cut
    }

    /// The first meeting link in free text (an event's notes or location; the text itself is never kept).
    public static func firstMeetingURL(in text: String?) -> String? {
        guard let text else { return nil }
        for token in text.split(whereSeparator: { $0.isWhitespace || $0 == "<" || $0 == ">" || $0 == "\"" || $0 == "(" || $0 == ")" }) {
            if let u = meetingURL(String(token)) { return u }
        }
        return nil
    }
}
