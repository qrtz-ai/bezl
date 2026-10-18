#if os(macOS)
import AppKit
import ApplicationServices
import Foundation
import QrtzMacShared
import QrtzNano
import QrtzShared

/// Thin, failure-tolerant wrappers over the AX C API.
public enum AX {
    public static func element(_ v: CFTypeRef?) -> AXUIElement? {
        guard let v, CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
        return (v as! AXUIElement)
    }

    public static func raw(_ el: AXUIElement, _ name: String) -> CFTypeRef? {
        var v: CFTypeRef?
        return AXUIElementCopyAttributeValue(el, name as CFString, &v) == .success ? v : nil
    }

    public static func string(_ el: AXUIElement, _ name: String) -> String? {
        guard let v = raw(el, name), CFGetTypeID(v) == CFStringGetTypeID() else { return nil }
        return (v as! CFString) as String
    }

    public static func elements(_ el: AXUIElement, _ name: String) -> [AXUIElement] {
        guard let v = raw(el, name), CFGetTypeID(v) == CFArrayGetTypeID() else { return [] }
        return (((v as! CFArray) as NSArray) as [AnyObject]).compactMap { element($0) }
    }

    public static func url(_ el: AXUIElement) -> String? {
        guard let v = raw(el, kAXURLAttribute) else { return nil }
        if CFGetTypeID(v) == CFURLGetTypeID() { return ((v as! CFURL) as URL).absoluteString }
        if CFGetTypeID(v) == CFStringGetTypeID() { return (v as! CFString) as String }
        return nil
    }

    public static let snapshotAttrs = [kAXRoleAttribute, kAXSubroleAttribute, kAXTitleAttribute, kAXValueAttribute, kAXDescriptionAttribute, kAXIdentifierAttribute, kAXChildrenAttribute] as [String]

    /// Copies an AX subtree into a value type, bounded by depth and node count (AX calls are synchronous IPC into the
    /// target app; a full Meet DOM can be tens of thousands of nodes).
    public static func snapshot(_ root: AXUIElement, maxDepth: Int = 40, budget: Int = 4000) -> AXNode {
        var remaining = budget
        func copy(_ el: AXUIElement, _ depth: Int) -> AXNode {
            remaining -= 1
            var values: CFArray?
            let names = snapshotAttrs as CFArray
            guard AXUIElementCopyMultipleAttributeValues(el, names, AXCopyMultipleAttributeOptions(rawValue: 0), &values) == .success,
                  let values
            else { return AXNode(role: "AXUnknown") }
            let arr = (values as NSArray) as [AnyObject]
            guard arr.count == snapshotAttrs.count else { return AXNode(role: "AXUnknown") }
            func str(_ i: Int) -> String? {
                let v = arr[i] as CFTypeRef
                return CFGetTypeID(v) == CFStringGetTypeID() ? ((v as! CFString) as String) : nil
            }
            var node = AXNode(role: str(0) ?? "AXUnknown", subrole: str(1), title: str(2), value: str(3), desc: str(4), identifier: str(5))
            let kids = arr[6] as CFTypeRef
            if depth < maxDepth, CFGetTypeID(kids) == CFArrayGetTypeID() {
                for k in (((kids as! CFArray) as NSArray) as [AnyObject]) {
                    guard remaining > 0, let ke = element(k) else { break }
                    node.children.append(copy(ke, depth + 1))
                }
            }
            return node
        }
        return copy(root, 0)
    }

    /// Breadth-first search for the first AXWebArea (the page content of a browser window).
    public static func webArea(in window: AXUIElement, maxNodes: Int = 400) -> AXUIElement? {
        var queue = [window]
        var seen = 0
        while !queue.isEmpty, seen < maxNodes {
            let el = queue.removeFirst()
            seen += 1
            if string(el, kAXRoleAttribute) == "AXWebArea" { return el }
            queue += elements(el, kAXChildrenAttribute)
        }
        return nil
    }
}

