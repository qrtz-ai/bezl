#if os(macOS)
// The Apple calls behind `--qrtz-gate-probe` (the same calls as scripts/modules/gate-probe.swift, so a module app's
// line reads like its stand-in's). No prompt unless asked.
import ApplicationServices
import AVFoundation
import CoreAudio
import CoreGraphics
import CoreServices
import Foundation

public struct MacGateProbeBackend: GateProbeBackend {
    public init() {}

    public func axTrusted(ask: Bool) -> Bool {
        ask
            ? AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as NSDictionary as CFDictionary)
            : AXIsProcessTrusted()
    }

    func micStatus() -> String {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: return "authorized"
        case .denied: return "denied"
        case .restricted: return "restricted"
        case .notDetermined: return "notDetermined"
        @unknown default: return "unknown"
        }
    }

    public func mic(ask: Bool, open: Bool) -> [(String, String)] {
        var fields = [("status", micStatus())]
        guard open else { return fields }
        final class Answer: @unchecked Sendable { var granted = false }
        let answer = Answer()
        answer.granted = AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
        if !answer.granted && ask {
            let s = DispatchSemaphore(value: 0)
            AVCaptureDevice.requestAccess(for: .audio) { answer.granted = $0; s.signal() }
            _ = s.wait(timeout: .now() + 60)
        }
        let granted = answer.granted
        var opened = "no-device"
        if let d = AVCaptureDevice.default(for: .audio) {
            opened = granted && (try? AVCaptureDeviceInput(device: d)) != nil ? "yes" : "no"
        }
        fields += [("granted", String(granted)), ("opened", opened), ("status_after", micStatus())]
        return fields
    }

    /// ADR 0150 N173: echo's speaker gate runs only with the screen & system audio recording grant already given to
    /// qrtz (a dictation never prompts). Run as a child of qrtz, this checks that the helper inherits qrtz's grant.
    public func systemAudio(ask: Bool, open: Bool) -> [(String, String)] {
        let granted = CGPreflightScreenCaptureAccess()
        var fields = [("screen", String(granted))]
        if ask && !granted {
            fields.append(("requested", String(CGRequestScreenCaptureAccess())))
            fields.append(("screen_after", String(CGPreflightScreenCaptureAccess())))
        }
        guard open else { return fields }
        guard CGPreflightScreenCaptureAccess() else { fields.append(("tap", "skipped")); return fields }
        if #available(macOS 14.2, *) {
            // The dictation tap's shape: a private global tap, created and destroyed at once.
            let desc = CATapDescription(stereoGlobalTapButExcludeProcesses: [])
            desc.isPrivate = true
            desc.muteBehavior = .unmuted
            desc.name = "qrtz gate probe"
            var tap = AudioObjectID(kAudioObjectUnknown)
            let st = AudioHardwareCreateProcessTap(desc, &tap)
            if st == noErr { AudioHardwareDestroyProcessTap(tap); fields.append(("tap", "ok")) } else { fields.append(("tap", "osstatus\(st)")) }
        } else {
            fields.append(("tap", "unsupported"))
        }
        return fields
    }

    public func automateSystemEvents(ask: Bool) -> [(String, String)] {
        let target = "com.apple.systemevents"
        var desc = AEDesc()
        let bytes = Array(target.utf8)
        _ = bytes.withUnsafeBufferPointer {
            AECreateDesc(DescType(typeApplicationBundleID), $0.baseAddress, $0.count, &desc)
        }
        defer { AEDisposeDesc(&desc) }
        let perm = AEDeterminePermissionToAutomateTarget(&desc, AEEventClass(typeWildCard), AEEventID(typeWildCard), ask)
        var out: [(String, String)] = [("permission", String(perm))]
        // -600 = System Events isn't running: the gate script starts it (open -g -b) and asks again.
        guard perm == noErr else { out.append(("sent", "no")); return out }
        var err: NSDictionary?
        let r = NSAppleScript(source: "tell application id \"\(target)\" to get name")?.executeAndReturnError(&err)
        out.append(("sent", r == nil ? "failed" : "ok"))
        if let n = err?[NSAppleScript.errorNumber] { out.append(("ae_error", "\(n)")) }
        return out
    }
}
#endif
