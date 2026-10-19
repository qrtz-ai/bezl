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
    /// ADR 0130: activations seen since the helper started (in memory only).
    let lastUsed = CtrlLastUsed(sinceMs: CtrlLastUsed.nowMs())
    private var activationObserver: NSObjectProtocol?
    /// ADR 0168: the general pasteboard's change count right after ctrl's last clear (nil: never cleared).
    private var lastClipboardClear: Int?
    /// ADR 0183: the Mac's last sleep time read just before a `sleep_system` act; the next `last_sleep` read waits for
    /// it to move, then clears it.
    private var pendingSleepBaseline: Int?
    /// ADR 0194: this process's one keep-awake assertion (nil: none held).
    private var awakeAssertion: IOPMAssertionID?

    public init() {
        // The app in front when the watch begins counts as used now; every later switch is recorded as it happens.
        if let id = NSWorkspace.shared.frontmostApplication?.bundleIdentifier { lastUsed.record(id, atMs: lastUsed.sinceMs) }
        let used = lastUsed
        activationObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: nil
        ) { note in
            if let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication, let id = app.bundleIdentifier {
                used.record(id, atMs: CtrlLastUsed.nowMs())
            }
        }
    }

    deinit {
        if let o = activationObserver { NSWorkspace.shared.notificationCenter.removeObserver(o) }
    }

    /// ADR 0130: running regular apps with their last activation; read-only, no AX prompt (window count only when
    /// already trusted), nothing activated.
    public func runningApps() throws -> (apps: [CtrlRunningApp], sinceMs: UInt64?, dayStartMs: UInt64) {
        let front = frontmostPid()
        let trusted = CtrlProbe.axTrusted
        let apps = NSWorkspace.shared.runningApplications
            .filter { $0.activationPolicy == .regular && !$0.isTerminated }
            .compactMap { a -> CtrlRunningApp? in
                guard let id = a.bundleIdentifier else { return nil }
                let unsaved: Bool? = trusted && CtrlActions.windows(a.processIdentifier).isEmpty ? false : nil
                return CtrlRunningApp(bundleId: id, name: a.localizedName ?? id, lastActiveMs: lastUsed.lastActive(id),
                                      frontmost: a.processIdentifier == front, unsaved: unsaved, hidden: a.isHidden)
            }
        return (apps, lastUsed.sinceMs, CtrlLastUsed.dayStartMs(nowMs: CtrlLastUsed.nowMs()))
    }

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
        // ADR 0137: System Events is background-only (not in `ids`); checked without asking, only while it runs.
        if let c = Self.automation(CtrlRoute.systemEvents) { automation[CtrlRoute.systemEvents] = c }
        // ADR 0158: read-only (never asks); the prompt comes on the first focus run.
        if let c = Self.automation(CtrlFocus.target) { automation[CtrlFocus.target] = c }
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

    /// ADR 0158: like `runScript`, but the error number decides the code ("set up the shortcut" / "allow automation").
    static func runFocus(_ source: String) throws {
        var code: Int?
        var failed = false
        let run = {
            var info: NSDictionary?
            let r: NSAppleEventDescriptor? = NSAppleScript(source: source)?.executeAndReturnError(&info)
            failed = info != nil || r == nil
            code = (info?["NSAppleScriptErrorNumber"] as? NSNumber)?.intValue
        }
        if Thread.isMainThread { run() } else { DispatchQueue.main.sync(execute: run) }
        if failed { throw CtrlFocus.failure(errorNumber: code) }
    }

    func script(_ verb: String, app: String) throws -> NSAppleEventDescriptor {
        // `tell application id` launches an app that isn't running: never.
        guard runningApp(app) != nil else { throw CtrlFailure("app.not_running") }
        guard let src = CtrlScripts.source(verb: verb, bundleId: app) else { throw CtrlFailure("policy.refused") }
        return try Self.runScript(src)
    }

    /// ADR 0135: the only two sound scripts (templates, never task text).
    static let muteSource = "set volume with output muted"
    /// ADR 0139: the reverse.
    static let unmuteSource = "set volume without output muted"
    /// ADR 0140: the level is checked 0–100 by the parse and is the only thing put in; never task text.
    static func setVolumeSource(_ level: Int) -> String {
        "set volume output volume \(min(max(level, 0), 100))\nset volume without output muted"
    }
    /// ADR 0140: -1 (read as nil) when muted or when the output has no volume, so an inaudible level never verifies.
    static let outputVolumeSource = "set s to (get volume settings)\nif output muted of s is true then\nreturn -1\nend if\nset v to output volume of s\nif v is missing value then\nreturn -1\nend if\nreturn v"
    static let outputMutedSource = "if output muted of (get volume settings) then\nreturn 1\nelse\nreturn 0\nend if"

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
        case let .quitApps(apps):
            // `terminate()` asks each app to quit the way the Dock's Quit does: no activation, no key event. An app
            // that already quit is fine. The frontmost app was refused before this point (CtrlServer).
            let front = frontmostPid()
            for id in apps {
                guard let a = runningApp(id), a.processIdentifier != front else { continue }
                _ = a.terminate()
            }
        case .muteOutput:
            // ADR 0135: Standard Additions, run in the helper's own process: no Apple Event to another app, no
            // volume HUD, no key event. The host verifies by reading the mute back.
            _ = try Self.runScript(Self.muteSource)
        case .unmuteOutput:
            // ADR 0139: the same, reversed.
            _ = try Self.runScript(Self.unmuteSource)
        case let .setVolume(level):
            // ADR 0140: the same process and rules; the host verifies by reading the level back.
            _ = try Self.runScript(Self.setVolumeSource(level))
        case .clearClipboard:
            // ADR 0168: the general pasteboard, emptied in this process: no app told anything, nothing shown, no key
            // event, and nothing read from it first. The count it returns is what the verify compares against.
            let count = NSPasteboard.general.clearContents()
            lock.lock(); lastClipboardClear = count; lock.unlock()
        case .sleepDisplay:
            // ADR 0180: the displays sleep (IOKit, this process); the Mac stays awake, nothing locked, no key event.
            try CtrlDisplayMac.sleepNow()
        case .sleepSystem:
            // ADR 0183: the whole Mac sleeps (IOKit, this process); nothing quit or closed, no key event. The time it
            // last slept, read first, is what the next read waits to see move.
            let before = CtrlSystemSleepMac.lastSleep()
            try CtrlSystemSleepMac.sleepNow()
            lock.lock(); pendingSleepBaseline = before; lock.unlock()
        case let .keepAwake(minutes, display):
            // ADR 0194: one power assertion of this process with a kernel timeout (IOKit); the new one is made before
            // the last one is let go, so the Mac never dips into sleep between them. Nothing on screen changes.
            let id = try CtrlKeepAwakeMac.hold(minutes: minutes, display: display)
            lock.lock(); let old = awakeAssertion; awakeAssertion = id; lock.unlock()
            if let old { CtrlKeepAwakeMac.release(old) }
        case .allowSleep:
            // ADR 0194: only this process's own assertion; nothing is put to sleep.
            lock.lock(); let old = awakeAssertion; awakeAssertion = nil; lock.unlock()
            if let old { CtrlKeepAwakeMac.release(old) }
        case let .setWifi(on):
            // ADR 0191: the Wi-Fi radio's power (CoreWLAN, this process); no network picked, nothing read about one.
            try CtrlWifiMac.setPower(on)
        case let .ejectVolumes(single):
            // ADR 0176: the volumes from the mount table (never a path from the host), each unmounted and ejected in
            // this process without force: one in use stays mounted (a file open, a copy, a backup) and the act says
            // so; nothing is brought forward, no key event.
            guard let volumes = ejectableVolumes() else { throw CtrlFailure("volumes.unreadable") }
            var busy = false
            for url in try CtrlVolumes.plan(volumes, single: single) {
                do { try NSWorkspace.shared.unmountAndEjectDevice(at: url) } catch { busy = true }
            }
            if busy { throw CtrlFailure("volume.busy") }
        case let .hideApps(apps):
            // ADR 0134: `hide()` is the Dock's Hide: the app's windows go, nothing is activated or raised, no key event.
            // The frontmost app was dropped before this point (CtrlServer); re-checked here for the race.
            let front = frontmostPid()
            for id in apps {
                guard let a = runningApp(id), a.processIdentifier != front, !a.isHidden else { continue }
                _ = a.hide()
            }
        case let .unhideApps(apps):
            // ADR 0152: `unhide()` brings the app's windows back where they were without activating it (unlike
            // `NSApplication.unhide(_:)` in-process): the frontmost app keeps the menu bar and the keyboard, no key
            // event. Minimized windows stay in the Dock. An app that isn't running or isn't hidden is skipped.
            for id in apps {
                guard let a = runningApp(id), a.isHidden else { continue }
                _ = a.unhide()
            }
        case let .setFocus(mode):
            // ADR 0158: one `run` event to Shortcuts Events (faceless: no activation, no window, no key event), the
            // name from the allowlist, the input the enum's raw value. The first run asks for Automation (the
            // system's own prompt, the first time a task needs it). A missing shortcut or a denial stops here.
            guard let src = CtrlFocus.source(shortcut: CtrlFocus.shortcutName, mode: mode) else { throw CtrlFailure("policy.refused") }
            try Self.runFocus(src)
        case let .openFolder(folder):
            // ADR 0166: the folder from the system's own location (never a path from the host), opened in the Finder
            // only while it runs (a cold start comes forward) through LaunchServices with `activates = false`: a new
            // Finder window behind the app the user is in, no key event, nothing read from the folder. A folder a
            // window shows already just gets that window (the Finder doesn't open it twice).
            guard let finder = runningApp(CtrlFolders.finder), let appURL = finder.bundleURL else { throw CtrlFailure("app.not_running") }
            guard let url = CtrlFolders.url(folder) else { throw CtrlFailure("folder.unknown") }
            try Self.wait(5) { done in CtrlActions.open([url], withApp: appURL) { _, e in done(e) } }
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
        // ADR 0135: read back by Standard Additions in this process (no app asked, no prompt); nil without an
        // output that can mute.
        case "output_muted": return (try? Self.runScript(Self.outputMutedSource)).map { Int($0.int32Value) }
        // ADR 0140: the level in this process; nil when muted or without a volume.
        case "output_volume": return (try? Self.runScript(Self.outputVolumeSource)).map { Int($0.int32Value) }.flatMap { $0 < 0 ? nil : $0 }
        // ADR 0137: the global appearance, read in this process (no app asked, no prompt): "Dark" or absent.
        // ADR 0158: not read (INFocusStatusCenter would be another prompt); the run's verify is the shortcut's reply.
        case "focus_on": return nil
        // ADR 0168: the change count only (never an item or a type), against the one the last clear returned.
        case CtrlClipboard.check:
            lock.lock(); let last = lastClipboardClear; lock.unlock()
            return CtrlClipboard.cleared(changeCount: NSPasteboard.general.changeCount, lastClear: last)
        // ADR 0176: the count only, from the mount table's resource values.
        case CtrlVolumes.check: return ejectableVolumes()?.count
        // ADR 0180: the main display's sleep state, read in this process (no app asked, no prompt).
        case CtrlDisplay.check: return CtrlDisplayMac.asleep() ? 1 : 0
        // ADR 0191: the Wi-Fi radio's power only, read in this process (no Location, no network name).
        case CtrlWifi.check: return CtrlWifi.value(powerOn: CtrlWifiMac.powerOn())
        // ADR 0194: this process's own keep-awake assertion, from IOKit's record of it (none: 0).
        case CtrlKeepAwake.check:
            lock.lock(); let id = awakeAssertion; lock.unlock()
            return CtrlKeepAwake.value(properties: id.flatMap(CtrlKeepAwakeMac.properties))
        // ADR 0183: when the Mac last slept; right after the act, waits (bounded) for the sleep to happen and end.
        case CtrlSystemSleep.check:
            lock.lock(); let baseline = pendingSleepBaseline; pendingSleepBaseline = nil; lock.unlock()
            return CtrlSystemSleep.awaitSleep(baseline: baseline, read: CtrlSystemSleepMac.lastSleep,
                                              wait: { Thread.sleep(forTimeInterval: $0) })
        case "dark_mode": return CtrlAppearance.isDark(UserDefaults.standard.persistentDomain(forName: UserDefaults.globalDomain)) ? 1 : 0
        // ADR 0173: the Dock's auto-hide, read from its own defaults in this process (no app asked, no prompt).
        case CtrlDock.check: return CtrlDock.autohides(UserDefaults.standard.persistentDomain(forName: CtrlDock.domain)) ? 1 : 0
        // ADR 0178: ScreenSaverEngine in this process's NSWorkspace list (no app asked, no prompt, nothing activated).
        case CtrlScreensaver.check:
            let apps = NSWorkspace.shared.runningApplications
            return CtrlScreensaver.running(bundleIds: apps.compactMap(\.bundleIdentifier),
                                           names: apps.compactMap(\.localizedName)) ? 1 : 0
        case "tab_count":
            return (try? script("count_tabs", app: app)).map { Int($0.int32Value) }
        case "saved_tabs", "unpinned_tabs":
            return (try? tabs(app: app)).flatMap { CtrlTabs.count($0, check: check) }
        case "window_count":
            guard let pid = pid(of: app) else { return nil }
            if let n = try? script("count_windows", app: app) { return Int(n.int32Value) }
            return CtrlProbe.axTrusted ? CtrlActions.windows(pid).count : nil
        default:
            // ADR 0166: `folder_open.<folder>`, from the Finder's AX windows, only when AX is trusted already (never
            // prompts); nil otherwise. ax_scope is verified by a re-snapshot and obs.diff on the host.
            if let f = CtrlFolders.measured(check) { return folderShown(f) }
            return nil
        }
    }

    /// ADR 0176: the mounted volumes an eject takes (`CtrlVolumes.ejectable`), from their resource values only;
    /// hidden system volumes are skipped. nil when the system gives no list.
    func ejectableVolumes() -> [URL]? {
        let keys: [URLResourceKey] = [.volumeIsEjectableKey, .volumeIsRemovableKey, .volumeIsLocalKey, .volumeIsRootFileSystemKey]
        guard let urls = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes])
        else { return nil }
        return urls.filter { url in
            guard let r = try? url.resourceValues(forKeys: Set(keys)) else { return false }
            return CtrlVolumes.ejectable(.init(ejectable: r.volumeIsEjectable ?? false, removable: r.volumeIsRemovable ?? false,
                                               local: r.volumeIsLocal ?? false, root: r.volumeIsRootFileSystem ?? true))
        }
    }

    /// ADR 0166: whether a Finder window shows `folder`, read from window documents and titles (compared here, never
    /// sent anywhere). No AX trust, no Finder, or no location: nil.
    func folderShown(_ folder: CtrlFolder) -> Int? {
        guard CtrlProbe.axTrusted, let pid = pid(of: CtrlFolders.finder), let url = CtrlFolders.url(folder) else { return nil }
        let windows = CtrlActions.windows(pid)
        return CtrlFolders.shown(windows: windows.count, documents: windows.compactMap { Self.string($0, kAXDocumentAttribute) },
                                 titles: windows.compactMap { Self.string($0, kAXTitleAttribute) }, path: url.path)
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
