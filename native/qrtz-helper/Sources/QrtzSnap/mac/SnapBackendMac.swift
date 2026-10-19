#if os(macOS)
// qrtz snap on macOS (ADR 0159): global hotkeys through Carbon's RegisterEventHotKey (no Input Monitoring needed, as
// in Spectacle), windows moved through the Accessibility API under qrtz's one Accessibility grant (ADR 0154: the
// host spawns this helper, so qrtz is its responsible process), and Spectacle's status item menu.
// Ported in spirit from SpectacleShortcutManager / SpectacleAccessibilityElement (MIT, Copyright (c) 2017 Eric Czarny;
// notice in ../SnapGeometry.swift). Everything here runs on the main thread.
import AppKit
import ApplicationServices
import Carbon.HIToolbox
import Foundation

// MARK: the Accessibility window system

public final class AXSnapWindowSystem: SnapWindowSystem {
    public typealias Element = AXUIElement
    public init() {}

    public func frontmost() -> SnapFrontmost<AXUIElement>? {
        guard let app = NSWorkspace.shared.frontmostApplication, let bundleId = app.bundleIdentifier else { return nil }
        let ax = AXUIElementCreateApplication(app.processIdentifier)
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(ax, kAXFocusedWindowAttribute as CFString, &value) == .success,
              let v = value, CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
        return SnapFrontmost(bundleId: bundleId, window: (v as! AXUIElement))
    }

    public func rect(of e: AXUIElement) -> SnapRect? {
        var pv: CFTypeRef?, sv: CFTypeRef?
        guard AXUIElementCopyAttributeValue(e, kAXPositionAttribute as CFString, &pv) == .success,
              AXUIElementCopyAttributeValue(e, kAXSizeAttribute as CFString, &sv) == .success,
              let pRef = pv, let sRef = sv else { return nil }
        var p = CGPoint.zero, s = CGSize.zero
        guard AXValueGetValue(pRef as! AXValue, .cgPoint, &p), AXValueGetValue(sRef as! AXValue, .cgSize, &s) else { return nil }
        return SnapRect(x: Double(p.x), y: Double(p.y), width: Double(s.width), height: Double(s.height))
    }

    public func isSheetOrDialog(_ e: AXUIElement) -> Bool {
        func string(_ attr: String) -> String? {
            var v: CFTypeRef?
            guard AXUIElementCopyAttributeValue(e, attr as CFString, &v) == .success else { return nil }
            return v as? String
        }
        return string(kAXRoleAttribute) == kAXSheetRole || string(kAXSubroleAttribute) == kAXSystemDialogSubrole
    }

    public func setRect(_ r: SnapRect, of e: AXUIElement) {
        var size = CGSize(width: r.width, height: r.height)
        var point = CGPoint(x: r.x, y: r.y)
        guard let s = AXValueCreate(.cgSize, &size), let p = AXValueCreate(.cgPoint, &point) else { return }
        AXUIElementSetAttributeValue(e, kAXSizeAttribute as CFString, s)
        AXUIElementSetAttributeValue(e, kAXPositionAttribute as CFString, p)
        AXUIElementSetAttributeValue(e, kAXSizeAttribute as CFString, s)
    }

    static func rect(_ r: NSRect) -> SnapRect {
        SnapRect(x: Double(r.origin.x), y: Double(r.origin.y), width: Double(r.size.width), height: Double(r.size.height))
    }

    public var screens: [SnapScreen] {
        NSScreen.screens.map { SnapScreen(frame: Self.rect($0.frame), visibleFrame: Self.rect($0.visibleFrame)) }
    }

    public var mainScreen: SnapScreen? {
        NSScreen.main.map { SnapScreen(frame: Self.rect($0.frame), visibleFrame: Self.rect($0.visibleFrame)) }
    }

    public var primaryHeight: Double { Double(NSScreen.screens.first?.frame.height ?? 0) }
}

// MARK: hotkeys

private let snapHotKeySignature: OSType = 0x534E_4150 // 'SNAP'
private var snapHotKeyActions: [UInt32: SnapAction] = [:]
private var snapHotKeyHandler: ((SnapAction) -> Void)?

private func snapHotKeyCallback(_: EventHandlerCallRef?, _ event: EventRef?, _: UnsafeMutableRawPointer?) -> OSStatus {
    guard let event else { return OSStatus(eventNotHandledErr) }
    var id = EventHotKeyID()
    let err = GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                                nil, MemoryLayout<EventHotKeyID>.size, nil, &id)
    guard err == noErr, id.signature == snapHotKeySignature, let a = snapHotKeyActions[id.id] else {
        return OSStatus(eventNotHandledErr)
    }
    snapHotKeyHandler?(a)
    return noErr
}

// MARK: the backend

public final class MacSnapBackend: NSObject, SnapBackend {
    let system = AXSnapWindowSystem()
    lazy var engine = SnapEngine(system: system)
    var hotKeys: [EventHotKeyRef] = []
    var handlerInstalled = false
    var statusItem: NSStatusItem?
    var current: [SnapAction: SnapShortcut] = [:]
    let send: (String) -> Void

