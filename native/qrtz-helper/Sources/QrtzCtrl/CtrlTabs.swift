// Browser tabs for "close all my tabs" on the Mac (run 12, ADR 0119), the plain-data half: Foundation-only so it
// builds and tests on Linux. The macOS backend runs these scripts against a browser that is already running and reads
// the tab bar through AX; nothing here or there activates the browser, selects a tab or moves focus (ADR 0100).
//
// What ctrl can know about a tab, and how:
// - pinned: the browser's own tab bar in the AX tree. Safari names a pinned tab's role "pinned tab" (a plain one
//   "tab"); Chromium browsers label a pinned tab "<title> - Pinned". Pinned tabs always lead the strip, and the AX strip
//   is matched to a scripted window only when its tab names line up one to one with that window's titles. Anything
//   else (no AX grant, another UI language, a label we don't recognise, a strip that doesn't line up) is nil.
// - unsaved form input: a read-only page script that compares every field with its default and returns one character
//   ("1" changed, "0" untouched). No value ever leaves the page. It needs the browser's own "allow javascript from
//   apple events" setting; without it the answer is nil.
// nil means "can't tell", and a tab ctrl can't tell about is never closed without asking (ADR 0114).
import Foundation

public enum CtrlTabFlavor: Equatable, Sendable {
    case safari, chromium

    public static func of(_ bundleId: String) -> CtrlTabFlavor? {
        switch bundleId {
        case "com.apple.Safari", "com.apple.SafariTechnologyPreview": return .safari
        case "com.google.Chrome", "com.google.Chrome.beta", "com.google.Chrome.dev", "com.google.Chrome.canary",
             "org.chromium.Chromium", "com.brave.Browser", "com.microsoft.edgemac", "com.vivaldi.Vivaldi": return .chromium
        default: return nil
        }
    }
}

/// One scripted tab: its window's script id, its 1-based index there, and what the page script said.
public struct CtrlTabRow: Equatable, Sendable {
    public let windowId: Int, index: Int, title: String, url: String, unsaved: Bool?
    public init(windowId: Int, index: Int, title: String, url: String, unsaved: Bool?) {
        self.windowId = windowId; self.index = index; self.title = title; self.url = url; self.unsaved = unsaved
    }
}

/// One tab as the AX tab bar shows it.
public struct CtrlAXTab: Equatable, Sendable {
    public let name: String, roleDescription: String?
    public init(name: String, roleDescription: String?) { self.name = name; self.roleDescription = roleDescription }
}

/// A scripted tab with what ctrl knows about it.
public struct CtrlTab: Equatable, Sendable {
    public let row: CtrlTabRow, pinned: Bool?
    public var unsaved: Bool? { row.unsaved }
    /// A blank new-tab page (the checks don't count it, ADR 0114).
    public var blank: Bool { CtrlTabs.blankURLs.contains(row.url.lowercased()) }
}

public enum CtrlTabs {
    static let field = "\u{1F}", record = "\u{1E}"
    static let blankURLs: Set<String> = ["", "about:blank", "about:newtab", "favorites://", "topsites://",
                                         "chrome://newtab/", "chrome://new-tab-page/", "edge://newtab/", "brave://newtab/"]

    /// The page script: "1" when any field differs from its default (or a contenteditable holds text, or the page
    /// set an unload guard), "0" otherwise, "?" if it threw. It reads, never writes, and returns nothing but that.
    public static let unsavedJS = """
    (function(){try{var d=0,q=document.querySelectorAll('input,textarea,select'),skip=['hidden','submit','button','reset','image'];\
    for(var i=0;i<q.length&&!d;i++){var e=q[i],t=(e.type||'').toLowerCase();\
    if(e.tagName=='SELECT'){for(var j=0;j<e.options.length;j++){if(e.options[j].selected!=e.options[j].defaultSelected){d=1}}}\
    else if(t=='checkbox'||t=='radio'){if(e.checked!=e.defaultChecked){d=1}}\
    else if(t=='file'){if(e.files&&e.files.length){d=1}}\
    else if(skip.indexOf(t)<0&&e.value!=e.defaultValue){d=1}}\
    var c=document.querySelectorAll('[contenteditable]:not([contenteditable=false])');\
    for(var k=0;k<c.length&&!d;k++){if((c[k].textContent||'').trim().length){d=1}}\
    if(window.onbeforeunload){d=1}return d?'1':'0'}catch(x){return '?'}})()
    """

