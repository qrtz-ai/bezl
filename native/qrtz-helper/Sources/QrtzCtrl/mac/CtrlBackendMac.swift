#if os(macOS)
// The macOS ctrl do backend (ADR 0214): what `CtrlServer` calls after its policy re-check. ctrl do works in the
// foreground with a stop key (amending ADR 0100): it may open and activate an app, press, focus and type into the
// elements of the accessibility tree it read, and send key combos to the app it brought forward. It never runs
// AppleScript, never clicks at coordinates and never moves the cursor.
//
// Reads go through `CtrlTreeReader` (CtrlTree.swift): a secure text field is skipped on its role alone, password and
// payment fields on their labels, and a value is read only for plain text inputs and a web area. Titles leave here
// only inside a `ctrl.tree.result` reply (the host hashes them before anything is logged); nothing is written to a
// log or to disk. Before acting, the element is looked up by the handle this helper created and its fingerprint is
// re-checked against the one the host saw (`act.stale`), and a secure field is refused again.
import AppKit
import Carbon.HIToolbox
import ApplicationServices
import CoreGraphics
import CoreML
import CoreServices
import Foundation

/// The accessibility API as a `CtrlAXSource`.
struct MacAXSource: CtrlAXSource {
    typealias Element = AXUIElement

    static func raw(_ el: AXUIElement, _ name: String) -> CFTypeRef? {
        var v: CFTypeRef?
        return AXUIElementCopyAttributeValue(el, name as CFString, &v) == .success ? v : nil
    }

    static func string(_ el: AXUIElement, _ name: String) -> String? {
        guard let v = raw(el, name), CFGetTypeID(v) == CFStringGetTypeID() else { return nil }
        return (v as! CFString) as String
    }

    static func element(_ v: CFTypeRef?) -> AXUIElement? {
        guard let v, CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
        return (v as! AXUIElement)
    }

    static func bool(_ el: AXUIElement, _ name: String) -> Bool? {
        guard let v = raw(el, name), CFGetTypeID(v) == CFBooleanGetTypeID() else { return nil }
        return CFBooleanGetValue((v as! CFBoolean))
    }

    static func int(_ el: AXUIElement, _ name: String) -> Int? {
        guard let v = raw(el, name), CFGetTypeID(v) == CFNumberGetTypeID() else { return nil }
        return ((v as! CFNumber) as NSNumber).intValue
    }

    func role(_ e: AXUIElement) -> String? { Self.string(e, kAXRoleAttribute) }
    func subrole(_ e: AXUIElement) -> String? { Self.string(e, kAXSubroleAttribute) }
    func title(_ e: AXUIElement) -> String {
        for a in [kAXTitleAttribute, kAXDescriptionAttribute, "AXLabelValue"] {
            if let s = Self.string(e, a), !s.isEmpty { return s }
        }
        return ""
    }
    func placeholder(_ e: AXUIElement) -> String { Self.string(e, "AXPlaceholderValue") ?? "" }
    func help(_ e: AXUIElement) -> String { Self.string(e, "AXHelp") ?? "" }
    func extraLabels(_ e: AXUIElement) -> [String] {
        var out: [String] = []
        if let t = Self.element(Self.raw(e, "AXTitleUIElement")) { out.append(title(t)); out.append(Self.string(t, kAXValueAttribute) ?? "") }
        if let d = Self.string(e, "AXDOMIdentifier") { out.append(d) }
        if let v = Self.raw(e, "AXDOMClassList"), CFGetTypeID(v) == CFArrayGetTypeID() {
            out += (((v as! CFArray) as NSArray) as? [String]) ?? []
        }
        return out
    }
    func identifier(_ e: AXUIElement) -> String { Self.string(e, "AXIdentifier") ?? "" }
    func pid(_ e: AXUIElement) -> Int32 { var p: pid_t = 0; AXUIElementGetPid(e, &p); return p }
    func parent(_ e: AXUIElement) -> AXUIElement? { Self.element(Self.raw(e, kAXParentAttribute)) }
    func value(_ e: AXUIElement) -> String? {
        if role(e) == "AXWebArea" {
            guard let v = Self.raw(e, "AXURL"), CFGetTypeID(v) == CFURLGetTypeID() else { return nil }
            return ((v as! CFURL) as URL).absoluteString
        }
        return Self.string(e, kAXValueAttribute)
    }
    func actions(_ e: AXUIElement) -> [String] {
        var names: CFArray?
        guard AXUIElementCopyActionNames(e, &names) == .success, let names else { return [] }
        return ((names as NSArray) as? [String]) ?? []
    }
    func enabled(_ e: AXUIElement) -> Bool { Self.bool(e, kAXEnabledAttribute) ?? true }
    func size(_ e: AXUIElement) -> [Double]? {
        guard let v = Self.raw(e, kAXSizeAttribute), CFGetTypeID(v) == AXValueGetTypeID() else { return nil }
        var s = CGSize.zero
        guard AXValueGetValue((v as! AXValue), .cgSize, &s) else { return nil }
        return [Double(s.width), Double(s.height)]
    }
    func children(_ e: AXUIElement) -> [AXUIElement] {
        guard let v = Self.raw(e, kAXChildrenAttribute), CFGetTypeID(v) == CFArrayGetTypeID() else { return [] }
        return (((v as! CFArray) as NSArray) as [AnyObject]).compactMap { Self.element($0 as CFTypeRef) }
    }
    func childCount(_ e: AXUIElement) -> Int {
        var n: CFIndex = 0
        return AXUIElementGetAttributeValueCount(e, kAXChildrenAttribute as CFString, &n) == .success ? n : 0
    }
    func same(_ a: AXUIElement, _ b: AXUIElement) -> Bool { CFEqual(a, b) }
}

