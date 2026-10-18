#if os(macOS)
import Foundation
import QrtzEcho
import QrtzMacShared
import QrtzShared

// ADR 0146 M2.5: echo's macOS seams, for qrtz-echo (its own mic hub) and the monolith (one hub shared with nano).

/// Echo's platform on a mic hub; `modelLoads` is the load gate its startup preload goes through (the monolith shares
/// nano's). Returns the features echo reports in its hello.
public func makeEchoPlatform(micHub: MicHub, modelLoads: ModelLoads = ModelLoads()) -> (deps: EchoDeps, features: [String]) {
    var features = ["talk.mic", "talk.insert.ax", "talk.insert.paste", "talk.hotkey", "talk.asr.parakeet"]
    var deps = EchoDeps(dictationMic: micHub.view("dictation"), dictationTranscribers: dictationAsr(), insertion: MacInsertion(),
                        hotkeys: MacHotkeys(), modelLoads: modelLoads)
    // N143: echo gates the mic against the speakers (QRTZ_ECHO_GATE=0 turns it off).
    if #available(macOS 14.2, *), ProcessInfo.processInfo.environment["QRTZ_ECHO_GATE"] != "0" {
        deps.dictationTap = MacDictationTap()
        features.append("talk.echo-gate")
    }
    // ADR 0092: the native echo pill, once the host asks for it (it then stops showing its web pill).
    if ProcessInfo.processInfo.environment["QRTZ_NATIVE_PILL"] == "1" {
        deps.pill = NativePill()
        features.append("talk.pill.native")
    }
    return (deps, features)
}

/// Echo's recognisers (separate instances from the recording's, so a dictation can run during one): Parakeet TDT v2
/// segmented at pauses (Jared, 2026-10-18; the rule from `QRTZ_ECHO_SEGMENT_PAUSE_MS` / `QRTZ_ECHO_SEGMENT_CAP_MS`),
/// SpeechAnalyzer as the fallback. Nemotron is the recording's only (loaded per recording), not resident for echo.
private func dictationAsr() -> [StreamingTranscriber] {
    var asr: [StreamingTranscriber] = [SegmentedTranscriber(ParakeetBatch(), rule: .fromEnvironment())]
    #if compiler(>=6.2) && canImport(Speech)
    if #available(macOS 26.0, *) { asr.append(SpeechAnalyzerTranscriber()) }
    #endif
    return asr
}
#endif
