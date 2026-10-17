// N29: silent-capture detection, per source. The host only sees the mixed meter level; the helper sees the system
// and mic channels apart, so it reports which one has been dead quiet for a minute (wrong device, muted input,
// ScreenCaptureKit delivering zeros) and when sound comes back. Pure value type: the controller feeds it blocks.
import Foundation

/// A capture channel, as `capture.silent` / `capture.audible` name it.
public enum CaptureSource: String, Sendable, CaseIterable { case system, mic }

/// When a source counts as silent. Mirrors TS `CAPTURE_SILENCE` in packages/core/src/helper/protocol.ts.
public struct SilenceConfig: Sendable, Equatable {
    /// Block RMS (linear, full scale 1.0) under which a source is silent. 0.0003 ≈ −70 dBFS: digital silence, a
    /// muted input or zeros, well under the noise floor of a quiet room through a real microphone.
    public var floorRms: Double
    /// How long a source must stay under the floor before `capture.silent` is sent.
    public var afterMs: Double
    public init(floorRms: Double, afterMs: Double) { self.floorRms = floorRms; self.afterMs = afterMs }
    public static let `default` = SilenceConfig(floorRms: 0.0003, afterMs: 60_000)
}

/// Per-source silence state for one recording. `update` returns the message to send, if any: one `capture.silent`
/// per silent stretch (after `afterMs`), and one `capture.audible` when that source is above the floor again.
public struct SilenceTracker: Sendable {
    public let config: SilenceConfig
    private var quietSince: [CaptureSource: Double] = [:]
    private var reported: Set<CaptureSource> = []
    public init(config: SilenceConfig = .default) { self.config = config }

    /// `rms` of the source's latest block at `atMs` (recording-relative ms).
    public mutating func update(_ source: CaptureSource, rms: Double, atMs: Double) -> HelperMessage? {
        if rms < config.floorRms {
            let since = quietSince[source] ?? atMs
            quietSince[source] = since
            if !reported.contains(source), atMs - since >= config.afterMs {
                reported.insert(source)
                return .captureSilent(source: source, sinceMs: since)
            }
            return nil
        }
        quietSince[source] = nil
        if reported.remove(source) != nil { return .captureAudible(source: source) }
        return nil
    }

    /// Paused: nothing is captured, so no stretch of silence runs across a pause (a reported one stays reported).
    public mutating func pause() { quietSince.removeAll() }

    public func isSilent(_ source: CaptureSource) -> Bool { reported.contains(source) }
}