/// The one stop watcher (the event tap's C callback can't capture).
let ctrlStopWatch = CtrlStopWatch()

private func ctrlTapCallback(proxy: CGEventTapProxy, type: CGEventType, event: CGEvent, refcon: UnsafeMutableRawPointer?) -> Unmanaged<CGEvent>? {
    let marker = event.getIntegerValueField(.eventSourceUserData)
    switch type {
    case .keyDown:
        ctrlStopWatch.record(.keyDown(code: UInt16(truncatingIfNeeded: event.getIntegerValueField(.keyboardEventKeycode)), marker: marker))
    case .leftMouseDown, .rightMouseDown, .otherMouseDown:
        ctrlStopWatch.record(.mouse(marker: marker))
    case .scrollWheel:
        ctrlStopWatch.record(.scroll(marker: marker))
    case .tapDisabledByTimeout, .tapDisabledByUserInput:
        ctrlStopWatch.record(.tapDisabled)
    default: break
    }
    return Unmanaged.passUnretained(event)
}

public final class MacCtrlBackend: CtrlBackend {
    let ax = MacAXSource()
    let handles = CtrlHandles<AXUIElement>()
    let layaRuntime = CtrlLayaRuntime()
    private var tap: CFMachPort?
    private var tapSource: CFRunLoopSource?

    /// Review M5: one 1 s AX messaging timeout for every element (set on the system-wide element).
    public init() { AXUIElementSetMessagingTimeout(AXUIElementCreateSystemWide(), 1.0) }

    // MARK: apps

    func runningApp(_ bundleId: String) -> NSRunningApplication? {
        NSWorkspace.shared.runningApplications.first { $0.bundleIdentifier == bundleId && !$0.isTerminated }
    }

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


    // MARK: tree

    func appElement(_ bundleId: String) throws -> (pid: Int32, el: AXUIElement) {
        guard AXIsProcessTrusted() else { throw CtrlFailure("ax.not_trusted") }
        guard let app = runningApp(bundleId) else { throw CtrlFailure("app.not_running") }
        let el = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(el, 1.0)
        return (app.processIdentifier, el)
    }

    public func tree(app: String, scope: String?, depth: Int) throws -> (root: CtrlTreeNode, focused: String?) {
        let (_, appEl) = try appElement(app)
        let reader = CtrlTreeReader(source: ax, handles: handles)
        let focused = MacAXSource.element(MacAXSource.raw(appEl, kAXFocusedUIElementAttribute))
        let root: AXUIElement
        if let scope {
            guard let h = handles.get(scope), h.app == app else { throw CtrlFailure("act.stale") }
            root = h.el
        } else {
            handles.forget(app: app)
            guard let w = MacAXSource.element(MacAXSource.raw(appEl, kAXFocusedWindowAttribute))
                ?? MacAXSource.element(MacAXSource.raw(appEl, kAXMainWindowAttribute)) else { throw CtrlFailure("app.no_window") }
            root = w
        }
        guard let r = reader.read(root, app: app, depth: depth, focused: focused) else { throw CtrlFailure("act.refused") }
        return r
    }

