import Foundation

/// A copied, framework-free view of an AX subtree (built from AXUIElement on macOS, from fixtures in tests).
public struct AXNode: Equatable, Sendable {
    public var role: String
    public var subrole: String?
    public var title: String?
    public var value: String?
    public var desc: String?
    public var identifier: String?
    public var children: [AXNode]

    public init(role: String, subrole: String? = nil, title: String? = nil, value: String? = nil, desc: String? = nil, identifier: String? = nil, children: [AXNode] = []) {
        self.role = role; self.subrole = subrole; self.title = title; self.value = value; self.desc = desc; self.identifier = identifier; self.children = children
    }

    /// Best human-readable label, in AX precedence order.
    public var label: String {
        for s in [title, desc, value] { if let s, !s.trimmingCharacters(in: .whitespaces).isEmpty { return s.trimmingCharacters(in: .whitespacesAndNewlines) } }
        return ""
    }

    public func walk(_ visit: (AXNode, Int) -> Bool, depth: Int = 0) {
        guard visit(self, depth) else { return }
        for c in children { c.walk(visit, depth: depth + 1) }
    }

    public func first(where pred: (AXNode) -> Bool) -> AXNode? {
        var found: AXNode?
        walk { n, _ in
            if found != nil { return false }
            if pred(n) { found = n; return false }
            return true
        }
        return found
    }

    /// All static-text labels under this node, in reading order.
    public var texts: [String] {
        var out: [String] = []
        walk { n, _ in
            if n.role == "AXStaticText" || n.role == "AXTextArea" { let l = n.label; if !l.isEmpty { out.append(l) } }
            return true
        }
        return out
    }
}

/// What one scrape pass found. nil fields mean "this pass could not tell" (keep the previous value).
public struct MeetingScrape: Equatable, Sendable {
    public var participants: [String]?
    public var activeSpeaker: String?
    /// (speaker, cumulative caption text) in on-screen order.
    public var captions: [(speaker: String?, text: String)] = []

    public init(participants: [String]? = nil, activeSpeaker: String? = nil, captions: [(speaker: String?, text: String)] = []) {
        self.participants = participants; self.activeSpeaker = activeSpeaker; self.captions = captions
    }

    public static func == (a: MeetingScrape, b: MeetingScrape) -> Bool {
        a.participants == b.participants && a.activeSpeaker == b.activeSpeaker
            && a.captions.map { "\($0.speaker ?? "")\u{1}\($0.text)" } == b.captions.map { "\($0.speaker ?? "")\u{1}\($0.text)" }
    }
}

/// Heuristic scrapers. Real AX shapes drift with app releases, so every rule here is matched against fixture trees in
/// Tests/QrtzHelperCoreTests/AXScrapeTests.swift; record new fixtures with `qrtz-helper --dump-ax` on a Mac. [MAC-VERIFY]
public enum AXScraper {
    /// "Jane Doe (Host, me)", "Jane Doe, muted", "Jane Doe is speaking" → "Jane Doe".
    public static func cleanName(_ raw: String) -> String {
        var s = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if let i = s.firstIndex(of: "(") { s = String(s[..<i]) }
        for sep in [",", " is speaking", " is talking", " - ", "\n"] {
            if let r = s.range(of: sep, options: .caseInsensitive) { s = String(s[..<r.lowerBound]) }
        }
        return s.trimmingCharacters(in: .whitespaces)
    }

    static let speakingPatterns: [NSRegularExpression] = [
        #"^(.+?),?\s+(?:is\s+)?(?:speaking|talking)\b"#,
        #"^(?:active speaker|speaking now|currently speaking)[:,]?\s+(.+)$"#,
    ].map { try! NSRegularExpression(pattern: $0, options: [.caseInsensitive]) }

    static func speakingName(_ label: String) -> String? {
        for re in speakingPatterns {
            if let g = MeetingClassifier.firstGroup(re, label) { let n = cleanName(g); if !n.isEmpty { return n } }
        }
        return nil
    }

    static func isLabeled(_ n: AXNode, _ words: [String]) -> Bool {
        let hay = [n.title, n.desc, n.identifier].compactMap { $0?.lowercased() }
        return hay.contains { h in words.contains { h.contains($0) } }
    }

    static let ignoredNames: Set<String> = ["", "participants", "people", "in call", "in the meeting", "contributors", "you", "mute", "unmute", "more options", "pin", "invite"]