/// Watches running meeting apps through Accessibility: polls once a second (robust to apps that don't post
/// notifications, e.g. Chrome web content) and scrapes immediately on AXObserver notifications from the live meeting.
public final class AXMeetingSource: MeetingSource, @unchecked Sendable {
    private let queue = DispatchQueue(label: "qrtz.ax")
    private var emit: (@Sendable (HelperMessage) -> Void)?
    private var clock: (@Sendable () -> Double)?
    private var timer: DispatchSourceTimer?
    private let tracker = MeetingTracker()
    private let differ = ScrapeDiffer()
    private var windowsByKey: [String: (pid: pid_t, window: AXUIElement, platform: MeetingPlatform, browser: Bool)] = [:]
    private var activeKey: String?
    private var observers: [pid_t: AXObserver] = [:]
    private var manualAXEnabled = Set<pid_t>()
    private var scrapeScheduled = false
    private var warnedTrust = false
    private let context: MeetingContext
    public init(context: MeetingContext) { self.context = context }

    public static let chromium: Set<String> = ["com.google.Chrome", "com.google.Chrome.beta", "company.thebrowser.Browser", "com.microsoft.edgemac", "com.brave.Browser"]
    public static let watched: Set<String> = Set(MeetingClassifier.browsers.keys).union([MeetingClassifier.zoomApp]).union(MeetingClassifier.teamsApps)

    public func start(emit: @escaping @Sendable (HelperMessage) -> Void, clock: @escaping @Sendable () -> Double) {
        queue.async {
            self.emit = emit
            self.clock = clock
            let t = DispatchSource.makeTimerSource(queue: self.queue)
            t.schedule(deadline: .now() + 0.5, repeating: 1.0, leeway: .milliseconds(200))
            t.setEventHandler { [weak self] in self?.poll() }
            t.resume()
            self.timer = t
        }
    }

    public func stop() {
        queue.sync {
            timer?.cancel(); timer = nil
            for (_, o) in observers { CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(o), .defaultMode) }
            observers.removeAll()
        }
    }

    private func poll() {
        guard AXIsProcessTrusted() else {
            if !warnedTrust { warnedTrust = true; emit?(.log(level: .warn, msg: "accessibility not granted; meeting detection paused")) }
            return
        }
        warnedTrust = false
        var infos: [WindowInfo] = []
        var found: [String: (pid: pid_t, window: AXUIElement, platform: MeetingPlatform, browser: Bool)] = [:]
        for app in NSWorkspace.shared.runningApplications {
            guard let bid = app.bundleIdentifier, Self.watched.contains(bid) else { continue }
            let pid = app.processIdentifier
            let axApp = AXUIElementCreateApplication(pid)
            AXUIElementSetMessagingTimeout(axApp, 0.5)
            if Self.chromium.contains(bid), !manualAXEnabled.contains(pid) {
                // Chromium only builds its web accessibility tree when an assistive client asks for it.
                AXUIElementSetAttributeValue(axApp, "AXManualAccessibility" as CFString, kCFBooleanTrue)
                manualAXEnabled.insert(pid)
            }
            let isBrowser = MeetingClassifier.browsers[bid] != nil
            for win in AX.elements(axApp, kAXWindowsAttribute) {
                let title = AX.string(win, kAXTitleAttribute) ?? ""
                var url: String?
                if isBrowser {
                    let l = title.lowercased()
                    guard l.contains("meet") || l.contains("zoom") || l.contains("teams") else { continue }
                    url = AX.webArea(in: win).flatMap(AX.url)
                }
                let info = WindowInfo(pid: pid, bundleId: bid, appName: app.localizedName ?? bid, title: title, url: url)
                infos.append(info)
                if let m = MeetingClassifier.classify(info) { found[m.key] = (pid, win, m.platform, isBrowser) }
            }
        }
        for (k, v) in found { windowsByKey[k] = v }
        let now = clock?() ?? 0
        for e in tracker.update(windows: infos, nowMs: now) {
            emit?(e)
            switch e {
            case .meetingDetected(let key, _, _, _):
                activeKey = key
                differ.reset()
                context.reset()
                if let w = windowsByKey[key] { observe(pid: w.pid, window: w.window) }
            case .meetingEnded(let key):
                windowsByKey[key] = nil
                if activeKey == key { activeKey = tracker.current.last?.key; differ.reset() }
            default: break
            }
        }
        scrape()
    }

    private func scrape() {
        scrapeScheduled = false
        guard let key = activeKey, let w = windowsByKey[key] else { return }
        let root = w.browser ? (AX.webArea(in: w.window) ?? w.window) : w.window
        let snap = AX.snapshot(root)
        let scrape = AXScraper.scrape(platform: w.platform, window: snap)
        context.update(selfName: scrape.selfName, activeSpeaker: scrape.activeSpeaker)
        for e in differ.diff(scrape, atMs: clock?() ?? 0) { emit?(e) }
    }

    /// Debounced scrape triggered by AX notifications (speaker highlight changes, captions updating).
    fileprivate func notified() {
        queue.async {
            guard !self.scrapeScheduled else { return }
            self.scrapeScheduled = true
            self.queue.asyncAfter(deadline: .now() + 0.15) { self.scrape() }
        }
    }

    private func observe(pid: pid_t, window: AXUIElement) {
        guard observers[pid] == nil else { return }
        var obs: AXObserver?
        let cb: AXObserverCallback = { _, _, _, refcon in
            guard let refcon else { return }
            Unmanaged<AXMeetingSource>.fromOpaque(refcon).takeUnretainedValue().notified()
        }
        guard AXObserverCreate(pid, cb, &obs) == .success, let obs else { return }
        let me = Unmanaged.passUnretained(self).toOpaque()
        let app = AXUIElementCreateApplication(pid)
        for n in [kAXValueChangedNotification, kAXTitleChangedNotification, kAXFocusedWindowChangedNotification, kAXUIElementDestroyedNotification, kAXCreatedNotification, "AXLiveRegionChanged"] {
            _ = AXObserverAddNotification(obs, n == kAXFocusedWindowChangedNotification ? app : window, n as CFString, me)
        }
        CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(obs), .defaultMode)
        observers[pid] = obs
    }
}