    /// Menu glyph modifiers (`AXMenuItemCmdModifiers`): 1 shift, 2 option, 4 control, 8 = no command key.
    static func combo(char: String?, virtualKey: Int?, modifiers: Int) -> String? {
        let named: [Int: String] = [36: "return", 48: "tab", 49: "space", 51: "delete", 53: "escape",
                                    123: "left", 124: "right", 125: "down", 126: "up"]
        var key: String
        if let c = char?.lowercased(), !c.isEmpty, c.unicodeScalars.allSatisfy({ $0.value >= 0x20 && $0.value < 0x7f }) {
            key = c == "," ? "comma" : c == "." ? "period" : c == "=" ? "equal" : c == "-" ? "minus" : c == "`" ? "grave" : c == " " ? "space" : c
        } else if let k = virtualKey, let n = named[k] {
            key = n
        } else {
            return nil
        }
        var parts: [String] = []
        if modifiers & 8 == 0 { parts.append("cmd") }
        if modifiers & 4 != 0 { parts.append("ctrl") }
        if modifiers & 2 != 0 { parts.append("option") }
        if modifiers & 1 != 0 { parts.append("shift") }
        if key.isEmpty { return nil }
        parts.append(key)
        return parts.joined(separator: "+")
    }

    public func menuShortcuts(app: String) throws -> [CtrlMenuShortcut] {
        let (_, appEl) = try appElement(app)
        handles.forget(app: app, prefix: "m")
        guard let bar = MacAXSource.element(MacAXSource.raw(appEl, kAXMenuBarAttribute)) else { return [] }
        var out: [CtrlMenuShortcut] = []
        func walk(_ el: AXUIElement, _ path: [String], _ level: Int) {
            guard level < 4, out.count < 400 else { return }
            for c in ax.children(el) {
                let title = ax.title(c)
                let role = ax.role(c) ?? ""
                if role == "AXMenuItem", !title.isEmpty,
                   let combo = Self.combo(char: MacAXSource.string(c, kAXMenuItemCmdCharAttribute),
                                          virtualKey: MacAXSource.int(c, kAXMenuItemCmdVirtualKeyAttribute),
                                          modifiers: MacAXSource.int(c, kAXMenuItemCmdModifiersAttribute) ?? 0) {
                    let sig = ctrlSig([role, "", title, ax.actions(c).sorted().joined(separator: ",")])
                    let id = handles.remember(c, app: app, sig: sig, prefix: "m")
                    out.append(CtrlMenuShortcut(id: id, path: path + [title], combo: combo, enabled: ax.enabled(c)))
                }
                walk(c, title.isEmpty ? path : path + [title], level + 1)
            }
        }
        walk(bar, [], 0)
        return out
    }

    // MARK: act

    /// Look the handle up and re-check it: still there, same fingerprint as the host saw, not a secure field.
    func live(_ handle: String, sig: String?) throws -> AXUIElement {
        guard let h = handles.get(handle) else { throw CtrlFailure("act.stale") }
        let reader = CtrlTreeReader(source: ax, handles: handles)
        guard let now = reader.sig(h.el), now == h.sig, sig == nil || sig == now else { throw CtrlFailure("act.stale") }
        let role = ax.role(h.el) ?? ""
        if CtrlTreeRules.secure(role: role, subrole: ax.subrole(h.el))
            || CtrlTreeRules.sensitive(role: role, labels: [ax.title(h.el), ax.placeholder(h.el), ax.help(h.el)]) {
            throw CtrlFailure("act.refused")
        }
        return h.el
    }

    /// The target is focused, its app's focused element, and its app frontmost (`act.not_focused` otherwise).
    static func mustHaveFocus(_ el: AXUIElement) throws {
        var pid: pid_t = 0
        AXUIElementGetPid(el, &pid)
        let app = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(app, 1.0)
        let focused = MacAXSource.element(MacAXSource.raw(app, kAXFocusedUIElementAttribute))
        guard MacAXSource.bool(el, kAXFocusedAttribute) == true, let focused, CFEqual(focused, el),
              NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { throw CtrlFailure("act.not_focused") }
    }

