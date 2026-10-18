#if os(macOS)
// The macOS ctrl backend (slice 5b, ADR 0110): what `CtrlServer` calls after its policy re-check. It observes
// without prompting (no TCC dialog from a background run) and acts only through the background rungs of the ladder:
// workspace opens with `activates = false`, Apple Events that can't activate, AX actions on a background app, key
// events posted to one pid with a private event source. Nothing here activates an app, moves the cursor or posts
// to the global event stream (ADR 0100); `restore` is the only activation and only puts the user's app back.
//
// Titles leave here only inside a snapshot reply (the host hashes them before anything is logged); AXValue and
// secure fields are never read.
import AppKit
import ApplicationServices
import CoreML
import CoreServices
import Foundation

public final class MacCtrlBackend: CtrlBackend {
    /// Opaque element ids ("e:<pid>:<n>") from the snapshots → their AX handles. A new top-level snapshot of an app
    /// drops that app's old handles; the whole cache is bounded.
    private var handles: [String: (pid: Int32, el: AXUIElement)] = [:]
    private var nextHandle = 0
    private let lock = NSLock()
    static let maxHandles = 512
    static let maxNodes = 40
    let layaRuntime = CtrlLayaRuntime()

    public init() {}

    // MARK: apps

    func runningApp(_ bundleId: String) -> NSRunningApplication? {
        NSWorkspace.shared.runningApplications.first { $0.bundleIdentifier == bundleId && !$0.isTerminated }
    }

    public func frontmostPid() -> Int32? { NSWorkspace.shared.frontmostApplication?.processIdentifier }
    public func pid(of app: String) -> Int32? { runningApp(app)?.processIdentifier }

    static let probeURL = URL(string: "https://example.com")!

    /// The default `https` handler, from LaunchServices.
    static func defaultBrowser() -> String? {
        guard let u = LSCopyDefaultApplicationURLForURL(probeURL as CFURL, .all, nil)?.takeRetainedValue() else { return nil }
        return Bundle(url: u as URL)?.bundleIdentifier
    }

    static func browserIds() -> Set<String> {
        Set(NSWorkspace.shared.urlsForApplications(toOpen: probeURL).compactMap { Bundle(url: $0)?.bundleIdentifier })
    }

    static let appDirs: [URL] = {
        let fm = FileManager.default
        var dirs = [URL(fileURLWithPath: "/Applications"), URL(fileURLWithPath: "/Applications/Utilities"),
                    URL(fileURLWithPath: "/System/Applications"), URL(fileURLWithPath: "/System/Applications/Utilities")]
        dirs.append(fm.homeDirectoryForCurrentUser.appendingPathComponent("Applications"))
        return dirs
    }()