    static func names(in list: AXNode) -> [String] {
        var out: [String] = []
        var seen = Set<String>()
        list.walk { n, _ in
            guard ["AXRow", "AXCell", "AXListItem", "AXGroup"].contains(n.role) || n.subrole == "AXOutlineRow" else { return true }
            // A row's first static text is the display name in Zoom, Meet and Teams.
            guard n.role != "AXGroup" || n.children.contains(where: { $0.role == "AXStaticText" }) else { return true }
            guard let raw = n.texts.first ?? (n.label.isEmpty ? nil : n.label) else { return true }
            let name = cleanName(raw)
            if !ignoredNames.contains(name.lowercased()), name.count <= 80, !seen.contains(name) { seen.insert(name); out.append(name) }
            return false
        }
        return out
    }

    public static func scrape(platform: MeetingPlatform, window: AXNode) -> MeetingScrape {
        var r = MeetingScrape()
        // Participants panel: Zoom "Participants (N)", Meet "People" / "Contributors", Teams "Participants".
        if let panel = window.first(where: { ($0.role == "AXTable" || $0.role == "AXOutline" || $0.role == "AXList" || $0.role == "AXGroup") && isLabeled($0, ["participants", "people", "contributors", "in call"]) }) {
            let n = names(in: panel)
            if !n.isEmpty { r.participants = n }
        }
        // Active speaker: tiles whose label announces speaking (Zoom app/web, Meet), or a speaking-indicator subtree.
        window.walk { n, _ in
            if r.activeSpeaker != nil { return false }
            for l in [n.desc, n.title, n.value].compactMap({ $0 }) { if let s = speakingName(l) { r.activeSpeaker = s; return false } }
            return true
        }
        // Captions: Meet "Captions" region (speaker name text, then caption text); Zoom "Live Transcript" / "Captions".
        if let region = window.first(where: { isLabeled($0, ["captions", "live transcript", "subtitles"]) }) {
            r.captions = captions(in: region, platform: platform)
        }
        return r
    }

    static func captions(in region: AXNode, platform: MeetingPlatform) -> [(speaker: String?, text: String)] {
        var out: [(speaker: String?, text: String)] = []
        // Each speaker turn is a child group: [name, text...]. A lone text without a name continues the previous turn.
        for turn in region.children {
            let t = turn.texts
            if t.count >= 2 {
                out.append((cleanName(t[0]), t.dropFirst().joined(separator: " ")))
            } else if let only = t.first {
                // Zoom formats a single line as "Name: text".
                if let r = only.range(of: ": "), only.distance(from: only.startIndex, to: r.lowerBound) <= 40 {
                    out.append((cleanName(String(only[..<r.lowerBound])), String(only[r.upperBound...])))
                } else {
                    out.append((out.last?.speaker, only))
                }
            }
        }
        return out
    }
}

/// Converts successive scrapes into deduplicated ax.* events. Captions are cumulative and get rewritten as the
/// recogniser revises them, so only completed lines are emitted: a caption line is final once a newer line exists,
/// or once it has been stable for `captionSettleMs`.
public final class ScrapeDiffer {
    public let captionSettleMs: Double
    private var participants: [String] = []
    private var speaker: String?
    private var emittedCaptions: [String] = []  // keys of emitted "speaker\u{1}text"
    private var pending: (key: String, speaker: String?, text: String, since: Double)?

    public init(captionSettleMs: Double = 1500) { self.captionSettleMs = captionSettleMs }

    public func reset() { participants = []; speaker = nil; emittedCaptions = []; pending = nil }

    public func diff(_ s: MeetingScrape, atMs: Double) -> [HelperMessage] {
        var out: [HelperMessage] = []
        if let p = s.participants, p != participants {
            participants = p
            out.append(.axParticipants(names: p))
        }
        if let a = s.activeSpeaker, a != speaker {
            speaker = a
            out.append(.axActiveSpeaker(atMs: atMs, name: a))
        }
        func key(_ c: (speaker: String?, text: String)) -> String { "\(c.speaker ?? "")\u{1}\(c.text)" }
        func emit(_ c: (speaker: String?, text: String)) {
            let k = key(c)
            guard !emittedCaptions.contains(k), !c.text.trimmingCharacters(in: .whitespaces).isEmpty else { return }
            emittedCaptions.append(k)
            if emittedCaptions.count > 200 { emittedCaptions.removeFirst(100) }
            out.append(.axCaption(atMs: atMs, speaker: c.speaker, text: c.text))
        }
        if !s.captions.isEmpty {
            for c in s.captions.dropLast() { emit(c) }
            let last = s.captions[s.captions.count - 1]
            if let p = pending, p.key == key(last) {
                if atMs - p.since >= captionSettleMs { emit(last) }
            } else {
                pending = (key(last), last.speaker, last.text, atMs)
            }
        }
        return out
    }
}
