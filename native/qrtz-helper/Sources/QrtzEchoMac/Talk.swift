#if os(macOS)
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import QrtzEcho
import QrtzMacShared
import QrtzShared

// ADR 0146 M2.5: split out of qrtz-helper/mac/Talk.swift (owner: echo: insertion and the push key).

public final class MacInsertion: InsertionBackend, @unchecked Sendable {
    private func focused() -> AXUIElement? {
        let sys = AXUIElementCreateSystemWide()
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(sys, kAXFocusedUIElementAttribute as CFString, &v) == .success, let v,
              CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
        return (v as! AXUIElement)
    }

    public func focusedAppName() -> String? { NSWorkspace.shared.frontmostApplication?.localizedName }

    public func axSetSelectedText(_ text: String) -> Bool {
        guard let el = focused() else { return false }
        var settable: DarwinBoolean = false
        guard AXUIElementIsAttributeSettable(el, kAXSelectedTextAttribute as CFString, &settable) == .success, settable.boolValue else { return false }
        // Some apps (Electron/Chromium, many web fields) report success but ignore the write: verify the value changed.
        let before = value(el)
        guard AXUIElementSetAttributeValue(el, kAXSelectedTextAttribute as CFString, text as CFString) == .success else { return false }
        guard let b = before else { return true }
        return value(el) != b
    }

    public func axInsertIntoValue(_ text: String) -> Bool {
        guard let el = focused(), let cur = value(el) else { return false }
        var settable: DarwinBoolean = false
        guard AXUIElementIsAttributeSettable(el, kAXValueAttribute as CFString, &settable) == .success, settable.boolValue else { return false }
        var rangeRef: CFTypeRef?
        var range = CFRange(location: (cur as NSString).length, length: 0)
        if AXUIElementCopyAttributeValue(el, kAXSelectedTextRangeAttribute as CFString, &rangeRef) == .success, let rangeRef,
           CFGetTypeID(rangeRef) == AXValueGetTypeID() {
            AXValueGetValue(rangeRef as! AXValue, .cfRange, &range)
        }
        let ns = cur as NSString
        guard range.location >= 0, range.location + range.length <= ns.length else { return false }
        let next = ns.replacingCharacters(in: NSRange(location: range.location, length: range.length), with: text)
        guard AXUIElementSetAttributeValue(el, kAXValueAttribute as CFString, next as CFString) == .success else { return false }
        var caret = CFRange(location: range.location + (text as NSString).length, length: 0)
        if let v = AXValueCreate(.cfRange, &caret) { AXUIElementSetAttributeValue(el, kAXSelectedTextRangeAttribute as CFString, v) }
        return value(el) == next
    }

    private func value(_ el: AXUIElement) -> String? {
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(el, kAXValueAttribute as CFString, &v) == .success else { return nil }
        return v as? String
    }

    public func pasteboardSnapshot() -> Any? {
        let pb = NSPasteboard.general
        return (pb.pasteboardItems ?? []).map { item -> [NSPasteboard.PasteboardType: Data] in
            var d: [NSPasteboard.PasteboardType: Data] = [:]
            for t in item.types { if let data = item.data(forType: t) { d[t] = data } }
            return d
        }
    }
    public func pasteboardSetString(_ text: String) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
        // Ask clipboard managers not to record the transient dictation text.
        pb.setData(Data(), forType: NSPasteboard.PasteboardType("org.nspasteboard.TransientType"))
    }
    public func pasteboardRestore(_ snapshot: Any?) {
        let pb = NSPasteboard.general
        pb.clearContents()
        guard let items = snapshot as? [[NSPasteboard.PasteboardType: Data]], !items.isEmpty else { return }
        pb.writeObjects(items.map { d in
            let it = NSPasteboardItem()
            for (t, data) in d { it.setData(data, forType: t) }
            return it
        })
    }
    public func pasteboardChangeCount() -> Int { NSPasteboard.general.changeCount }
    public func postPaste() -> Bool {
        guard AXIsProcessTrusted() else { return false }
        let src = CGEventSource(stateID: .combinedSessionState)
        let v: CGKeyCode = 9  // kVK_ANSI_V
        guard let down = CGEvent(keyboardEventSource: src, virtualKey: v, keyDown: true),
              let up = CGEvent(keyboardEventSource: src, virtualKey: v, keyDown: false) else { return false }
        down.flags = .maskCommand; up.flags = .maskCommand
        down.post(tap: .cghidEventTap); up.post(tap: .cghidEventTap)
        return true
    }
}