    public func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) {
        let browsers = Self.browserIds()
        var byId: [String: CtrlCatalogApp] = [:]
        let running = NSWorkspace.shared.runningApplications.filter { $0.activationPolicy == .regular && !$0.isTerminated }
        let runningIds = Set(running.compactMap(\.bundleIdentifier))
        let fm = FileManager.default
        for dir in Self.appDirs {
            guard let items = try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]) else { continue }
            for u in items where u.pathExtension == "app" {
                guard let b = Bundle(url: u), let id = b.bundleIdentifier, byId[id] == nil else { continue }
                let name = (b.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String)
                    ?? (b.object(forInfoDictionaryKey: "CFBundleName") as? String)
                    ?? u.deletingPathExtension().lastPathComponent
                byId[id] = CtrlCatalogApp(bundleId: id, name: name, browser: browsers.contains(id), running: runningIds.contains(id))
            }
        }
        for app in running {
            guard let id = app.bundleIdentifier, byId[id] == nil else { continue }
            byId[id] = CtrlCatalogApp(bundleId: id, name: app.localizedName ?? id, browser: browsers.contains(id), running: true)
        }
        return (byId.values.sorted { $0.bundleId < $1.bundleId }, Self.defaultBrowser())
    }

    // MARK: probe

    /// Apple Events consent for one running app, checked without asking (`askUserIfNeeded: false`).
    static func automation(_ bundleId: String) -> String? {
        var addr = AEAddressDesc()
        let bytes = Array(bundleId.utf8)
        let made = bytes.withUnsafeBytes { AECreateDesc(DescType(typeApplicationBundleID), $0.baseAddress, $0.count, &addr) }
        guard made == 0 else { return nil }
        defer { AEDisposeDesc(&addr) }
        switch AEDeterminePermissionToAutomateTarget(&addr, AEEventClass(typeWildCard), AEEventID(typeWildCard), false) {
        case 0: return "granted"
        case -1743: return "denied" // errAEEventNotPermitted
        case -1744: return "not_asked" // errAEEventWouldRequireUserConsent
        default: return nil // procNotFound and friends: not running, nothing to say
        }
    }

    /// Apps whose scripting verbs ctrl uses: browsers and TextEdit, only while running (no launch to ask).
    static let scriptable: Set<String> = ["com.apple.TextEdit"]

    public func probe() throws -> (env: [String: Any], ui: [String: Any]) {
        let running = NSWorkspace.shared.runningApplications.filter { $0.activationPolicy == .regular && !$0.isTerminated }
        let ids = running.compactMap(\.bundleIdentifier)
        let browsers = Self.browserIds()
        var automation: [String: String] = [:]
        for id in ids where browsers.contains(id) || Self.scriptable.contains(id) {
            if let c = Self.automation(id) { automation[id] = c }
        }
        var env: [String: Any] = [
            "secure_input": CtrlProbe.secureInput,
            // Files: never probed by touching a protected folder (that would prompt); slice 7 decides how.
            "files": "not_asked",
            "accessibility": CtrlProbe.axTrusted ? "granted" : "not_asked",
            "post_event": CtrlProbe.postEventAllowed ? "granted" : "not_asked",
            "automation": automation,
            "quarantined": [Any](),
            "running_apps": ids.sorted(),
        ]
        env["frontmost_app"] = NSWorkspace.shared.frontmostApplication?.bundleIdentifier as Any? ?? NSNull()
        env["user_idle_ms"] = CtrlProbe.hidIdleMs().map { NSNumber(value: $0) } as Any? ?? NSNull()
        let data = try JSONEncoder().encode(CtrlProbe.snapshot())
        let ui = (try JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        return (env, ui)
    }

    // MARK: AX snapshot

    static func string(_ el: AXUIElement, _ name: String) -> String? {
        guard let v = CtrlActions.raw(el, name), CFGetTypeID(v) == CFStringGetTypeID() else { return nil }
        return (v as! CFString) as String
    }

    static func children(_ el: AXUIElement) -> [AXUIElement] {
        guard let v = CtrlActions.raw(el, kAXChildrenAttribute), CFGetTypeID(v) == CFArrayGetTypeID() else { return [] }
        return (((v as! CFArray) as NSArray) as [AnyObject]).compactMap { CtrlActions.element($0 as CFTypeRef) }
    }

    static func actions(_ el: AXUIElement) -> [String] {
        var names: CFArray?
        guard AXUIElementCopyActionNames(el, &names) == .success, let names else { return [] }
        return ((names as NSArray) as? [String]) ?? []
    }

    static func childCount(_ el: AXUIElement) -> Int {
        var n: CFIndex = 0
        return AXUIElementGetAttributeValueCount(el, kAXChildrenAttribute as CFString, &n) == .success ? n : 0
    }

    func remember(_ el: AXUIElement, pid: Int32) -> String {
        lock.lock(); defer { lock.unlock() }
        if handles.count >= Self.maxHandles { handles.removeAll() }
        nextHandle += 1
        let id = "e:\(pid):\(nextHandle)"
        handles[id] = (pid, el)
        return id
    }

    func handle(_ id: String) -> (pid: Int32, el: AXUIElement)? {
        lock.lock(); defer { lock.unlock() }
        return handles[id]
    }

    func forget(pid: Int32) {
        lock.lock(); defer { lock.unlock() }
        handles = handles.filter { $0.value.pid != pid }
    }

    public func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] {
        guard let pid = pid(of: app) else { throw CtrlFailure("app.not_running") }
        guard CtrlProbe.axTrusted else { throw CtrlFailure("ax.not_trusted") }
        let root: AXUIElement
        if let scope {
            guard let h = handle(scope), h.pid == pid else { throw CtrlFailure("ax.stale") }
            root = h.el
        } else {
            forget(pid: pid)
            let appEl = CtrlActions.appElement(pid)
            guard let w = CtrlActions.element(CtrlActions.raw(appEl, kAXFocusedWindowAttribute))
                ?? CtrlActions.element(CtrlActions.raw(appEl, kAXMainWindowAttribute))
                ?? CtrlActions.windows(pid).first
            else { return [] }
            root = w
        }
        return Self.children(root).prefix(Self.maxNodes).map { el in
            let role = CtrlActions.role(el)
            let subrole = Self.string(el, kAXSubroleAttribute)
            let secure = subrole == "AXSecureTextField" || role == "AXSecureTextField"
            let title = secure ? "" : (Self.string(el, kAXTitleAttribute) ?? Self.string(el, kAXDescriptionAttribute) ?? "")
            return CtrlAXNode(id: remember(el, pid: pid), role: role, subrole: subrole, title: title,
                              actions: Self.actions(el), children: Self.childCount(el))
        }
    }

    // MARK: act

    static func code(_ f: CtrlActions.Failure) -> CtrlFailure {
        switch f {
        case .notTrusted: return CtrlFailure("ax.not_trusted")
        case .ax: return CtrlFailure("ax.failed")
        case .noElement: return CtrlFailure("ax.stale")
        case .eventSource: return CtrlFailure("keys.no_event_source")
        case .refused: return CtrlFailure("policy.refused")
        }
    }

    /// Wait for a workspace completion handler (they run on an arbitrary queue; this is ctrl's own queue).
    static func wait(_ seconds: Double, _ start: (@escaping (Error?) -> Void) -> Void) throws {
        let done = DispatchSemaphore(value: 0)
        var err: Error?
        start { e in err = e; done.signal() }
        guard done.wait(timeout: .now() + seconds) == .success else { throw CtrlFailure("workspace.timeout") }
        if err != nil { throw CtrlFailure("workspace.failed") }
    }

    /// NSAppleScript belongs on the main thread (the helper's main run loop is running).
    static func runScript(_ source: String) throws -> NSAppleEventDescriptor {
        var result: NSAppleEventDescriptor?
        var failed = false
        let run = {
            var info: NSDictionary?
            let r: NSAppleEventDescriptor? = NSAppleScript(source: source)?.executeAndReturnError(&info)
            // Any error info (including -1743, Apple Events not permitted) is a failure, whatever came back.
            failed = info != nil || r == nil
            result = failed ? nil : r
        }
        if Thread.isMainThread { run() } else { DispatchQueue.main.sync(execute: run) }
        guard let r = result, !failed else { throw CtrlFailure("script.failed") }
        return r
    }

    func script(_ verb: String, app: String) throws -> NSAppleEventDescriptor {
        // `tell application id` launches an app that isn't running: never.
        guard runningApp(app) != nil else { throw CtrlFailure("app.not_running") }
        guard let src = CtrlScripts.source(verb: verb, bundleId: app) else { throw CtrlFailure("policy.refused") }
        return try Self.runScript(src)
    }

    public func act(_ route: CtrlRoute, content: [String: String]) throws {
        switch route {
        case .files: throw CtrlFailure("files.not_yet")
        case .takesOver: throw CtrlFailure("policy.refused")
        case let .openURL(app, ref):
            // Only into an app that is already running: a cold launch would come to the front (run 1's finding).
            guard let running = runningApp(app), let appURL = running.bundleURL else { throw CtrlFailure("app.not_running") }
            guard let s = content[ref], let url = URL(string: s), ["http", "https"].contains(url.scheme?.lowercased() ?? "")
            else { throw CtrlFailure("content.missing") }
            try Self.wait(5) { done in CtrlActions.open([url], withApp: appURL) { _, e in done(e) } }
        case let .launchHidden(app):
            // Already running: it is open, and opening it again with `hides` would hide the user's window (ADR 0128).
            if runningApp(app) != nil { return }
            guard let appURL = NSWorkspace.shared.urlForApplication(withBundleIdentifier: app) else { throw CtrlFailure("app.not_installed") }
            let c = CtrlActions.backgroundConfiguration()
            c.hides = true
            try Self.wait(10) { done in NSWorkspace.shared.openApplication(at: appURL, configuration: c) { _, e in done(e) } }
        case let .scripting(app, verb) where CtrlRoute.tabVerbs.contains(verb):
            try closeTabs(app: app, verb: verb)
        case let .scripting(app, verb):
            _ = try script(verb, app: app)
        case let .axPerform(app, role, element, action):
            guard let pid = pid(of: app) else { throw CtrlFailure("app.not_running") }
            guard let h = handle(element), h.pid == pid, CtrlActions.role(h.el) == role else { throw CtrlFailure("ax.stale") }
            do { try CtrlActions.perform(action, on: h.el) } catch let f as CtrlActions.Failure { throw Self.code(f) }
        case let .axSet(app, role, element, attribute):
            guard let pid = pid(of: app) else { throw CtrlFailure("app.not_running") }
            guard let h = handle(element), h.pid == pid, CtrlActions.role(h.el) == role else { throw CtrlFailure("ax.stale") }
            guard let value = content["value"] else { throw CtrlFailure("content.missing") }
            do { try CtrlActions.set(attribute, value as CFString, on: h.el) } catch let f as CtrlActions.Failure { throw Self.code(f) }
        case let .pidKey(app, combo):
            guard let pid = pid(of: app) else { throw CtrlFailure("app.not_running") }
            // Without the grant postToPid drops the event silently; say so instead of "succeeding".
            guard CtrlProbe.postEventAllowed else { throw CtrlFailure("keys.not_granted") }
            guard let k = CtrlKeyCombo.parse(combo) else { throw CtrlFailure("keys.unknown") }
            var flags: CGEventFlags = []
            if k.command { flags.insert(.maskCommand) }
            if k.shift { flags.insert(.maskShift) }
            if k.option { flags.insert(.maskAlternate) }
            if k.control { flags.insert(.maskControl) }
            do { try CtrlActions.postKey(CGKeyCode(k.keyCode), flags: flags, combo: combo, to: pid) } catch let f as CtrlActions.Failure { throw Self.code(f) }
        }
    }

    // MARK: verify

    public func measure(check: String, app: String) -> Int? {
        switch check {
        case "app_running": return runningApp(app) == nil ? 0 : 1
        case "tab_count":
            return (try? script("count_tabs", app: app)).map { Int($0.int32Value) }
        case "saved_tabs", "unpinned_tabs":
            return (try? tabs(app: app)).flatMap { CtrlTabs.count($0, check: check) }
        case "window_count":
            guard let pid = pid(of: app) else { return nil }
            if let n = try? script("count_windows", app: app) { return Int(n.int32Value) }
            return CtrlProbe.axTrusted ? CtrlActions.windows(pid).count : nil
        default: return nil // ax_scope is verified by a re-snapshot and obs.diff on the host
        }
    }

    public func restore(frontmostPid: Int32?) -> Bool {
        guard let pid = frontmostPid else { return false }
        return CtrlActions.restoreFrontmost(pid)
    }

    // MARK: tabs (run 12, ADR 0119)

    /// The confirm gate's preview. Tabs only: windows stay on the host's count fallback.
    public func preview(app: String, what: String) throws -> [CtrlPreviewItem] {
        guard what == "tabs" else { throw CtrlFailure("preview.unsupported") }
        return CtrlTabs.preview(try tabs(app: app))
    }

    /// Every tab of a running Safari / Chromium browser with what ctrl can tell about it. One Apple Event script
    /// (titles, URLs, the read-only page script) plus an AX read of the tab bars: no activation, no tab selected,
    /// nothing focused. Without the AX grant every `pinned` is nil (so nothing closes unasked).
    func tabs(app: String) throws -> [CtrlTab] {
        guard let flavor = CtrlTabFlavor.of(app) else { throw CtrlFailure("tabs.unsupported") }
        guard let pid = pid(of: app) else { throw CtrlFailure("app.not_running") }
        guard let src = CtrlTabs.enumerateSource(bundleId: app, flavor: flavor) else { throw CtrlFailure("policy.refused") }
        guard let out = try Self.runScript(src).stringValue, let rows = CtrlTabs.parse(out) else { throw CtrlFailure("tabs.unreadable") }
        let strips = CtrlProbe.axTrusted ? CtrlActions.windows(pid).flatMap(Self.tabStrips) : []
        let english = Locale.preferredLanguages.first?.lowercased().hasPrefix("en") ?? false
        return CtrlTabs.join(rows, strips: strips, flavor: flavor, english: english)
    }

    /// The tab bars under one window (AXTabGroup → its AXRadioButton tabs, one AXGroup level flattened for
    /// Chromium's tab groups), breadth-first and bounded. Reads only roles, titles and role descriptions.
    static func tabStrips(_ window: AXUIElement) -> [[CtrlAXTab]] {
        var strips: [[CtrlAXTab]] = []
        var queue: [(AXUIElement, Int)] = [(window, 0)]
        var seen = 0
        while !queue.isEmpty, seen < 600 {
            let (el, depth) = queue.removeFirst()
            seen += 1
            if CtrlActions.role(el) == "AXTabGroup" {
                var tabs: [CtrlAXTab] = []
                for c in children(el) {
                    let r = CtrlActions.role(c)
                    let members = r == "AXRadioButton" ? [c] : r == "AXGroup" ? children(c).filter { CtrlActions.role($0) == "AXRadioButton" } : []
                    for t in members {
                        let name = [string(t, kAXTitleAttribute), string(t, kAXDescriptionAttribute)].compactMap { $0 }.first { !$0.isEmpty } ?? ""
                        tabs.append(CtrlAXTab(name: name, roleDescription: string(t, kAXRoleDescriptionAttribute)))
                    }
                }
                if !tabs.isEmpty { strips.append(tabs); continue }
            }
            // Never into page content: the tab bar is browser chrome.
            if depth < 8 { queue += children(el).filter { CtrlActions.role($0) != "AXWebArea" }.map { ($0, depth + 1) } }
        }
        return strips
    }

    /// `close_saved_tabs` / `close_unpinned_tabs`: read the tabs again right now, close only the ones the verb
    /// covers (each re-checked by URL inside the script), never a tab ctrl can't tell about. Nothing to close is a
    /// success; the host's check then verifies.
    func closeTabs(app: String, verb: String) throws {
        let targets = CtrlTabs.targets(try tabs(app: app), verb: verb)
        guard !targets.isEmpty else { return }
        guard let src = CtrlTabs.closeSource(bundleId: app, targets: targets) else { throw CtrlFailure("tabs.unreadable") }
        _ = try Self.runScript(src)
    }

    // MARK: Laya

    public func laya(state: String, question: String, options: [String]) throws -> [Double] {
        try layaRuntime.choice(model: nil, state: state, question: question, options: options)
    }

    public func laya(model: String?, state: String, question: String, options: [String]) throws -> [Double] {
        try layaRuntime.choice(model: model, state: state, question: question, options: options)
    }
}

