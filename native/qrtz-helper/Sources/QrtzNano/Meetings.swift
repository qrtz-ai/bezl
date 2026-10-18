import Foundation
import QrtzShared

/// What the AX layer can see about a top-level window, independent of the AX API (so it is testable on Linux).
public struct WindowInfo: Equatable, Sendable {
    public var pid: Int32
    public var bundleId: String
    public var appName: String
    public var title: String
    /// URL of the focused web area (browsers only), when AX exposes it.
    public var url: String?
    public init(pid: Int32, bundleId: String, appName: String, title: String, url: String? = nil) {
        self.pid = pid; self.bundleId = bundleId; self.appName = appName; self.title = title; self.url = url
    }
}

public struct DetectedMeeting: Equatable, Sendable {
    public var key: String
    public var title: String
    public var platform: MeetingPlatform
    public var app: String
}

public enum MeetingClassifier {
    public static let zoomApp = "us.zoom.xos"
    public static let teamsApps: Set<String> = ["com.microsoft.teams2", "com.microsoft.teams"]
    /// P0 browsers are Chrome and Safari; the others are best-effort (same AX shape as Chrome for Chromium ones).
    public static let browsers: [String: String] = [
        "com.google.Chrome": "Chrome",
        "com.google.Chrome.beta": "Chrome",
        "com.apple.Safari": "Safari",
        "com.apple.SafariTechnologyPreview": "Safari",
        "company.thebrowser.Browser": "Arc",
        "com.microsoft.edgemac": "Edge",
        "com.brave.Browser": "Brave",
    ]

    private static let meetCode = try! NSRegularExpression(pattern: #"meet\.google\.com/([a-z]{3,4}-[a-z]{4}-[a-z]{3,4})"#, options: [.caseInsensitive])
    private static let meetTitleCode = try! NSRegularExpression(pattern: #"\b([a-z]{3}-[a-z]{4}-[a-z]{3})\b"#)
    private static let zoomWeb = try! NSRegularExpression(pattern: #"(?:^|//)(?:[\w-]+\.)?zoom\.us/(?:wc|j|s)/(?:join/)?(\d{9,12})"#, options: [.caseInsensitive])
    private static let teamsWeb = try! NSRegularExpression(pattern: #"teams\.(?:microsoft|live)\.com/"#, options: [.caseInsensitive])

    static func firstGroup(_ re: NSRegularExpression, _ s: String) -> String? {
        let r = NSRange(s.startIndex..., in: s)
        guard let m = re.firstMatch(in: s, range: r), m.numberOfRanges > 1, let g = Range(m.range(at: 1), in: s) else { return nil }
        return String(s[g])
    }

    /// Returns a meeting if this window is an in-progress call (not a lobby, home screen or chat).
    public static func classify(_ w: WindowInfo) -> DetectedMeeting? {
        let title = w.title.trimmingCharacters(in: .whitespaces)
        if w.bundleId == zoomApp {
            // The meeting window is "Zoom Meeting" / "Zoom Webinar" (optionally "Zoom Meeting - <topic>" or with
            // " 40-Minutes" suffixes on free accounts). The home window is "Zoom" / "Zoom Workplace".
            let lower = title.lowercased()
            guard lower.hasPrefix("zoom meeting") || lower.hasPrefix("zoom webinar") else { return nil }
            let topic = title.components(separatedBy: " - ").dropFirst().joined(separator: " - ")
            return DetectedMeeting(key: "zoom-app:\(w.pid)", title: topic.isEmpty ? "Zoom Meeting" : topic, platform: .zoom, app: "Zoom")
        }
        if teamsApps.contains(w.bundleId) {
            // Teams (secondary): call windows are titled "<name> | Microsoft Teams" with a separate "Meeting" stage.
            guard title.contains("| Microsoft Teams"), !title.lowercased().hasPrefix("chat") else { return nil }
            let name = title.components(separatedBy: " | ").first ?? "Teams meeting"
            return DetectedMeeting(key: "teams-app:\(w.pid):\(name)", title: name, platform: .teams, app: "Microsoft Teams")
        }
        guard let browser = browsers[w.bundleId] else { return nil }
        let url = w.url ?? ""
        if let code = firstGroup(meetCode, url) ?? (title.hasPrefix("Meet") ? firstGroup(meetTitleCode, title) : nil) {
            // Titles: "Meet - abc-defg-hij" in the lobby and in-call; "Meet - <event name>" for calendar meetings.
            var t = title
            for p in ["Meet - ", "Meet – ", "Meet: "] where t.hasPrefix(p) { t = String(t.dropFirst(p.count)) }
            for suffix in [" - Google Chrome", " – Audio playing", " - Audio playing", " - Camera or microphone recording"] {
                t = t.replacingOccurrences(of: suffix, with: "")
            }
            if t.isEmpty || t == "Meet" || t == code { t = "Google Meet" }
            return DetectedMeeting(key: "meet:\(code.lowercased())", title: t, platform: .googleMeet, app: "Google Meet (\(browser))")
        }
        if let id = firstGroup(zoomWeb, url) {
            return DetectedMeeting(key: "zoom-web:\(id)", title: "Zoom Meeting", platform: .zoom, app: "Zoom (\(browser))")
        }
        if url.range(of: teamsWeb.pattern, options: .regularExpression) != nil, title.lowercased().contains("meeting") {
            return DetectedMeeting(key: "teams-web:\(w.pid)", title: "Teams meeting", platform: .teams, app: "Microsoft Teams (\(browser))")
        }
        return nil
    }
}

/// Tracks which meetings are live across polls and turns changes into protocol events.
/// A meeting must be missing for `graceMs` before `meeting.ended` (tabs switch, windows re-create, AX blips).
public final class MeetingTracker {
    public let graceMs: Double
    private var live: [String: (meeting: DetectedMeeting, lastSeen: Double)] = [:]
    public init(graceMs: Double = 4000) { self.graceMs = graceMs }

    public var current: [DetectedMeeting] { live.values.map(\.meeting).sorted { $0.key < $1.key } }

    public func update(windows: [WindowInfo], nowMs: Double) -> [HelperMessage] {
        var out: [HelperMessage] = []
        var seen = Set<String>()
        for w in windows {
            guard let m = MeetingClassifier.classify(w), !seen.contains(m.key) else { continue }
            seen.insert(m.key)
            if live[m.key] == nil {
                out.append(.meetingDetected(key: m.key, title: m.title, platform: m.platform, app: m.app))
            }
            live[m.key] = (m, nowMs)
        }
        for (k, v) in live where !seen.contains(k) && nowMs - v.lastSeen >= graceMs {
            live[k] = nil
            out.append(.meetingEnded(key: k))
        }
        return out
    }
}