    /// An AppleScript string literal, or nil when `s` holds a control character (then that tab can't be targeted).
    public static func literal(_ s: String) -> String? {
        guard !s.unicodeScalars.contains(where: { $0.value < 0x20 || $0.value == 0x7F }) else { return nil }
        return "\"" + s.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
    }

    static func tell(_ bundleId: String, _ body: String) -> String? {
        guard CtrlScripts.safeBundleId(bundleId) else { return nil }
        return "tell application id \"\(bundleId)\"\n\(body)\nend tell"
    }

    /// Every tab of every window: `wid␟index␟title␟url␟js␞`. The page script runs only when `withPageScript`.
    public static func enumerateSource(bundleId: String, flavor: CtrlTabFlavor, withPageScript: Bool = true) -> String? {
        let title = flavor == .safari ? "name" : "title"
        let js: String
        if withPageScript, let lit = literal(unsavedJS) {
            js = flavor == .safari ? "set r to (do JavaScript \(lit) in t)" : "set r to (execute t javascript \(lit))"
        } else {
            js = "set r to \"?\""
        }
        let body = """
        set us to character id 31
        set rs to character id 30
        set out to ""
        repeat with w in windows
        try
        set wid to (id of w) as text
        set n to count of tabs of w
        repeat with i from 1 to n
        set t to tab i of w
        set ti to ""
        set u to ""
        set r to "?"
        try
        set ti to (\(title) of t) as text
        end try
        try
        set u to (URL of t) as text
        end try
        try
        \(js)
        end try
        if r is missing value then set r to "?"
        set out to out & wid & us & i & us & ti & us & u & us & (r as text) & rs
        end repeat
        end try
        end repeat
        return out
        """
        return tell(bundleId, body)
    }

    /// The enumeration's output → rows; nil if any record is malformed (then nothing is known, the host asks).
    public static func parse(_ out: String) -> [CtrlTabRow]? {
        var rows: [CtrlTabRow] = []
        for rec in out.components(separatedBy: record) where !rec.isEmpty {
            let f = rec.components(separatedBy: field)
            guard f.count == 5, let w = Int(f[0]), let i = Int(f[1]), i >= 1 else { return nil }
            rows.append(CtrlTabRow(windowId: w, index: i, title: f[2], url: f[3],
                                   unsaved: f[4] == "1" ? true : f[4] == "0" ? false : nil))
        }
        return rows
    }

    /// Chromium's English pinned marker. Other UI languages read as "can't tell" (`english` false).
    static let chromiumPinned = [" - pinned", " (pinned)", ", pinned"]

    /// One AX tab against the scripted title at the same position: pinned true / false, or nil if it can't tell.
    public static func pinned(_ ax: CtrlAXTab, title: String, flavor: CtrlTabFlavor, english: Bool) -> Bool? {
        switch flavor {
        case .safari:
            switch ax.roleDescription?.lowercased() {
            case "pinned tab": return true
            case "tab": return false
            default: return nil
            }
        case .chromium:
            if ax.name == title { return false }
            guard !title.isEmpty, ax.name.hasPrefix(title) else { return nil }
            let rest = String(ax.name.dropFirst(title.count)).lowercased()
            if chromiumPinned.contains(where: { rest.hasPrefix($0) || rest.contains($0) }) { return true }
            // Another status suffix ("- audio playing"): not pinned, but only if the label is English.
            return english && rest.hasPrefix(" - ") ? false : nil
        }
    }