/// The Core ML Laya runtime (ADR 0106 / 0109 / 0116). It runs only a model the user consented to download: the host
/// sends the installed model's path with each `ctrl.laya` after consent and never otherwise (`QRTZ_CTRL_LAYA_MODEL`
/// at launch is the developer override), and nothing here downloads anything. A `.mlmodelc` loads as is; an
/// `.mlpackage` / `.mlmodel` is compiled once into a sibling `.mlmodelc` (the persisted cache, reused while it is
/// newer than its source). Without a usable model every question answers `laya.no_model` / `laya.load_failed` and
/// the host keeps the heuristic scorer for that question.
final class CtrlLayaRuntime {
    private var models: [String: MLModel] = [:]
    private var failed: Set<String> = []
    private let lock = NSLock()
    static let kinds: Set<String> = ["mlmodelc", "mlpackage", "mlmodel"]

    /// The compiled cache for a source model: `<dir>/<name>.mlmodelc`.
    static func compiledURL(for source: URL) -> URL {
        source.deletingPathExtension().appendingPathExtension("mlmodelc")
    }

    static func modified(_ u: URL) -> Date? {
        (try? FileManager.default.attributesOfItem(atPath: u.path))?[.modificationDate] as? Date
    }

    /// Compile an `.mlpackage` / `.mlmodel` into its cache unless the cache is already newer than the source.
    static func compiled(_ source: URL) throws -> URL {
        if source.pathExtension == "mlmodelc" { return source }
        let cache = compiledURL(for: source)
        if let c = modified(cache), let s = modified(source), c >= s { return cache }
        let tmp = try MLModel.compileModel(at: source)
        let fm = FileManager.default
        if fm.fileExists(atPath: cache.path) {
            _ = try fm.replaceItemAt(cache, withItemAt: tmp)
        } else {
            try fm.moveItem(at: tmp, to: cache)
        }
        return cache
    }

