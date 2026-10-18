#if os(macOS)
// ctrl on a real macOS session (ADR 0100): background actions must leave the frontmost app, the cursor, the front
// window and (with Accessibility trust) the focused element exactly as they were. Runs in CI's macOS job.
// Tests that need Accessibility trust skip with a message when the test process doesn't have it; the file and
// key-state checks need no permission and always run.
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import XCTest
@testable import QrtzCtrl

/// Spin the main run loop so NSWorkspace's frontmost-app state is current (it updates from notifications).
private func spin(_ seconds: Double = 0.2, until done: () -> Bool = { false }) {
    let end = Date().addingTimeInterval(seconds)
    while Date() < end, !done() { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
}

private func fresh() -> UiSnapshot {
    spin(0.15)
    return CtrlProbe.snapshot()
}

private func describe(_ s: UiSnapshot) -> String {
    "frontmost=\(s.frontmostPid.map(String.init) ?? "nil") cursor=\(s.cursor.map { "\($0.x),\($0.y)" } ?? "nil") "
        + "focused=\(s.focusedElement == nil ? "nil" : "set") frontWindow=\(s.keyWindow.map(String.init) ?? "nil") hidIdleMs=\(s.hidIdleMs.map(String.init) ?? "nil")"
}

final class CtrlMacGuardTests: XCTestCase {
    func testProbeReadsTheSessionWithoutPrompting() {
        let s = fresh()
        print("[ctrl-probe] axTrusted=\(CtrlProbe.axTrusted) postEvent=\(CtrlProbe.postEventAllowed) secureInput=\(CtrlProbe.secureInput) \(describe(s))")
        XCTAssertNotNil(s.cursor, "the cursor position needs no permission")
        if !CtrlProbe.axTrusted { XCTAssertNil(s.focusedElement, "without trust the focused element must be unknown, not guessed") }
    }

    /// Rung 1: a downloads cleanup through FileManager. Nothing the user owns may change.
    func testDownloadsCleanupLeavesFrontmostCursorAndFocusAlone() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-ctrl-mac-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let downloads = root.appendingPathComponent("Downloads")
        let archive = downloads.appendingPathComponent("older than a week")
        try FileManager.default.createDirectory(at: archive, withIntermediateDirectories: true)
        let names = (0..<25).map { "file-\($0).zip" }
        for n in names { try Data(n.utf8).write(to: downloads.appendingPathComponent(n)) }

        let r = CtrlGuard.guarded(probe: fresh) {
            for n in names.prefix(20) { try CtrlFiles.moveInto(downloads.appendingPathComponent(n), archive) }
            for n in names.suffix(5) { try CtrlFiles.trash(downloads.appendingPathComponent(n), into: root.appendingPathComponent(".Trash")) }
        }
        XCTAssertNil(r.error)
        print("[ctrl-guard] files before: \(describe(r.before))\n[ctrl-guard] files after:  \(describe(r.after)) outcome=\(r.outcome)")
        if r.outcome.userInput { throw XCTSkip("physical input during the action; rerun without touching the mouse or keyboard") }
        XCTAssertTrue(r.outcome.clean, "violations: \(r.outcome.violations)")
        XCTAssertEqual(r.before.frontmostPid, r.after.frontmostPid)
        XCTAssertEqual(r.before.cursor, r.after.cursor)
        XCTAssertEqual(r.before.keyWindow, r.after.keyWindow)
        XCTAssertEqual(r.before.focusedElement, r.after.focusedElement)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: archive.path).count, 20)
    }

    /// Rung 5's precondition: a private event source doesn't leak modifiers into the session's key state.
    /// Posts ⌘F19 (bound to nothing) to this test process itself, so no other app can act on it.
    func testPrivateSourceKeyEventsDontTouchTheUsersModifierState() throws {
        let flagsBefore = CGEventSource.flagsState(.combinedSessionState)
        let me = ProcessInfo.processInfo.processIdentifier
        let r = CtrlGuard.guarded(probe: fresh) {
            // Posted straight to our own pid (CtrlActions.postKey would refuse if xctest were frontmost).
            guard let src = CGEventSource(stateID: .privateState),
                  let down = CGEvent(keyboardEventSource: src, virtualKey: 0x50, keyDown: true),
                  let up = CGEvent(keyboardEventSource: src, virtualKey: 0x50, keyDown: false)
            else { throw CtrlActions.Failure.eventSource }
            down.flags = .maskCommand
            up.flags = .maskCommand
            down.postToPid(me)
            up.postToPid(me)
            spin(0.2)
        }
        let flagsAfter = CGEventSource.flagsState(.combinedSessionState)
        print("[ctrl-key] postEvent=\(CtrlProbe.postEventAllowed) flags before=\(flagsBefore.rawValue) after=\(flagsAfter.rawValue) outcome=\(r.outcome)")
        XCTAssertNil(r.error)
        if r.outcome.userInput { throw XCTSkip("physical input during the action") }
        XCTAssertFalse(flagsAfter.contains(.maskCommand) && !flagsBefore.contains(.maskCommand), "⌘ leaked into the session key state")
        XCTAssertTrue(r.outcome.clean, "violations: \(r.outcome.violations)")
    }

    /// Rungs 2 + 4: open a document in TextEdit without activating it, then close its window with AXPress on the
    /// close button. Needs Accessibility trust for the AX half.
    func testBackgroundTextEditOpenAndAXCloseDontTakeOver() throws {
        guard CtrlProbe.axTrusted else {
            throw XCTSkip("ctrl AX check skipped: this test process has no Accessibility trust (AXIsProcessTrusted() == false). Run on a Mac with the terminal trusted; see docs/ctrl/LOG.md")
        }
        guard let textEdit = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.TextEdit") else {
            throw XCTSkip("TextEdit is not installed")
        }
        let wasRunning = !NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.TextEdit").isEmpty
        let doc = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-ctrl-\(UUID().uuidString).txt")
        try Data("qrtz ctrl background probe\n".utf8).write(to: doc)
        var launched: NSRunningApplication?
        defer {
            if !wasRunning, let app = launched {
                app.terminate()
                // Wait it out: a TextEdit still quitting would load the runner during later timing-sensitive tests.
                spin(5) { app.isTerminated }
                if !app.isTerminated { app.forceTerminate() }
            }
            try? FileManager.default.removeItem(at: doc)
        }

        // Rung 2: open without activation. Measured, not assumed: some launches activate anyway.
        let before0 = fresh()
        var openErr: Error?
        var finished = false
        CtrlActions.open([doc], withApp: textEdit) { app, err in
            launched = app
            openErr = err
            finished = true
        }
        spin(10) { finished }
        XCTAssertNil(openErr)
        guard let app = launched else { return XCTFail("TextEdit did not open the document within 10 s") }
        spin(10) { !CtrlActions.windows(app.processIdentifier).isEmpty }
        let after0 = fresh()
        let launch = CtrlGuard.compare(before0, after0, elapsedMs: 0)
        print("[ctrl-launch] wasRunning=\(wasRunning) before: \(describe(before0))\n[ctrl-launch] after:  \(describe(after0)) outcome=\(launch)")
        if after0.frontmostPid == app.processIdentifier, before0.frontmostPid != app.processIdentifier {
            let restored = before0.frontmostPid.map { CtrlActions.restoreFrontmost($0) } ?? false
            spin(1)
            print("[ctrl-launch] TextEdit activated on open (activates=false was not honored); restore requested=\(restored) frontmost now=\(fresh().frontmostPid.map(String.init) ?? "nil")")
            if fresh().frontmostPid == app.processIdentifier {
                throw XCTSkip("opening the document activated TextEdit and the restore was denied; the background AX press can't be judged here (recorded in the log above)")
            }
        }

        // Rung 4: close the document window from the background.
        let pid = app.processIdentifier
        let windowsBefore = CtrlActions.windows(pid).count
        guard let window = CtrlActions.windows(pid).first, let close = CtrlActions.closeButton(window) else {
            return XCTFail("no AX window / close button on the background TextEdit document")
        }
        let r = CtrlGuard.guarded(probe: fresh) {
            try CtrlActions.perform(kAXPressAction, on: close)
            spin(5) { CtrlActions.windows(pid).count < windowsBefore }
        }
        print("[ctrl-ax] before: \(describe(r.before))\n[ctrl-ax] after:  \(describe(r.after)) outcome=\(r.outcome) windows \(windowsBefore)->\(CtrlActions.windows(pid).count)")
        XCTAssertNil(r.error)
        XCTAssertLessThan(CtrlActions.windows(pid).count, windowsBefore, "AXPress on the close button did not close the background window")
        if r.outcome.userInput { throw XCTSkip("physical input during the action") }
        XCTAssertTrue(r.outcome.clean, "background AXPress changed what the user owns: \(r.outcome.violations)")
        XCTAssertEqual(r.before.frontmostPid, r.after.frontmostPid)
        XCTAssertEqual(r.before.cursor, r.after.cursor)
    }

    /// Opt-in measurement for a real Mac (QRTZ_CTRL_MAC_PROBES=1; never in CI): AXSetValue and a postToPid
    /// keystroke into a background TextEdit document, each between guard snapshots. Prints whether the write and the
    /// key landed. Skips if TextEdit is already running, so none of Jared's documents are touched; the TextEdit it
    /// launched is force-quit at the end (the probe document is a temp file).
    func testMacProbeBackgroundWritesAndPidKeys() throws {
        guard ProcessInfo.processInfo.environment["QRTZ_CTRL_MAC_PROBES"] == "1" else { throw XCTSkip("opt-in: QRTZ_CTRL_MAC_PROBES=1") }
        guard CtrlProbe.axTrusted else { throw XCTSkip("needs Accessibility trust (AXIsProcessTrusted() == false)") }
        guard NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.TextEdit").isEmpty else {
            throw XCTSkip("TextEdit is running; quit it first so the probe can't touch your documents")
        }
        guard let textEdit = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.TextEdit") else { throw XCTSkip("no TextEdit") }
        let doc = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-ctrl-probe-\(UUID().uuidString).txt")
        try Data("probe\n".utf8).write(to: doc)
        var launched: NSRunningApplication?
        var finished = false
        let before0 = fresh()
        CtrlActions.open([doc], withApp: textEdit) { app, _ in
            launched = app
            finished = true
        }
        spin(10) { finished }
        defer {
            if let app = launched {
                app.forceTerminate()
                spin(5) { app.isTerminated }
            }
            try? FileManager.default.removeItem(at: doc)
        }
        guard let app = launched else { return XCTFail("TextEdit did not open") }
        let pid = app.processIdentifier
        spin(10) { !CtrlActions.windows(pid).isEmpty }
        print("[ctrl-probe-mac] launch outcome=\(CtrlGuard.compare(before0, fresh(), elapsedMs: 0))")
        guard let window = CtrlActions.windows(pid).first else { return XCTFail("no window") }
        func find(_ el: AXUIElement, role want: String, depth: Int = 0) -> AXUIElement? {
            if CtrlActions.role(el) == want { return el }
            guard depth < 8, let kids = CtrlActions.raw(el, kAXChildrenAttribute), CFGetTypeID(kids) == CFArrayGetTypeID() else { return nil }
            for k in (((kids as! CFArray) as NSArray) as [AnyObject]) {
                if let e = CtrlActions.element(k as CFTypeRef), let hit = find(e, role: want, depth: depth + 1) { return hit }
            }
            return nil
        }
        guard let text = find(window, role: "AXTextArea") else { return XCTFail("no AXTextArea") }
        func value() -> String { (CtrlActions.raw(text, kAXValueAttribute) as? String) ?? "" }

        let w = CtrlGuard.guarded(probe: fresh) { try CtrlActions.set(kAXValueAttribute, "ctrl wrote this" as CFString, on: text) }
        print("[ctrl-probe-mac] AXSetValue landed=\(value() == "ctrl wrote this") err=\(String(describing: w.error)) outcome=\(w.outcome)")
        XCTAssertTrue(w.outcome.clean || w.outcome.userInput, "AXSetValue on a background app took over: \(w.outcome.violations)")

        let lenBefore = value().count
        let k = CtrlGuard.guarded(probe: fresh) {
            try CtrlActions.postKey(0x00, flags: [], combo: "a", to: pid) // kVK_ANSI_A
            spin(0.5)
        }
        print("[ctrl-probe-mac] postToPid 'a' landed=\(value().count > lenBefore) value_len=\(value().count) postEvent=\(CtrlProbe.postEventAllowed) err=\(String(describing: k.error)) outcome=\(k.outcome)")
        XCTAssertTrue(k.outcome.clean || k.outcome.userInput, "postToPid to a background app took over: \(k.outcome.violations)")
    }

    /// The executor refuses forbidden AX actions before they reach the target.
    func testForbiddenAXActionsAreRefusedBeforeTheCall() throws {
        guard CtrlProbe.axTrusted else { throw XCTSkip("needs Accessibility trust (AXIsProcessTrusted() == false)") }
        let finder = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.finder").first
        guard let finder else { throw XCTSkip("Finder is not running") }
        let el = CtrlActions.appElement(finder.processIdentifier)
        XCTAssertThrowsError(try CtrlActions.perform("AXRaise", on: el)) { e in
            XCTAssertEqual(e as? CtrlActions.Failure, .refused(.forbiddenAction("AXRaise")))
        }
        XCTAssertThrowsError(try CtrlActions.set("AXFrontmost", kCFBooleanTrue, on: el)) { e in
            XCTAssertEqual(e as? CtrlActions.Failure, .refused(.forbiddenAttribute("AXFrontmost")))
        }
    }
}
#endif