/// Modifier-only push-to-talk keys via session-level CGEvent taps (`HotkeyFilter` decides). Fn is another dictation app's
/// default.
///
/// Two taps, so blocking the globe key can never break push-to-talk (Oct 17 regression: 4f19009 reported the key
/// edges from the active tap itself; on Jared's Mac that tap was created but the Fn edges never reached qrtz, so
/// pressing fn did nothing at all):
/// - Edges: a listen-only `flagsChanged` tap, exactly what worked before the globe fix. It alone reports down/up.
/// - Blocking (only with `fn`, consumeFn on = `talk.blockGlobeKey`, and Accessibility): a separate active tap that
///   drops the Fn/globe events so macOS doesn't also switch the input source or open the emoji picker. It reports
///   nothing; if macOS withholds events from it, talk still works and only the globe action runs too.
/// The blocker is created first and the listener inserted at the head after it, so the listener sees every Fn event
/// before the blocker drops it.
public final class MacHotkeys: HotkeyMonitor, @unchecked Sendable {
    private var listenTap: CFMachPort?
    private var listenSource: CFRunLoopSource?
    private var blockTap: CFMachPort?
    private var blockSource: CFRunLoopSource?
    fileprivate var onChange: (@Sendable (HotkeyAction) -> Void)?
    /// Edge state (listen tap only).
    fileprivate var filter: HotkeyFilter?
    /// Consume decisions (block tap only; its edges are ignored).
    fileprivate var blocker: HotkeyFilter?

    public func start(key: String, consumeFn: Bool, onChange: @escaping @Sendable (HotkeyAction) -> Void) -> Bool {
        stop()
        guard let f = HotkeyFilter(key: key, consumeFn: consumeFn) else { return false }
        filter = f
        self.onChange = onChange
        let me = Unmanaged.passUnretained(self).toOpaque()

        if f.consumes && AXIsProcessTrusted() {
            blocker = HotkeyFilter(key: key, consumeFn: true)
            let block: CGEventTapCallBack = { _, type, event, ctx in
                guard let ctx else { return Unmanaged.passUnretained(event) }
                let hk = Unmanaged<MacHotkeys>.fromOpaque(ctx).takeUnretainedValue()
                if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
                    _ = hk.blocker?.reset()
                    if let t = hk.blockTap { CGEvent.tapEnable(tap: t, enable: true) }
                    return Unmanaged.passUnretained(event)
                }
                guard var b = hk.blocker else { return Unmanaged.passUnretained(event) }
                let r = b.handle(type: type.rawValue, keyCode: event.getIntegerValueField(.keyboardEventKeycode),
                                 flags: event.flags.rawValue)
                hk.blocker = b
                return r.consume ? nil : Unmanaged.passUnretained(event)
            }
            let mask = CGEventMask(1 << CGEventType.flagsChanged.rawValue)
                | CGEventMask(1 << CGEventType.keyDown.rawValue) | CGEventMask(1 << CGEventType.keyUp.rawValue)
            if let t = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap,
                                         eventsOfInterest: mask, callback: block, userInfo: me) {
                blockTap = t
                let src = CFMachPortCreateRunLoopSource(nil, t, 0)
                blockSource = src
                CFRunLoopAddSource(CFRunLoopGetMain(), src, .commonModes)
                CGEvent.tapEnable(tap: t, enable: true)
            }
        }

        let listen: CGEventTapCallBack = { _, type, event, ctx in
            guard let ctx else { return Unmanaged.passUnretained(event) }
            let hk = Unmanaged<MacHotkeys>.fromOpaque(ctx).takeUnretainedValue()
            if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
                if let up = hk.filter?.reset() { hk.onChange?(up) }
                if let t = hk.listenTap { CGEvent.tapEnable(tap: t, enable: true) }
                return Unmanaged.passUnretained(event)
            }
            guard type == .flagsChanged, var f = hk.filter else { return Unmanaged.passUnretained(event) }
            let r = f.handle(type: type.rawValue, keyCode: event.getIntegerValueField(.keyboardEventKeycode),
                             flags: event.flags.rawValue)
            hk.filter = f
            if let a = r.action { hk.onChange?(a) }
            return Unmanaged.passUnretained(event)
        }
        guard let t = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .listenOnly,
                                        eventsOfInterest: CGEventMask(1 << CGEventType.flagsChanged.rawValue),
                                        callback: listen, userInfo: me) else {
            stop()
            return false
        }
        listenTap = t
        let src = CFMachPortCreateRunLoopSource(nil, t, 0)
        listenSource = src
        CFRunLoopAddSource(CFRunLoopGetMain(), src, .commonModes)
        CGEvent.tapEnable(tap: t, enable: true)
        return true
    }

    public func stop() {
        for (t, s) in [(listenTap, listenSource), (blockTap, blockSource)] {
            if let t { CGEvent.tapEnable(tap: t, enable: false) }
            if let s { CFRunLoopRemoveSource(CFRunLoopGetMain(), s, .commonModes) }
        }
        listenTap = nil; listenSource = nil; blockTap = nil; blockSource = nil
        filter = nil; blocker = nil
    }
}
#endif