    static func perform(_ el: AXUIElement, _ action: String) throws {
        guard AXUIElementPerformAction(el, action as CFString) == .success else { throw CtrlFailure("act.failed") }
    }

    /// One key event pair to `pid`, stamped so the stop watcher ignores it.
    static func post(combo: String, to pid: Int32) throws {
        let parts = combo.split(separator: "+").map(String.init)
        guard let key = parts.last, let code = CtrlKeys.code(key) else { throw CtrlFailure("act.refused") }
        var flags: CGEventFlags = []
        for m in parts.dropLast() {
            switch m {
            case "cmd": flags.insert(.maskCommand)
            case "ctrl": flags.insert(.maskControl)
            case "option": flags.insert(.maskAlternate)
            case "shift": flags.insert(.maskShift)
            default: throw CtrlFailure("act.refused")
            }
        }
        guard let src = CGEventSource(stateID: .privateState) else { throw CtrlFailure("act.failed") }
        src.userData = CtrlStopWatch.ctrlMarker
        for down in [true, false] {
            guard let e = CGEvent(keyboardEventSource: src, virtualKey: CGKeyCode(code), keyDown: down) else { throw CtrlFailure("act.failed") }
            e.flags = flags
            e.setIntegerValueField(.eventSourceUserData, value: CtrlStopWatch.ctrlMarker)
            e.postToPid(pid)
        }
    }

    static func wait(_ seconds: Double, _ start: (@escaping @Sendable (Error?) -> Void) -> Void) throws {
        let done = DispatchSemaphore(value: 0)
        let box = ErrorBox()
        start { e in box.error = e; done.signal() }
        guard done.wait(timeout: .now() + seconds) == .success else { throw CtrlFailure("act.timeout") }
        if box.error != nil { throw CtrlFailure("act.failed") }
    }

    final class ErrorBox: @unchecked Sendable { var error: Error? }