/// `qrtz-helper --dump-ax`: prints AX snapshots of meeting windows as JSON, for recording test fixtures.
public enum AXDump {
    public static func json(_ n: AXNode) -> JSONValue {
        var o: [String: JSONValue] = ["role": .string(n.role)]
        for (k, v) in [("subrole", n.subrole), ("title", n.title), ("value", n.value), ("desc", n.desc), ("identifier", n.identifier)] {
            if let v, !v.isEmpty { o[k] = .string(v) }
        }
        if !n.children.isEmpty { o["children"] = .array(n.children.map(json)) }
        return .object(o)
    }

    public static func run() {
        guard AXIsProcessTrusted() else { logErr("grant Accessibility to the terminal first"); return }
        for app in NSWorkspace.shared.runningApplications {
            guard let bid = app.bundleIdentifier, AXMeetingSource.watched.contains(bid) else { continue }
            let axApp = AXUIElementCreateApplication(app.processIdentifier)
            if AXMeetingSource.chromium.contains(bid) {
                AXUIElementSetAttributeValue(axApp, "AXManualAccessibility" as CFString, kCFBooleanTrue)
                Thread.sleep(forTimeInterval: 1)
            }
            for win in AX.elements(axApp, kAXWindowsAttribute) {
                let title = AX.string(win, kAXTitleAttribute) ?? ""
                let root = AX.webArea(in: win) ?? win
                let info = WindowInfo(pid: app.processIdentifier, bundleId: bid, appName: app.localizedName ?? bid, title: title, url: AX.url(root))
                let meeting = MeetingClassifier.classify(info)
                let snap = AX.snapshot(root, budget: 20_000)
                let scrape = meeting.map { AXScraper.scrape(platform: $0.platform, window: snap) }
                let out: JSONValue = .object([
                    "bundleId": .string(bid), "title": .string(title), "url": info.url.map(JSONValue.string) ?? .null,
                    "meeting": meeting.map { JSONValue.string($0.key) } ?? .null,
                    "participants": scrape?.participants.map { JSONValue.array($0.map(JSONValue.string)) } ?? .null,
                    "activeSpeaker": scrape?.activeSpeaker.map(JSONValue.string) ?? .null,
                    "tree": json(snap),
                ])
                print(out.serialized())
            }
        }
    }
}
#endif
