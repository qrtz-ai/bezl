import Foundation

// Echo tuning (docs/echo/dictation-tuning.md, ADR 0099): where one dictation's time went, measured by the engine
// itself. One `echo timing:` info line per dictation goes to the host log (~/Library/Logs/qrtz/qrtz.log, target
// `helper`), and the bench reads the same record. Content-free by construction (ADR 0083): only durations, counts,
// flags and the recogniser's name; never text, never a word of it.

/// One dictation's stage timings (ms). Filled by `DictationEngine` as the stages run; -1 = the stage didn't run.
public struct DictationTiming: Sendable, Equatable {
    /// The recogniser that heard it ("" when none did).
    public var asr = ""
    /// Audio fed to the recogniser (pre-roll included), and the pre-roll part of it.
    public var audioMs = 0
    public var preRollMs = 0
    /// `dictation.start` → the first non-empty partial (poll granularity: 100 ms). -1: none before the release.
    public var firstPartialMs = -1
    /// Recogniser `append` calls during the dictation (live audio only, not the pad): count, total and worst.
    public var appends = 0
    public var appendTotalMs = 0.0
    public var appendMaxMs = 0.0
    /// Release → the end of each stage: the tail sleep, the mic drain (next block + barrier), close() (appends in
    /// flight), the pad + finish (the final decode).
    public var tailMs = -1
    public var drainMs = -1
    public var closeMs = -1
    public var finalizeMs = -1
    /// Release → `dictation.stopped`.
    public var releaseToStoppedMs = -1
    /// Audio still queued in the mic stream at the end of the tail.
    public var queuedAtReleaseMs = 0
    public var drained = true
    /// The final decode outlived its limit (the last partial was sent) / an append outlived close() (N131).
    public var finalTimedOut = false
    public var appendHeld = false
    /// Words in the final text (a count only).
    public var words = 0
    /// Echo gate: mic blocks held down / gated blocks seen (0/0 = no gate ran).
    public var gateDucked = 0
    public var gateBlocks = 0
    /// The press opened the mic before the host's `dictation.start` (armed): `preRollMs` is then the head that the
    /// round trip would have cut off.
    public var armed = false
    /// A segmented batch recogniser (echo on Parakeet): its segment counts and the release's tail decode. nil for
    /// a streaming recogniser.
    public var segments: SegmentStats?

    public init() {}

    /// Recogniser compute per second of audio during the dictation (append time / audio time); 0 = no audio.
    public var liveRtf: Double { audioMs > 0 ? appendTotalMs / Double(audioMs) : 0 }

    /// The log line. Only numbers, flags and the recogniser's name (a fixed identifier, not user content).
    public var logLine: String {
        func r(_ v: Double) -> String { String(format: "%.1f", v) }
        let asrName = asr.isEmpty ? "none" : String(asr.unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) || "-_.".unicodeScalars.contains($0) }.prefix(48))
        var p = [
            "asr=\(asrName)", "audio_ms=\(audioMs)", "preroll_ms=\(preRollMs)", "first_partial_ms=\(firstPartialMs)",
            "appends=\(appends)", "append_total_ms=\(r(appendTotalMs))", "append_max_ms=\(r(appendMaxMs))",
            "live_rtf=\(String(format: "%.3f", liveRtf))",
            "tail_ms=\(tailMs)", "drain_ms=\(drainMs)", "close_ms=\(closeMs)", "finalize_ms=\(finalizeMs)",
            "release_to_stopped_ms=\(releaseToStoppedMs)", "queued_ms=\(queuedAtReleaseMs)", "word_count=\(words)",
        ]
        if !drained { p.append("drain_timed_out=1") }
        if finalTimedOut { p.append("final_timed_out=1") }
        if appendHeld { p.append("append_held=1") }
        if gateBlocks > 0 { p.append("gate_ducked=\(gateDucked)/\(gateBlocks)") }
        if armed { p.append("armed=1") }
        if let g = segments {
            p += ["segments=\(g.segments)", "bg_segments=\(g.backgroundSegments)", "cap_cuts=\(g.capCuts)",
                  "tail_decode_ms=\(g.tailDecodeMs < 0 ? "-1" : r(g.tailDecodeMs))", "backlog_wait_ms=\(r(g.backlogWaitMs))"]
            if g.failures > 0 { p.append("decode_failures=\(g.failures)") }
            // ADR 0120 cut windows (only when they ran: the knob is off by default).
            if g.cutWindows + g.windowsSkipped > 0 {
                p += ["cut_windows=\(g.cutWindows)", "window_fixes=\(g.windowFixes)", "windows_skipped=\(g.windowsSkipped)"]
            }
            // ADR 0127 gated segment peak (only when the gate is on: off by default).
            if g.peakGate { p.append("peak_gained=\(g.peakGained)") }
        }
        return "echo timing: " + p.joined(separator: " ")
    }

    /// For the bench's JSON.
    public var json: [String: Any] {
        var j: [String: Any] = [
            "asr": asr, "audioMs": audioMs, "preRollMs": preRollMs, "firstPartialMs": firstPartialMs,
            "appends": appends, "appendTotalMs": appendTotalMs, "appendMaxMs": appendMaxMs, "liveRtf": liveRtf,
            "tailMs": tailMs, "drainMs": drainMs, "closeMs": closeMs, "finalizeMs": finalizeMs,
            "releaseToStoppedMs": releaseToStoppedMs, "queuedAtReleaseMs": queuedAtReleaseMs, "drained": drained,
            "finalTimedOut": finalTimedOut, "appendHeld": appendHeld, "words": words,
            "gateDucked": gateDucked, "gateBlocks": gateBlocks, "armed": armed,
        ]
        if let g = segments {
            j["segments"] = g.segments; j["backgroundSegments"] = g.backgroundSegments; j["capCuts"] = g.capCuts
            j["tailDecodeMs"] = g.tailDecodeMs; j["backlogWaitMs"] = g.backlogWaitMs; j["decodeFailures"] = g.failures
            j["cutWindows"] = g.cutWindows; j["windowFixes"] = g.windowFixes; j["windowsSkipped"] = g.windowsSkipped
            if g.peakGate { j["peakGained"] = g.peakGained }
        }
        return j
    }
}

extension ContinuousClock.Instant {
    /// Milliseconds from this instant to now (fractional).
    func msToNow() -> Double {
        let d = self.duration(to: .now)
        return Double(d.components.seconds) * 1000 + Double(d.components.attoseconds) / 1e15
    }
}