    func load(_ model: String?) throws -> MLModel {
        let env = ProcessInfo.processInfo.environment["QRTZ_CTRL_LAYA_MODEL"]
        guard let path = (model?.isEmpty == false ? model : env), !path.isEmpty else { throw CtrlFailure("laya.no_model") }
        lock.lock(); defer { lock.unlock() }
        if let m = models[path] { return m }
        if failed.contains(path) { throw CtrlFailure("laya.load_failed") }
        let url = URL(fileURLWithPath: path)
        guard Self.kinds.contains(url.pathExtension), FileManager.default.fileExists(atPath: path) else { throw CtrlFailure("laya.no_model") }
        let c = MLModelConfiguration()
        c.computeUnits = .all // L512: GPU 9 ms vs CPU+ANE 27.5 ms warm (laya-coreml README, ADR 0129)
        do {
            let m = try MLModel(contentsOf: try Self.compiled(url), configuration: c)
            models[path] = m
            return m
        } catch {
            failed.insert(path)
            throw CtrlFailure("laya.load_failed")
        }
    }

    private var tokenizers: [String: CtrlLayaTokenizer] = [:]

    /// The tokenizer next to the model (`<set dir>/tokenizer.json`, ADR 0129) or `QRTZ_CTRL_LAYA_TOKENIZER`; read
    /// once per path. A missing or unreadable file answers `laya.no_tokenizer` and the host keeps the heuristic.
    func tokenizer(for modelPath: String) throws -> CtrlLayaTokenizer {
        let env = ProcessInfo.processInfo.environment["QRTZ_CTRL_LAYA_TOKENIZER"]
        let path = (env?.isEmpty == false ? env! : URL(fileURLWithPath: modelPath).deletingLastPathComponent().appendingPathComponent("tokenizer.json").path)
        lock.lock(); defer { lock.unlock() }
        if let t = tokenizers[path] { return t }
        guard let t = try? CtrlLayaTokenizer(contentsOf: URL(fileURLWithPath: path)) else { throw CtrlFailure("laya.no_tokenizer") }
        tokenizers[path] = t
        return t
    }

