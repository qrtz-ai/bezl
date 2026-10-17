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
    deps.systemTranscribers = [NemotronTranscriber()]
    deps.diarizers = [.mic: NemotronChannelDiarizer(), .system: NemotronChannelDiarizer()]
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