    func appURL(_ bundleId: String) throws -> URL {
        guard let u = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleId) else { throw CtrlFailure("app.unknown") }
        return u
    }

    public func act(_ action: CtrlAction, sig: String?) throws {
        switch action {
        case let .press(h):
            try Self.perform(try live(h, sig: sig), "AXPress")
        case let .increment(h):
            try Self.perform(try live(h, sig: sig), "AXIncrement")
        case let .decrement(h):
            try Self.perform(try live(h, sig: sig), "AXDecrement")
        case let .showMenu(h):
            try Self.perform(try live(h, sig: sig), "AXShowMenu")
        case let .focus(h):
            let el = try live(h, sig: sig)
            guard AXUIElementSetAttributeValue(el, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success else { throw CtrlFailure("act.failed") }
        case let .setValue(h, text, submit):
            let el = try live(h, sig: sig)
            guard CtrlTreeRules.textRoles.contains(ax.role(el) ?? "") else { throw CtrlFailure("act.refused") }
            // Review H3: the focus result counts. Rulings 3 / (c): right before typing (and again before Return) the
            // target has kAXFocused, it is its app's focused element, and its app is frontmost; else refused.
            guard AXUIElementSetAttributeValue(el, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success else { throw CtrlFailure("act.failed") }
            try Self.mustHaveFocus(el)
            guard AXUIElementSetAttributeValue(el, kAXValueAttribute as CFString, text as CFString) == .success else { throw CtrlFailure("act.failed") }
            if submit {
                // M5: the watcher again before Return.
                guard ctrlStopWatch.mayAct else { throw CtrlFailure("guard.stopped") }
                try Self.mustHaveFocus(el)
                var pid: pid_t = 0
                AXUIElementGetPid(el, &pid)
                try Self.post(combo: "return", to: pid)
            }
        case let .scroll(h, down):
            let el = try live(h, sig: sig)
            if (try? Self.perform(el, down ? "AXScrollDownByPage" : "AXScrollUpByPage")) != nil { return }
            guard let bar = MacAXSource.element(MacAXSource.raw(el, kAXVerticalScrollBarAttribute)) else { throw CtrlFailure("act.refused") }
            try Self.perform(bar, down ? "AXIncrement" : "AXDecrement")
        case let .keyCombo(app, combo):
            guard let a = runningApp(app) else { throw CtrlFailure("app.not_running") }
            // Only to the app ctrl brought forward: a combo never lands in another app.
            guard NSWorkspace.shared.frontmostApplication?.processIdentifier == a.processIdentifier else { throw CtrlFailure("act.refused") }
            // Review (low): no clipboard or select-all combo into a secure field.
            if CtrlDoPolicy.clipboardCombos.contains(combo) {
                let appEl = AXUIElementCreateApplication(a.processIdentifier)
                AXUIElementSetMessagingTimeout(appEl, 1.0)
                if let f = MacAXSource.element(MacAXSource.raw(appEl, kAXFocusedUIElementAttribute)),
                   CtrlTreeRules.secure(role: ax.role(f) ?? "", subrole: ax.subrole(f)) { throw CtrlFailure("act.refused") }
            }
            try Self.post(combo: combo, to: a.processIdentifier)
        case let .openURL(app, url):
            // Review (low): the helper itself only opens a URL in a browser.
            guard Self.browserIds().contains(app), let u = URL(string: url) else { throw CtrlFailure("act.refused") }
            let at = try appURL(app)
            let cfg = NSWorkspace.OpenConfiguration()
            cfg.activates = true
            cfg.promptsUserIfNeeded = false
            try Self.wait(10) { done in NSWorkspace.shared.open([u], withApplicationAt: at, configuration: cfg) { _, e in done(e) } }
        case let .openApp(app):
            let at = try appURL(app)
            // Review (low): only an installed app bundle (the Applications folders), never qrtz itself.
            let dirs = ["/Applications/", "/System/Applications/", NSHomeDirectory() + "/Applications/"]
            guard app != Bundle.main.bundleIdentifier, !app.hasPrefix("ai.qrtz."),
                  dirs.contains(where: at.standardizedFileURL.path.hasPrefix) else { throw CtrlFailure("act.refused") }
            let cfg = NSWorkspace.OpenConfiguration()
            cfg.activates = true
            cfg.promptsUserIfNeeded = false
            try Self.wait(10) { done in NSWorkspace.shared.openApplication(at: at, configuration: cfg) { _, e in done(e) } }
        }
    }

    // MARK: stop key

    /// Listen-only, on the main run loop (the helper's `main` runs it). nil when the system refuses the tap.
    func startTap() -> Bool {
        if let tap { CGEvent.tapEnable(tap: tap, enable: true); return true }
        let types: [CGEventType] = [.keyDown, .leftMouseDown, .rightMouseDown, .otherMouseDown, .scrollWheel]
        let mask = types.reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << CGEventMask($1.rawValue)) }
        guard let t = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .listenOnly,
                                        eventsOfInterest: mask, callback: ctrlTapCallback, userInfo: nil) else { return false }
        let src = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, t, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), src, .commonModes)
        CGEvent.tapEnable(tap: t, enable: true)
        tap = t
        tapSource = src
        return true
    }

    /// Review (d): the tap only counts with the permission it needs and no secure event input.
    static func mayListen() -> Bool {
        (CGPreflightListenEventAccess() || AXIsProcessTrusted()) && !IsSecureEventInputEnabled()
    }

    public func mayAct() -> Bool {
        if IsSecureEventInputEnabled() { ctrlStopWatch.lost() }
        return ctrlStopWatch.mayAct
    }

    func stopTap() {
        if let tap { CGEvent.tapEnable(tap: tap, enable: false) }
    }

    public func observe(arm: Bool?) -> CtrlObserved {
        switch arm {
        case true?: ctrlStopWatch.arm(started: Self.mayListen() && startTap())
        case false?: ctrlStopWatch.disarm(); stopTap()
        case nil: break
        }
        if IsSecureEventInputEnabled() { ctrlStopWatch.lost() }
        let f = ctrlStopWatch.flags
        return CtrlObserved(frontmost: NSWorkspace.shared.frontmostApplication?.bundleIdentifier, focused: nil,
                            stopped: f.stopped, userInput: f.userInput, armed: f.armed)
    }

    // MARK: laya

    public func laya(model: String?, state: String, question: String, options: [String]) throws -> [Double] {
        try layaRuntime.choice(model: model, state: state, question: question, options: options)
    }

    public func warmLaya(model: String) throws -> CtrlLayaWarmer.State {
        try layaRuntime.warmer.start(model)
    }
}


