#if os(macOS)
import Foundation
import QrtzMacShared
import QrtzNano
import QrtzShared

// ADR 0146 M2.5: nano's macOS seams, for qrtz-nano (its own mic hub) and the monolith (one hub shared with echo).

/// Nano's platform on a mic hub; `modelLoads` is the load gate its warm-up and recordings share. Returns the features
/// nano reports in its hello.
public func makeNanoPlatform(micHub: MicHub, modelLoads: ModelLoads = ModelLoads()) -> (deps: NanoDeps, features: [String]) {
    // Shared between the AX layer (who is speaking, who am I) and the audio path (mic echo gate).
    let context = MeetingContext()
    var audio: [QrtzNano.AudioSource] = []
    var features = ["ax", "ax.observer"]
    if #available(macOS 14.2, *) { audio.append(CoreAudioTapSource(mic: micHub.view("recording", gated: true))); features.append("audio.process-tap") }
    audio.append(ScreenCaptureKitSource(mic: micHub.view("recording", gated: true))); features.append("audio.screencapturekit")
    features += ["audio.echo-gate", "audio.aec.localvqe"]

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

    var deps = NanoDeps(meetings: AXMeetingSource(context: context), audio: audio, transcribers: asr, context: context,
                        enhancer: LocalVqeEnhancer())
    deps.summarizer = FoundationModelsSummarizer()
    deps.systemTranscribers = [NemotronTranscriber()]
    deps.diarizers = [.mic: NemotronChannelDiarizer(), .system: NemotronChannelDiarizer()]
    deps.modelLoads = modelLoads
    deps.modelWarmup = { await warmModels(modelLoads) }
    return (deps, features)
}

/// `models.warm` (ADR 0079 / N121): each model loaded once and unloaded, one at a time (peak = one model), so Core ML
/// compiles and caches them now. Fresh instances: nothing a recording or talk holds is touched (echo's resident
/// recogniser included, N174: in split it is another process; in the monolith another instance).
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
