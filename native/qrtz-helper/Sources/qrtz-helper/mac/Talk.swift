#if os(macOS)
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import QrtzHelperCore

// qrtz talk on macOS: mic-only capture, AX/pasteboard text insertion, and the native push-to-talk key monitor.
// None of this compiles on Linux; the logic it plugs into (QrtzHelperCore/Dictation.swift) is tested there. [MAC-VERIFY]

/// Mic only, through its own AVAudioEngine (independent of meeting capture, so dictation works mid-meeting).
final class DictationMicSource: MicSource, @unchecked Sendable {
    private var mic: MicCapture?
    func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws {
        let m = MicCapture()
        try m.start(onSamples)
        mic = m
    }
    func stop() { mic?.stop(); mic = nil }
}

final class MacInsertion: InsertionBackend, @unchecked Sendable {
    private func focused() -> AXUIElement? {
        let sys = AXUIElementCreateSystemWide()
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(sys, kAXFocusedUIElementAttribute as CFString, &v) == .success, let v,
              CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
        return (v as! AXUIElement)
    }

    func focusedAppName() -> String? { NSWorkspace.shared.frontmostApplication?.localizedName }

    func axSetSelectedText(_ text: String) -> Bool {
        guard let el = focused() else { return false }
        var settable: DarwinBoolean = false
        guard AXUIElementIsAttributeSettable(el, kAXSelectedTextAttribute as CFString, &settable) == .success, settable.boolValue else { return false }
        // Some apps (Electron/Chromium, many web fields) report success but ignore the write: verify the value changed.
        let before = value(el)
        guard AXUIElementSetAttributeValue(el, kAXSelectedTextAttribute as CFString, text as CFString) == .success else { return false }
        guard let b = before else { return true }
        return value(el) != b
    }

    func axInsertIntoValue(_ text: String) -> Bool {
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

    func pasteboardSnapshot() -> Any? {
        let pb = NSPasteboard.general
        return (pb.pasteboardItems ?? []).map { item -> [NSPasteboard.PasteboardType: Data] in
            var d: [NSPasteboard.PasteboardType: Data] = [:]
            for t in item.types { if let data = item.data(forType: t) { d[t] = data } }
            return d
        }
    }
    func pasteboardSetString(_ text: String) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
        // Ask clipboard managers not to record the transient dictation text.
        pb.setData(Data(), forType: NSPasteboard.PasteboardType("org.nspasteboard.TransientType"))
    }
    func pasteboardRestore(_ snapshot: Any?) {
        let pb = NSPasteboard.general
        pb.clearContents()
        guard let items = snapshot as? [[NSPasteboard.PasteboardType: Data]], !items.isEmpty else { return }
        pb.writeObjects(items.map { d in
            let it = NSPasteboardItem()
            for (t, data) in d { it.setData(data, forType: t) }
            return it
        })
    }
    func pasteboardChangeCount() -> Int { NSPasteboard.general.changeCount }
    func postPaste() -> Bool {
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

/// Modifier-only push-to-talk keys via a listen-only CGEvent tap on flagsChanged (needs Input Monitoring or
/// Accessibility). Fn is another dictation app's default.
final class MacHotkeys: HotkeyMonitor, @unchecked Sendable {
    private var tap: CFMachPort?
    private var source: CFRunLoopSource?
    fileprivate var onChange: (@Sendable (HotkeyAction) -> Void)?
    fileprivate var keyCode: Int64 = 63
    fileprivate var flag: CGEventFlags = .maskSecondaryFn
    fileprivate var down = false

    func start(key: String, onChange: @escaping @Sendable (HotkeyAction) -> Void) -> Bool {
        stop()
        switch key {
        case "fn": keyCode = 63; flag = .maskSecondaryFn
        case "right-option": keyCode = 61; flag = .maskAlternate
        case "right-command": keyCode = 54; flag = .maskCommand
        case "right-control": keyCode = 62; flag = .maskControl
        default: return false
        }
        self.onChange = onChange
        let mask = CGEventMask(1 << CGEventType.flagsChanged.rawValue)
        let me = Unmanaged.passUnretained(self).toOpaque()
        guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .listenOnly,
                                          eventsOfInterest: mask, callback: { _, type, event, ctx in
            guard type == .flagsChanged, let ctx else { return Unmanaged.passUnretained(event) }
            let hk = Unmanaged<MacHotkeys>.fromOpaque(ctx).takeUnretainedValue()
            if event.getIntegerValueField(.keyboardEventKeycode) == hk.keyCode {
                let isDown = event.flags.contains(hk.flag)
                if isDown != hk.down { hk.down = isDown; hk.onChange?(isDown ? .down : .up) }
            }
            return Unmanaged.passUnretained(event)
        }, userInfo: me) else { return false }
        self.tap = tap
        let src = CFMachPortCreateRunLoopSource(nil, tap, 0)
        source = src
        CFRunLoopAddSource(CFRunLoopGetMain(), src, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
        return true
    }

    func stop() {
        if let tap { CGEvent.tapEnable(tap: tap, enable: false) }
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        tap = nil; source = nil; down = false
    }
}
#endif
