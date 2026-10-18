#if os(macOS)
import Foundation
import QrtzHelperCore

func makePlatformDeps(version: String) -> HelperDeps {
    // Shared between the AX layer (who is speaking, who am I) and the audio path (mic echo gate).
    let context = MeetingContext()
    var audio: [QrtzHelperCore.AudioSource] = []
    var features = ["ax", "ax.observer"]
    // H6 mux: one AVAudioEngine mic tap shared by meeting capture and dictation.
    let micHub = MicHub(source: DictationMicSource())
    if #available(macOS 14.2, *) { audio.append(CoreAudioTapSource(mic: micHub.view("recording", gated: true))); features.append("audio.process-tap") }
    audio.append(ScreenCaptureKitSource(mic: micHub.view("recording", gated: true))); features.append("audio.screencapturekit")
    features.append("audio.echo-gate")
    features += ["talk.mic", "talk.insert.ax", "talk.insert.paste", "talk.hotkey", "helper.mux", "permissions.watch"]
    features.append("audio.aec.localvqe")

    // ADR 0079: Nemotron (local files only) on the mic and system channels, each diarized; SpeechAnalyzer on the mix
    // as the backup while Nemotron isn't downloaded (only with OS assets already installed). Echo never diarizes.
    var asr: [StreamingTranscriber] = [NemotronTranscriber()]
    features += ["asr.nemotron", "diarize.nemotron3"]
    #if compiler(>=6.2) && canImport(Speech)
    if #available(macOS 26.0, *) { asr.append(SpeechAnalyzerTranscriber()); features.append("asr.speechanalyzer") }
    #endif

    // ADR 0075: on-device summaries (Apple Intelligence). Availability is checked per request.
    #if canImport(FoundationModels)
    features.append("summarize.foundation-models")
    #endif

    let v = ProcessInfo.processInfo.operatingSystemVersion
    var deps = HelperDeps(
        permissions: MacPermissions(), meetings: AXMeetingSource(context: context), audio: audio, transcribers: asr,
        helperVersion: version, os: "macOS \(v.majorVersion).\(v.minorVersion).\(v.patchVersion)", features: features,
        context: context, enhancer: LocalVqeEnhancer(),
        dictationMic: micHub.view("dictation"), dictationTranscribers: dictationAsr(), insertion: MacInsertion(), hotkeys: MacHotkeys(), micHub: micHub)
    deps.summarizer = FoundationModelsSummarizer()
    // N143: echo gates the mic against the speakers (QRTZ_ECHO_GATE=0 turns it off).
    if #available(macOS 14.2, *), ProcessInfo.processInfo.environment["QRTZ_ECHO_GATE"] != "0" {
        deps.dictationTap = MacDictationTap()
        deps.features.append("talk.echo-gate")
    }
    deps.systemTranscribers = [NemotronTranscriber()]
    deps.diarizers = [.mic: NemotronChannelDiarizer(), .system: NemotronChannelDiarizer()]
    let loads = deps.modelLoads
    deps.modelWarmup = { await warmModels(loads) }
    // ADR 0092: the native echo pill, once the host asks for it (it then stops showing its web pill).
    if ProcessInfo.processInfo.environment["QRTZ_NATIVE_PILL"] == "1" {
        deps.pill = NativePill()
        deps.features.append("talk.pill.native")
    }
    return deps
}

/// Separate recogniser instances for qrtz talk so a dictation can run during a meeting recording.
private func dictationAsr() -> [StreamingTranscriber] {
    var asr: [StreamingTranscriber] = [NemotronTranscriber()]
    #if compiler(>=6.2) && canImport(Speech)
    if #available(macOS 26.0, *) { asr.append(SpeechAnalyzerTranscriber()) }
    #endif
    return asr
}

func logErr(_ s: String) { FileHandle.standardError.write(Data("[qrtz-helper] \(s)\n".utf8)) }
#endif

#if os(macOS)
/// `models.warm` (ADR 0079 / N121): each model loaded once and unloaded, one at a time (peak = one model), so Core ML
/// compiles and caches them now. Fresh instances: nothing a recording or talk holds is touched.
func warmModels(_ loads: ModelLoads) async -> String {
    func ms(_ t: Date) -> Int { Int(Date().timeIntervalSince(t) * 1000) }
    var parts: [String] = []
    // N127: a recording that starts cancels the warm; it stops between models (each one is unloaded right after its
    // load, so nothing stays resident next to the recording's models).
    func cancelled() -> String? { Task.isCancelled ? "models.warm cancelled (recording started) after: " + (parts.isEmpty ? "none" : parts.joined(separator: ", ")) : nil }
    // N137: each load and its unload hold that model's gate, so a recording starting meanwhile waits for this
    // compile and then loads from the cache, never compiling next to it.
    let asr = NemotronTranscriber()
    parts.append((try? await loads.load("asr") { () async -> String in
        let t0 = Date()
        do { try await asr.prepare() } catch { await asr.unload(); return "asr unavailable: \(error)" }
        let r = "asr \(Int(Date().timeIntervalSince(t0) * 1000)) ms"
        await asr.unload()
        return r
    }) ?? "asr skipped")
    if let c = cancelled() { return c }
    let diar = NemotronChannelDiarizer()
    parts.append((try? await loads.load("diarizer") { () async -> String in
        let t0 = Date()
        do { try await diar.prepare() } catch { await diar.unload(); return "diarizer unavailable: \(error)" }
        let r = "diarizer \(Int(Date().timeIntervalSince(t0) * 1000)) ms"
        await diar.unload()
        return r
    }) ?? "diarizer skipped")
    if let c = cancelled() { return c }
    let t = Date()
    let vqe = LocalVqeEnhancer()
    do { try await vqe.prepare(); parts.append("echo \(ms(t)) ms") } catch { parts.append("echo unavailable: \(error)") }
    return "models warmed: " + parts.joined(separator: ", ")
}
#endif
