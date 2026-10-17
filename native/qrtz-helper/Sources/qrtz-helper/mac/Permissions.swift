#if os(macOS)
import AVFoundation
import ApplicationServices
import AppKit
import CoreGraphics
import QrtzHelperCore

/// TCC attributes the helper's requests to the responsible process (the qrtz.app that spawned it), so grants show
/// up under "qrtz" in System Settings. [MAC-VERIFY]
///
/// Screen recording covers ScreenCaptureKit audio. Core Audio process taps use the separate "System Audio Recording
/// Only" TCC service (kTCCServiceAudioCapture), which has no public preflight API: we report screen-recording and let
/// the first tap attempt trigger the system-audio prompt.
struct MacPermissions: PermissionsProvider {
    func status() -> [OsPermission: PermissionState] {
        let mic: PermissionState
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: mic = .granted
        case .denied, .restricted: mic = .denied
        case .notDetermined: mic = .notDetermined
        @unknown default: mic = .notDetermined
        }
        return [
            .accessibility: AXIsProcessTrusted() ? .granted : .notDetermined,
            .screenRecording: CGPreflightScreenCaptureAccess() ? .granted : .notDetermined,
            .microphone: mic,
        ]
    }

    func request(_ p: OsPermission) async {
        switch p {
        case .accessibility:
            let opts = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
            if !AXIsProcessTrustedWithOptions(opts) { open("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") }
        case .screenRecording:
            if !CGRequestScreenCaptureAccess() { open("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture") }
        case .microphone:
            switch AVCaptureDevice.authorizationStatus(for: .audio) {
            case .notDetermined: _ = await AVCaptureDevice.requestAccess(for: .audio)
            case .authorized: break
            default: open("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            }
        }
    }

    private func open(_ url: String) {
        if let u = URL(string: url) { _ = NSWorkspace.shared.open(u) }
    }
}
#endif