    static func array(_ shape: [Int], int32 xs: [Int32]) throws -> MLMultiArray {
        let a = try MLMultiArray(shape: shape.map { NSNumber(value: $0) }, dataType: .int32)
        for (i, x) in xs.enumerated() { a[i] = NSNumber(value: x) }
        return a
    }

    static func array(_ shape: [Int], float xs: [Float]) throws -> MLMultiArray {
        let a = try MLMultiArray(shape: shape.map { NSNumber(value: $0) }, dataType: .float32)
        for (i, x) in xs.enumerated() { a[i] = NSNumber(value: x) } // every slot: a new MLMultiArray is not zeroed
        return a
    }

    /// One typed `choice` pass of laya_multilingual_e8_L512_options32 (ADR 0129): the tokenizer and
    /// `CtrlLayaInputs` build input_ids / attention_mask / marker_map / question_type, the model answers
    /// `probabilities` [1, 32]; the first `options.count` slots, renormalised, are the answer. Any other interface
    /// answers `laya.bad_model`; more than 32 options, or none, `laya.bad_question`.
    func choice(model: String?, state: String, question: String, options: [String]) throws -> [Double] {
        let m = try load(model)
        let path = (model?.isEmpty == false ? model! : ProcessInfo.processInfo.environment["QRTZ_CTRL_LAYA_MODEL"] ?? "")
        let t = try tokenizer(for: path)
        guard let x = CtrlLayaInputs.build(tokenizer: t, state: state, question: question, options: options) else {
            throw CtrlFailure("laya.bad_question")
        }
        let L = CtrlLayaInputs.length, n = CtrlLayaInputs.names
        guard let input = try? MLDictionaryFeatureProvider(dictionary: [
            n.ids: MLFeatureValue(multiArray: try Self.array([1, L], int32: x.inputIds(pad: t.padId))),
            n.mask: MLFeatureValue(multiArray: try Self.array([1, L], int32: x.attentionMask)),
            n.markers: MLFeatureValue(multiArray: try Self.array([1, CtrlLayaInputs.maxOptions, L], float: x.markerMap)),
            n.type: MLFeatureValue(multiArray: try Self.array([1, 3], float: x.questionTypeVector)),
        ]),
            let out = try? m.prediction(from: input),
            let probs = out.featureValue(for: CtrlLayaInputs.outputName)?.multiArrayValue,
            let p = CtrlLayaInputs.optionProbabilities((0..<probs.count).map { probs[$0].doubleValue }, count: options.count)
        else { throw CtrlFailure("laya.bad_model") }
        return p
    }
}
#endif
