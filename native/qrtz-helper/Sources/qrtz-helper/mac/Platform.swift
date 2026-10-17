#if os(macOS)
import Foundation
import QrtzHelperCore

func makePlatformDeps(version: String) -> HelperDeps {
    // Shared between the AX layer (who is speaking, who am I) and the audio path (mic echo gate).
    let context = MeetingContext()
    var audio: [AudioSource] = []
    var features = ["ax", "ax.observer"]
    // H6 mux: one AVAudioEngine mic tap shared by meeting capture and dictation.
    let micHub = MicHub(source: DictationMicSource())
    if #available(macOS 14.2, *) { audio.append(CoreAudioTapSource(mic: micHub.view("recording", gated: true))); features.append("audio.process-tap") }
    audio.append(ScreenCaptureKitSource(mic: micHub.view("recording", gated: true))); features.append("audio.screencapturekit")
    features.append("audio.echo-gate")
    features += ["talk.mic", "talk.insert.ax", "talk.insert.paste", "talk.hotkey", "helper.mux", "permissions.watch"]
    features.append("audio.aec.localvqe")

    var asr: [StreamingTranscriber] = [ParakeetTranscriber()]
    features.append("asr.parakeet")
    #if compiler(>=6.2) && canImport(Speech)
    if #available(macOS 26.0, *) { asr.append(SpeechAnalyzerTranscriber()); features.append("asr.speechanalyzer") }
    #endif

    let v = ProcessInfo.processInfo.operatingSystemVersion
    return HelperDeps(
        permissions: MacPermissions(), meetings: AXMeetingSource(context: context), audio: audio, transcribers: asr,
        helperVersion: version, os: "macOS \(v.majorVersion).\(v.minorVersion).\(v.patchVersion)", features: features,
        context: context, enhancer: LocalVqeEnhancer(),
        dictationMic: micHub.view("dictation"), dictationTranscribers: dictationAsr(), insertion: MacInsertion(), hotkeys: MacHotkeys(), micHub: micHub)
}

/// Separate recogniser instances for qrtz talk so a dictation can run during a meeting recording.
private func dictationAsr() -> [StreamingTranscriber] {
    var asr: [StreamingTranscriber] = [ParakeetTranscriber()]
    #if compiler(>=6.2) && canImport(Speech)
    if #available(macOS 26.0, *) { asr.append(SpeechAnalyzerTranscriber()) }
    #endif
    return asr
}

func logErr(_ s: String) { FileHandle.standardError.write(Data("[qrtz-helper] \(s)\n".utf8)) }
#endif