/// The Core ML Laya runtime (ADR 0106 / 0109 / 0116). It runs only a model the user consented to download: the host
/// sends the installed model's path with each `ctrl.laya` after consent and never otherwise (`QRTZ_CTRL_LAYA_MODEL`
/// at launch is the developer override), and nothing here downloads anything. A host path is the read-only,
/// store-linked `model.laya` package (ADR 0157 §9): only a `.mlmodelc` with no writable file loads, as is
/// (`CtrlLayaLoad`). Only the developer override's `.mlpackage` / `.mlmodel` is compiled once into a sibling
/// `.mlmodelc` (the persisted cache, reused while it is newer than its source). Without a usable model every
/// question answers `laya.no_model` / `laya.load_failed` / `laya.not_read_only` / `laya.needs_compiled` and the host
/// keeps the heuristic scorer for that question.
final class CtrlLayaRuntime: @unchecked Sendable {
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
        let fromHost = model?.isEmpty == false
        guard let path = (fromHost ? model : env), !path.isEmpty else { throw CtrlFailure("laya.no_model") }
        lock.lock(); defer { lock.unlock() }
        if let m = models[path] { return m }
        if failed.contains(path) { throw CtrlFailure("laya.load_failed") }
        // ADR 0157 §9: a host path is the read-only store package, opened as is and never compiled into.
        let open: URL
        switch CtrlLayaLoad.plan(path: path, fromHost: fromHost) {
        case let .refuse(code): throw CtrlFailure(code)
        case let .asIs(p): open = URL(fileURLWithPath: p)
        case let .compile(p):
            do { open = try Self.compiled(URL(fileURLWithPath: p)) } catch {
                failed.insert(path)
                throw CtrlFailure("laya.load_failed")
            }
        }
        let c = MLModelConfiguration()
        c.computeUnits = .all // L512: GPU 9 ms vs CPU+ANE 27.5 ms warm (laya-coreml README, ADR 0129)
        do {
            let m = try MLModel(contentsOf: open, configuration: c)
            // Run 46: a model without ctrl's inputs or `probabilities` is refused once, at load, not per question.
            let d = m.modelDescription
            if CtrlLayaInterface.missing(inputs: Set(d.inputDescriptionsByName.keys),
                                         outputs: Set(d.outputDescriptionsByName.keys)) != nil {
                failed.insert(path)
                throw CtrlFailure("laya.bad_model")
            }
            models[path] = m
            return m
        } catch let f as CtrlFailure {
            throw f
        } catch {
            failed.insert(path)
            throw CtrlFailure("laya.load_failed")
        }
    }

    private let tokenizers = CtrlLayaTokenizerCache()

    /// ADR 0133: the warm-up the host asks for at helper start: the tokenizer, then the model (and its compiled
    /// cache), on the warmer's background queue. Errors are left for the first question to report.
    lazy var warmer: CtrlLayaWarmer = CtrlLayaWarmer { [weak self] path in
        guard let self else { return }
        _ = try? self.tokenizer(for: path)
        _ = try? self.load(path)
    }

    /// The tokenizer next to the model (`<set dir>/tokenizer.json`, ADR 0129) or `QRTZ_CTRL_LAYA_TOKENIZER`; read
    /// once per path. A missing or unreadable file answers `laya.no_tokenizer` and the host keeps the heuristic.
    func tokenizer(for modelPath: String) throws -> CtrlLayaTokenizer {
        try tokenizers.tokenizer(at: CtrlLayaWarmer.tokenizerPath(for: modelPath))
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
        let path = (model?.isEmpty == false ? model! : ProcessInfo.processInfo.environment["QRTZ_CTRL_LAYA_MODEL"] ?? "")
        // ADR 0133: don't queue behind the warm-up's load; the host's heuristic takes this one question.
        if warmer.isWarming(path) { throw CtrlFailure("laya.warming") }
        let m = try load(model)
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