    /// `send` writes a line to the host (the status menu's "disable for <app>", and move outcomes).
    public init(send: @escaping (String) -> Void) {
        self.send = send
        super.init()
        snapHotKeyHandler = { [weak self] a in self?.fire(a) }
    }

    public var axTrusted: Bool { AXIsProcessTrusted() }

    func fire(_ a: SnapAction) {
        send(SnapLines.outcome(a, perform(a)))
    }

    public func perform(_ action: SnapAction) -> SnapOutcome {
        guard AXIsProcessTrusted() else {
            NSSound.beep()
            return .failed("qrtz needs accessibility access")
        }
        let o = engine.perform(action)
        if case .failed = o { NSSound.beep() }
        return o
    }

    public func register(_ shortcuts: [SnapAction: SnapShortcut]) -> [SnapAction: String] {
        for h in hotKeys { UnregisterEventHotKey(h) }
        hotKeys.removeAll()
        snapHotKeyActions.removeAll()
        if !handlerInstalled {
            var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
            InstallEventHandler(GetApplicationEventTarget(), snapHotKeyCallback, 1, &spec, nil, nil)
            handlerInstalled = true
        }
        var failed: [SnapAction: String] = [:]
        current = [:]
        for (i, a) in SnapAction.allCases.enumerated() {
            guard let s = shortcuts[a] else { continue }
            var ref: EventHotKeyRef?
            let id = EventHotKeyID(signature: snapHotKeySignature, id: UInt32(i + 1))
            let err = RegisterEventHotKey(s.keyCode, s.carbonModifiers, id, GetApplicationEventTarget(), 0, &ref)
            if err == noErr, let ref {
                hotKeys.append(ref)
                snapHotKeyActions[id.id] = a
                current[a] = s
            } else {
                failed[a] = "another app uses these keys"
            }
        }
        rebuildMenu()
        return failed
    }

    public func setDisabledApps(_ apps: Set<String>) {
        engine.disabledApps = apps
        rebuildMenu()
    }

    public func setStatusItem(_ shown: Bool) {
        if shown, statusItem == nil {
            let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
            item.button?.image = NSImage(systemSymbolName: "rectangle.split.2x1", accessibilityDescription: "qrtz snap")
            statusItem = item
            rebuildMenu()
        } else if !shown, let item = statusItem {
            NSStatusBar.system.removeStatusItem(item)
            statusItem = nil
        }
    }

    // Spectacle's status menu: every action with its shortcut, then "disable for <frontmost app>".
    func rebuildMenu() {
        guard let item = statusItem else { return }
        let menu = NSMenu()
        menu.delegate = self
        for a in SnapAction.allCases {
            let m = NSMenuItem(title: a.label, action: #selector(menuAction(_:)), keyEquivalent: "")
            m.target = self
            m.representedObject = a.rawValue
            if let s = current[a] { m.title = "\(a.label)    \(Self.symbols(s))" }
            menu.addItem(m)
            if a == .fullscreen || a == .bottomHalf || a == .lowerRight || a == .previousDisplay || a == .previousThird || a == .makeSmaller {
                menu.addItem(.separator())
            }
        }
        menu.addItem(.separator())
        let d = NSMenuItem(title: "disable for this app", action: #selector(toggleApp(_:)), keyEquivalent: "")
        d.target = self
        d.tag = 1
        menu.addItem(d)
        item.menu = menu
    }

    static func symbols(_ s: SnapShortcut) -> String {
        let mods = ["ctrl": "⌃", "alt": "⌥", "shift": "⇧", "cmd": "⌘"]
        let keys = ["left": "←", "right": "→", "up": "↑", "down": "↓"]
        return SnapShortcut.modifierOrder.filter(s.modifiers.contains).compactMap { mods[$0] }.joined()
            + (keys[s.key] ?? s.key)
    }

    var lastOtherApp: String?

    @objc func menuAction(_ sender: NSMenuItem) {
        guard let raw = sender.representedObject as? String, let a = SnapAction(rawValue: raw) else { return }
        fire(a)
    }

    @objc func toggleApp(_ sender: NSMenuItem) {
        guard let id = lastOtherApp else { return }
        let disabled = !engine.disabledApps.contains(id)
        // The host owns the setting (config `snap.disabledApps`); it answers with a new `snap.configure`.
        send(SnapLines.disableApp(id, disabled: disabled))
    }
}

extension MacSnapBackend: NSMenuDelegate {
    public func menuWillOpen(_ menu: NSMenu) {
        let app = NSWorkspace.shared.frontmostApplication
        lastOtherApp = app?.bundleIdentifier
        guard let item = menu.items.first(where: { $0.tag == 1 }) else { return }
        let name = (app?.localizedName ?? "this app").lowercased()
        let off = lastOtherApp.map { engine.disabledApps.contains($0) } ?? false
        item.title = off ? "enable for \(name)" : "disable for \(name)"
        item.isEnabled = lastOtherApp != nil && !(lastOtherApp.map(SnapEngine<AXSnapWindowSystem>.alwaysDisabled.contains) ?? false)
    }
}
#endif
