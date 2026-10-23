#if os(macOS)
import AppKit
import ApplicationServices
import Carbon.HIToolbox
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

    /// ADR 0225: a password field (role or subrole `AXSecureTextField`, `SecureField`) or secure event input on.
    /// An element whose role can't be read is not taken as secure; secure event input still covers most prompts.
    public func focusedFieldIsSecure() -> Bool {
        if IsSecureEventInputEnabled() { return true }
        guard let el = focused() else { return false }
        return isSecure(el)
    }

    private func isSecure(_ el: AXUIElement) -> Bool {
        func str(_ a: String) -> String? {
            var v: CFTypeRef?
            guard AXUIElementCopyAttributeValue(el, a as CFString, &v) == .success else { return nil }
            return v as? String
        }
        return SecureField.isSecure(role: str(kAXRoleAttribute as String), subrole: str(kAXSubroleAttribute as String))
    }

    public func focusedAppName() -> String? { NSWorkspace.shared.frontmostApplication?.localizedName }

    public func axSetSelectedText(_ text: String) -> Bool {
        guard let el = focused(), !isSecure(el), !IsSecureEventInputEnabled() else { return false }
        var settable: DarwinBoolean = false
        guard AXUIElementIsAttributeSettable(el, kAXSelectedTextAttribute as CFString, &settable) == .success, settable.boolValue else { return false }
        // Some apps (Electron/Chromium, many web fields) report success but ignore the write: verify the value changed.
        let before = value(el)
        guard AXUIElementSetAttributeValue(el, kAXSelectedTextAttribute as CFString, text as CFString) == .success else { return false }
        guard let b = before else { return true }
        return value(el) != b
    }

    public func axInsertIntoValue(_ text: String) -> Bool {
        guard let el = focused(), !isSecure(el), !IsSecureEventInputEnabled(), let cur = value(el) else { return false }
        // QRT-356: Slate's U+FEFF markers make an empty-with-hint box and a real doc indistinguishable here; never rewrite
        // the whole value then (it could wipe the doc). Fall through to paste at the caret instead.
        guard FieldText.classify(value: cur, placeholder: nil).canOverwriteValue else { return false }
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

    public func focusedValue() -> String? {
        guard let el = focused(), !isSecure(el) else { return nil }
        return value(el)
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
        if focusedFieldIsSecure() { return }  // ADR 0225: never stage words for a password field
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
        guard AXIsProcessTrusted(), !focusedFieldIsSecure() else { return false }
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
///
/// Oct 18 (Jared, Developer ID build): the taps were made once, at `dictation.configure`, which on a fresh install
/// runs before Accessibility is granted. The blocker was then never made, so every fn press also ran macOS's globe
/// action (with a second input source enabled, "change input source"), and a listener that could not be made was
/// never tried again. Now a 2 s retry runs until both exist: the moment Accessibility is on, the blocker and
/// listener are rebuilt (in that order), with no restart. The edge state carries over, so a held key stays held.
public final class MacHotkeys: HotkeyMonitor, @unchecked Sendable {
    private let lock = NSLock()
    private var listenTap: CFMachPort?
    private var listenSource: CFRunLoopSource?
    private var blockTap: CFMachPort?
    private var blockSource: CFRunLoopSource?
    private var retry: DispatchSourceTimer?
    /// N196: NSWorkspace sleep/wake observers (main queue, like the taps' callbacks).
    private var powerObservers: [NSObjectProtocol] = []
    private var key = ""
    fileprivate var onChange: (@Sendable (HotkeyAction) -> Void)?
    /// Edge state (listen tap only).
    fileprivate var filter: HotkeyFilter?
    /// Consume decisions (block tap only; its edges are ignored).
    fileprivate var blocker: HotkeyFilter?
    /// How the taps are checked for (tests swap them).
    var trusted: () -> Bool = { AXIsProcessTrusted() }
    var retryInterval: DispatchTimeInterval = .seconds(2)

    public init() {}

    /// ADR 0216: read on the tap's thread right after the filter reported the edge.
    public var pressWithControl: Bool { filter?.withControl ?? false }
    /// R2 (ADR 0216): set only while a release the helper made up (sleep, wake, tap disabled) is reported (main
    /// queue / main run loop), so a ctrl+fn command runs nothing.
    let synthetic = SyntheticRelease()
    public var releaseAtSleep: Bool { synthetic.active }

    public func start(key: String, consumeFn: Bool, onChange: @escaping @Sendable (HotkeyAction) -> Void) -> Bool {
        stop()
        guard let f = HotkeyFilter(key: key, consumeFn: consumeFn) else { return false }
        lock.lock(); defer { lock.unlock() }
        filter = f
        self.key = key
        self.onChange = onChange
        if f.consumes && trusted() { installBlocker() }
        let ok = installListener()
        if !ok { removeBlocker() }  // never swallow the key with nothing listening for it
        if !complete { scheduleRetry() }
        if ok { observePower() }
        return ok
    }

    /// N196 (ADR 0224): a lid closed while holding the push key can lose the release, leaving dictation stuck
    /// listening. On sleep a held key ends; on wake one still believed held ends if the session's flags say it is up.
    private func observePower() {
        let nc = NSWorkspace.shared.notificationCenter
        let sleep = nc.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in
            guard let self else { return }
            self.synthetic.report(self.filter?.willSleep(), to: self.onChange)
        }
        let wake = nc.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            guard let self else { return }
            let flags = CGEventSource.flagsState(.combinedSessionState).rawValue
            self.synthetic.report(self.filter?.didWake(flags: flags), to: self.onChange)
        }
        powerObservers = [sleep, wake]
    }

    private func removePowerObservers() {
        let nc = NSWorkspace.shared.notificationCenter
        for o in powerObservers { nc.removeObserver(o) }
        powerObservers = []
    }

    public func stop() {
        lock.lock(); defer { lock.unlock() }
        retry?.cancel(); retry = nil
        removePowerObservers()
        removeListener()
        removeBlocker()
        filter = nil; blocker = nil
    }

    /// Both taps this key wants exist.
    private var complete: Bool { listenTap != nil && (filter?.consumes != true || blockTap != nil) }

    private func scheduleRetry() {
        guard retry == nil else { return }
        let t = DispatchSource.makeTimerSource(queue: .main)
        t.schedule(deadline: .now() + retryInterval, repeating: retryInterval)
        t.setEventHandler { [weak self] in self?.retryTick() }
        retry = t
        t.resume()
    }

    /// One retry: the listener if it is missing; once Accessibility is on, the blocker (and the listener again,
    /// after it, so the listener keeps the head).
    func retryTick() {
        lock.lock(); defer { lock.unlock() }
        guard let f = filter else { retry?.cancel(); retry = nil; return }
        let wantBlock = f.consumes && blockTap == nil && trusted()
        if wantBlock || listenTap == nil {
            removeListener()
            if wantBlock { installBlocker() }
            if !installListener() { removeBlocker() }
        }
        if complete { retry?.cancel(); retry = nil }
    }

    private func installBlocker() {
        let me = Unmanaged.passUnretained(self).toOpaque()
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
        guard let t = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap,
                                        eventsOfInterest: mask, callback: block, userInfo: me) else { return }
        blockTap = t
        let src = CFMachPortCreateRunLoopSource(nil, t, 0)
        blockSource = src
        CFRunLoopAddSource(CFRunLoopGetMain(), src, .commonModes)
        CGEvent.tapEnable(tap: t, enable: true)
    }

    private func installListener() -> Bool {
        let me = Unmanaged.passUnretained(self).toOpaque()
        let listen: CGEventTapCallBack = { _, type, event, ctx in
            guard let ctx else { return Unmanaged.passUnretained(event) }
            let hk = Unmanaged<MacHotkeys>.fromOpaque(ctx).takeUnretainedValue()
            if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
                // Bozniak MEDIUM (ADR 0216): a disabled tap's made-up release is synthetic too: a held ctrl+fn
                // runs nothing (plain fn still inserts what it heard).
                hk.synthetic.report(hk.filter?.reset(), to: hk.onChange)
                if let t = hk.listenTap { CGEvent.tapEnable(tap: t, enable: true) }
                return Unmanaged.passUnretained(event)
            }
            guard type == .flagsChanged, var f = hk.filter else { return Unmanaged.passUnretained(event) }
            let r = f.handle(type: type.rawValue, keyCode: event.getIntegerValueField(.keyboardEventKeycode),
                             flags: event.flags.rawValue,
                             sourceState: event.getIntegerValueField(.eventSourceStateID),
                             userData: event.getIntegerValueField(.eventSourceUserData))
            hk.filter = f
            if let a = r.action { hk.onChange?(a) }
            return Unmanaged.passUnretained(event)
        }
        guard let t = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .listenOnly,
                                        eventsOfInterest: CGEventMask(1 << CGEventType.flagsChanged.rawValue),
                                        callback: listen, userInfo: me) else { return false }
        listenTap = t
        let src = CFMachPortCreateRunLoopSource(nil, t, 0)
        listenSource = src
        CFRunLoopAddSource(CFRunLoopGetMain(), src, .commonModes)
        CGEvent.tapEnable(tap: t, enable: true)
        return true
    }

    private func removeListener() {
        if let t = listenTap { CGEvent.tapEnable(tap: t, enable: false); CFMachPortInvalidate(t) }
        if let s = listenSource { CFRunLoopRemoveSource(CFRunLoopGetMain(), s, .commonModes) }
        listenTap = nil; listenSource = nil
    }

    private func removeBlocker() {
        if let t = blockTap { CGEvent.tapEnable(tap: t, enable: false); CFMachPortInvalidate(t) }
        if let s = blockSource { CFRunLoopRemoveSource(CFRunLoopGetMain(), s, .commonModes) }
        blockTap = nil; blockSource = nil
    }
}
#endif
