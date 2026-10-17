#if os(macOS)
import Foundation
import QrtzHelperCore

func makePlatformDeps(version: String) -> HelperDeps {
    var audio: [AudioSource] = []
    var features = ["ax", "ax.observer"]
    if #available(macOS 14.2, *) { audio.append(CoreAudioTapSource()); features.append("audio.process-tap") }
    audio.append(ScreenCaptureKitSource()); features.append("audio.screencapturekit")

    var asr: [StreamingTranscriber] = [ParakeetTranscriber()]
    features.append("asr.parakeet")
    #if compiler(>=6.2) && canImport(Speech)
    if #available(macOS 26.0, *) { asr.append(SpeechAnalyzerTranscriber()); features.append("asr.speechanalyzer") }
    #endif

    let v = ProcessInfo.processInfo.operatingSystemVersion
    return HelperDeps(
        permissions: MacPermissions(), meetings: AXMeetingSource(), audio: audio, transcribers: asr,
        helperVersion: version, os: "macOS \(v.majorVersion).\(v.minorVersion).\(v.patchVersion)", features: features)
}

func logErr(_ s: String) { FileHandle.standardError.write(Data("[qrtz-helper] \(s)\n".utf8)) }
#endif