    /// Does an AX strip line up with a scripted window's titles, one to one?
    static func lines(_ strip: [CtrlAXTab], up titles: [String], flavor: CtrlTabFlavor) -> Bool {
        guard strip.count == titles.count, !strip.isEmpty else { return false }
        var compared = 0
        for (ax, t) in zip(strip, titles) where !ax.name.isEmpty && !t.isEmpty {
            let same = flavor == .safari ? ax.name == t : ax.name.hasPrefix(t)
            if !same { return false }
            compared += 1
        }
        return compared > 0
    }

    /// Rows + the AX tab strips of the browser's windows → tabs with `pinned`. A window whose strip doesn't line up
    /// with exactly one AX strip gets pinned nil throughout; so does a strip whose pinned tabs don't lead it.
    public static func join(_ rows: [CtrlTabRow], strips: [[CtrlAXTab]], flavor: CtrlTabFlavor, english: Bool) -> [CtrlTab] {
        var out: [CtrlTab] = []
        var order: [Int] = []
        var byWindow: [Int: [CtrlTabRow]] = [:]
        for r in rows {
            if byWindow[r.windowId] == nil { order.append(r.windowId) }
            byWindow[r.windowId, default: []].append(r)
        }
        for w in order {
            let tabs = byWindow[w]!.sorted { $0.index < $1.index }
            let titles = tabs.map(\.title)
            let matches = strips.filter { lines($0, up: titles, flavor: flavor) }
            var pins: [Bool?] = Array(repeating: nil, count: tabs.count)
            if matches.count == 1 {
                pins = zip(matches[0], titles).map { pinned($0, title: $1, flavor: flavor, english: english) }
                // Pinned tabs lead the strip: a pinned tab after an unpinned one means we misread it.
                if let firstFree = pins.firstIndex(of: false), pins[firstFree...].contains(true) {
                    pins = Array(repeating: nil, count: tabs.count)
                }
            }
            out += zip(tabs, pins).map { CtrlTab(row: $0, pinned: $1) }
        }
        return out
    }

    /// The tabs a verb closes: `close_saved_tabs` → known unpinned and known untouched; `close_unpinned_tabs` →
    /// known unpinned. Anything ctrl can't tell about stays.
    public static func targets(_ tabs: [CtrlTab], verb: String) -> [CtrlTab] {
        switch verb {
        case "close_saved_tabs": return tabs.filter { $0.pinned == false && $0.unsaved == false }
        case "close_unpinned_tabs": return tabs.filter { $0.pinned == false }
        default: return []
        }
    }

    /// The checks, blank pages not counted. `saved_tabs`: what `close_saved_tabs` would still close (held tabs stay
    /// open by design). `unpinned_tabs`: every tab not known pinned, so a tab ctrl couldn't tell about keeps the
    /// approved close from verifying (the host then asks instead of reporting a close that didn't happen).
    public static func count(_ tabs: [CtrlTab], check: String) -> Int? {
        switch check {
        case "saved_tabs": return targets(tabs, verb: "close_saved_tabs").filter { !$0.blank }.count
        case "unpinned_tabs": return tabs.filter { $0.pinned != true && !$0.blank }.count
        default: return nil
        }
    }

    /// Close `targets`, highest index first in each window so the others keep their place, each only if its URL is
    /// still the one ctrl read (a tab that moved or navigated in between stays). Never `saving no`, never activate.
    public static func closeSource(bundleId: String, targets: [CtrlTab]) -> String? {
        var lines: [String] = []
        for t in targets.sorted(by: { ($0.row.windowId, -$0.row.index) < ($1.row.windowId, -$1.row.index) }) {
            guard let u = literal(t.row.url) else { continue }
            let tab = "tab \(t.row.index) of window id \(t.row.windowId)"
            lines.append("try\nif ((URL of \(tab)) as text) is \(u) then close \(tab)\nend try")
        }
        guard !lines.isEmpty else { return nil }
        return tell(bundleId, lines.joined(separator: "\n"))
    }

    /// The preview items (titles leave only in this reply; the host hashes them).
    public static func preview(_ tabs: [CtrlTab]) -> [CtrlPreviewItem] {
        tabs.map { CtrlPreviewItem(title: $0.row.title, unsaved: $0.unsaved, pinned: $0.pinned) }
    }
}
