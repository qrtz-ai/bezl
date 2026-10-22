#if os(macOS)
// qrtz snap's "summon agent" on macOS: a listen-only CGEvent tap for the ⌘+⌘ chord (SummonChordFilter decides) and
// NSWorkspace to bring the chosen app forward or go back. The tap needs Input Monitoring (asked for only when the
// user turns summon on); it never drops or changes an event. Tap setup, the 2 s retry until the grant exists, and
// re-enabling after a timeout are copied from MacHotkeys (QrtzEchoMac/Talk.swift): snap's module imports only its own
// targets (the offline_graph ratchet), so it can't share that code. Everything here runs on the main thread.
import AppKit
import CoreGraphics
import Foundation

public final class MacSnapSummon {
    private var tap: CFMachPort?
    private var source: CFRunLoopSource?
    private var retry: Timer?
    var filter = SummonChordFilter()
    public private(set) var config = SnapSummonConfig()
    private var previous: String?
    private var apps: [SnapApp]?

    public init() {}

    /// The tap's callback holds this instance unretained: tear the tap down before the instance goes away.
    deinit { stop() }

    public var inputMonitoring: Bool { CGPreflightListenEventAccess() }

    public var status: SnapSummonStatus {
        SnapSummonStatus(inputMonitoring: inputMonitoring, active: tap != nil, apps: config.enabled ? apps : nil)
    }

    public func apply(_ c: SnapSummonConfig, requestAccess: Bool) {
        config = c
        guard c.enabled else {
            stop()
            apps = nil
            return
        }
        if apps == nil { apps = Self.installedApps() }
        filter.blocked = c.clashesWithEcho
        filter.reset()
        // Only a bundle id the picker offered (an installed app) is ever summoned; summon() checks again.
        if !SnapSummon.allowed(c.bundleId, installed: apps ?? []) { config.bundleId = "" }
        if requestAccess, !CGPreflightListenEventAccess() { _ = CGRequestListenEventAccess() }
        if tap == nil, !install() { startRetry() }
    }

    private func stop() {
        retry?.invalidate(); retry = nil
        if let t = tap { CGEvent.tapEnable(tap: t, enable: false); CFMachPortInvalidate(t) }
        if let s = source { CFRunLoopRemoveSource(CFRunLoopGetMain(), s, .commonModes) }
        tap = nil; source = nil
        filter.reset()
    }

    /// Like MacHotkeys: a tap that couldn't be made (no grant yet) is tried again every 2 s, so the chord works the
    /// moment the user switches qrtz on under Input Monitoring, with no restart.
    private func startRetry() {
        guard retry == nil else { return }
        retry = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            guard let self else { return }
            if !self.config.enabled || self.install() { self.retry?.invalidate(); self.retry = nil }
        }
    }

    private func install() -> Bool {
        guard tap == nil else { return true }
        guard CGPreflightListenEventAccess() else { return false }
        let me = Unmanaged.passUnretained(self).toOpaque()
        let cb: CGEventTapCallBack = { _, type, event, ctx in
            guard let ctx else { return Unmanaged.passUnretained(event) }
            let s = Unmanaged<MacSnapSummon>.fromOpaque(ctx).takeUnretainedValue()
            if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
                s.filter.reset()
                if let t = s.tap { CGEvent.tapEnable(tap: t, enable: true) }
                return Unmanaged.passUnretained(event)
            }
            let fire = s.filter.handle(type: type.rawValue,
                                       keyCode: event.getIntegerValueField(.keyboardEventKeycode),
                                       flags: event.flags.rawValue,
                                       sourceState: event.getIntegerValueField(.eventSourceStateID))
            if fire { DispatchQueue.main.async { s.summon() } }
            return Unmanaged.passUnretained(event)
        }
        let mask = CGEventMask(1 << CGEventType.flagsChanged.rawValue) | CGEventMask(1 << CGEventType.keyDown.rawValue)
        guard let t = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .tailAppendEventTap, options: .listenOnly,
                                        eventsOfInterest: mask, callback: cb, userInfo: me) else { return false }
        tap = t
        let src = CFMachPortCreateRunLoopSource(nil, t, 0)
        source = src
        CFRunLoopAddSource(CFRunLoopGetMain(), src, .commonModes)
        CGEvent.tapEnable(tap: t, enable: true)
        return true
    }

    func summon() {
        let chosen = config.bundleId
        guard config.enabled, !config.clashesWithEcho, SnapSummon.allowed(chosen, installed: apps ?? []) else {
            NSSound.beep(); return
        }
        let front = NSWorkspace.shared.frontmostApplication?.bundleIdentifier
        switch SnapSummon.step(frontmost: front, chosen: chosen, previous: previous) {
        case .summon(let prev):
            if prev != nil { previous = prev }
            if let running = NSRunningApplication.runningApplications(withBundleIdentifier: chosen).first {
                running.unhide()
                running.activate(options: [.activateAllWindows])
            } else if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: chosen) {
                let cfg = NSWorkspace.OpenConfiguration()
                cfg.activates = true
                NSWorkspace.shared.openApplication(at: url, configuration: cfg) { _, _ in }
            } else {
                NSSound.beep()
            }
        case .dismiss(let back):
            NSRunningApplication.runningApplications(withBundleIdentifier: chosen).first?.hide()
            if let back, let app = NSRunningApplication.runningApplications(withBundleIdentifier: back).first {
                app.activate(options: [])
            }
        }
    }

    /// The apps in /Applications, /System/Applications and ~/Applications (one level, plus one folder deep, like
    /// /Applications/Utilities), by display name. Read once when summon turns on.
    static func installedApps() -> [SnapApp] {
        let fm = FileManager.default
        var dirs = [URL(fileURLWithPath: "/Applications"), URL(fileURLWithPath: "/System/Applications"),
                    fm.homeDirectoryForCurrentUser.appendingPathComponent("Applications")]
        dirs += dirs.map { $0.appendingPathComponent("Utilities") }
        var seen = Set<String>()
        var out: [SnapApp] = []
        for d in dirs {
            guard let items = try? fm.contentsOfDirectory(at: d, includingPropertiesForKeys: nil) else { continue }
            for u in items where u.pathExtension == "app" {
                guard let b = Bundle(url: u), let id = b.bundleIdentifier, seen.insert(id).inserted else { continue }
                let name = (b.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String)
                    ?? (b.object(forInfoDictionaryKey: "CFBundleName") as? String)
                    ?? u.deletingPathExtension().lastPathComponent
                out.append(SnapApp(bundleId: id, name: name))
            }
        }
        return out.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }
}
#endif
