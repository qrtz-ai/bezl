#if os(macOS)
import AVFoundation
import FluidAudio
import Foundation
import QrtzHelperCore
#if compiler(>=6.2) && canImport(Speech)
import Speech
#endif

func pcmBuffer16k(_ s: [Float]) -> AVAudioPCMBuffer? {
    guard let fmt = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
          let buf = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: AVAudioFrameCount(s.count))
    else { return nil }
    buf.frameLength = AVAudioFrameCount(s.count)
    s.withUnsafeBufferPointer { src in buf.floatChannelData![0].update(from: src.baseAddress!, count: s.count) }
    return buf
}

/// Parakeet Unified 0.6B streaming (FluidAudio StreamingUnifiedAsrManager) on the Neural Engine. Models are fetched
/// from Hugging Face into ~/Library/Application Support on first use (~600 MB, once). [MAC-VERIFY]
final class ParakeetTranscriber: StreamingTranscriber, @unchecked Sendable {
    let name = "parakeet-unified-streaming"
    private var manager: StreamingUnifiedAsrManager?
    private var processing = false

    func prepare() async throws {
        if manager != nil { return }
        let m = StreamingUnifiedAsrManager()
        if let dir = ProcessInfo.processInfo.environment["QRTZ_MODELS_DIR"], !dir.isEmpty {
            try await m.loadModels(to: URL(fileURLWithPath: dir))
        } else {
            try await m.loadModels()
        }
        manager = m
    }

    func append(_ s: [Float]) async {
        guard let m = manager, let buf = pcmBuffer16k(s) else { return }
        do {
            try await m.appendAudio(buf)
            // Decoding is cheaper than real time; skip a cycle rather than queueing if a previous pass is running.
            if !processing {
                processing = true
                try await m.processBufferedAudio()
                processing = false
            }
        } catch {
            processing = false
            logErr("parakeet: \(error)")
        }
    }

    func transcript() async -> String { await manager?.getPartialTranscript() ?? "" }

    func finish() async -> String {
        guard let m = manager else { return "" }
        do { return try await m.finish() } catch { logErr("parakeet finish: \(error)"); return await m.getPartialTranscript() }
    }

    func reset() async { try? await manager?.reset() }
}

#if compiler(>=6.2) && canImport(Speech)
/// Fallback: Apple SpeechAnalyzer + SpeechTranscriber (macOS 26), on-device, with volatile (partial) results.
@available(macOS 26.0, *)
final class SpeechAnalyzerTranscriber: StreamingTranscriber, @unchecked Sendable {
    let name = "apple-speechanalyzer"
    private var analyzer: SpeechAnalyzer?
    private var transcriber: SpeechTranscriber?
    private var input: AsyncStream<AnalyzerInput>.Continuation?
    private var format: AVAudioFormat?
    private var converter: AVAudioConverter?
    private var finalized = ""
    private var volatile = ""
    private var resultsTask: Task<Void, Never>?
    private let lock = NSLock()

    func prepare() async throws {
        let t = SpeechTranscriber(locale: Locale.current, transcriptionOptions: [], reportingOptions: [.volatileResults], attributeOptions: [])
        if let req = try await AssetInventory.assetInstallationRequest(supporting: [t]) { try await req.downloadAndInstall() }
        transcriber = t
        format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [t])
    }

    func reset() async {
        await stopAnalyzer()
        guard let t = transcriber else { return }
        lock.withLock { finalized = ""; volatile = "" }
        let (stream, cont) = AsyncStream<AnalyzerInput>.makeStream()
        input = cont
        let a = SpeechAnalyzer(modules: [t])
        analyzer = a
        resultsTask = Task { [weak self] in
            do {
                for try await r in t.results {
                    let text = String(r.text.characters)
                    self?.lock.withLock {
                        if r.isFinal { self?.finalized += (self?.finalized.isEmpty == false ? " " : "") + text; self?.volatile = "" }
                        else { self?.volatile = text }
                    }
                }
            } catch { logErr("speechanalyzer: \(error)") }
        }
        do { try await a.start(inputSequence: stream) } catch { logErr("speechanalyzer start: \(error)") }
    }

    func append(_ s: [Float]) async {
        guard let input, let src = pcmBuffer16k(s) else { return }
        var buf = src
        if let format, format != src.format {
            if converter == nil { converter = AVAudioConverter(from: src.format, to: format) }
            let cap = AVAudioFrameCount(Double(s.count) * format.sampleRate / 16_000) + 16
            guard let conv = converter, let out = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: cap) else { return }
            var fed = false
            var err: NSError?
            conv.convert(to: out, error: &err) { _, status in
                if fed { status.pointee = .noDataNow; return nil }
                fed = true; status.pointee = .haveData; return src
            }
            if err != nil { return }
            buf = out
        }
        input.yield(AnalyzerInput(buffer: buf))
    }

    func transcript() async -> String {
        lock.withLock { [finalized, volatile].filter { !$0.isEmpty }.joined(separator: " ") }
    }

    func finish() async -> String {
        input?.finish()
        try? await analyzer?.finalizeAndFinishThroughEndOfInput()
        _ = await resultsTask?.value
        return await transcript()
    }

    private func stopAnalyzer() async {
        input?.finish()
        input = nil
        await analyzer?.cancelAndFinishNow()
        resultsTask?.cancel()
        analyzer = nil
    }
}
#endif
#endif
