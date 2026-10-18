#if os(macOS)
import Foundation
import QrtzShared

// ADR 0146 M2.5: split out of qrtz-helper/mac/Talk.swift (owner: shared: the mic every app's MicHub wraps).

/// The physical mic (one AVAudioEngine input tap). Wrapped by MicHub so meeting capture and dictation share it (H6).
public final class DictationMicSource: MicSource, @unchecked Sendable {
    public init() {}
    private var mic: MicCapture?
    private var lost: (@Sendable (String) -> Void)?
    public func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws {
        let m = MicCapture()
        m.onLost = lost
        try m.start(onSamples)
        mic = m
    }
    public func stop() { mic?.stop(); mic = nil }
    public func flush() { mic?.flush() }
    /// N138: MicHub registers here; each MicCapture reports a device that didn't come back.
    public func onDeviceLost(_ cb: @escaping @Sendable (String) -> Void) { lost = cb }
}
#endif
