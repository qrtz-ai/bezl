#if os(macOS)
import AVFoundation
import Foundation
import QrtzShared
#if compiler(>=6.2) && canImport(Speech)
import Speech
#endif

// ADR 0146 M2.5: split out of qrtz-helper/mac/Transcribers.swift (owner: shared: Apple's on-device backup recogniser, both modules').

public func pcmBuffer16k(_ s: [Float]) -> AVAudioPCMBuffer? {
    guard let fmt = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
          let buf = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: AVAudioFrameCount(s.count))
    else { return nil }
    buf.frameLength = AVAudioFrameCount(s.count)
    s.withUnsafeBufferPointer { src in buf.floatChannelData![0].update(from: src.baseAddress!, count: s.count) }
    return buf
}

#if compiler(>=6.2) && canImport(Speech)
/// Backup: Apple SpeechAnalyzer + SpeechTranscriber (macOS 26), on-device, with volatile (partial) results, used
/// while Nemotron (recordings) or Parakeet (echo) isn't downloaded and only if the OS already has the assets.
@available(macOS 26.0, *)
public final class SpeechAnalyzerTranscriber: StreamingTranscriber, @unchecked Sendable {
    public let name = "apple-speechanalyzer"
    public init() {}
    private var analyzer: SpeechAnalyzer?
    private var transcriber: SpeechTranscriber?
    private var input: AsyncStream<AnalyzerInput>.Continuation?
    private var format: AVAudioFormat?
    private var converter: AVAudioConverter?
    private var finalized = ""
    private var volatile = ""
    private var resultsTask: Task<Void, Never>?
    private let lock = NSLock()

    public func prepare() async throws {
        let t = SpeechTranscriber(locale: Locale.current, transcriptionOptions: [], reportingOptions: [.volatileResults], attributeOptions: [])
        // ADR 0079: only when macOS already has this language's speech assets. qrtz never asks the OS to download
        // them (no download outside onboarding/settings); without them this backup is simply not used.
        guard await AssetInventory.status(forModules: [t]) == .installed else {
            throw NSError(domain: "qrtz.speechanalyzer", code: 1, userInfo: [NSLocalizedDescriptionKey: "speech assets for \(Locale.current.identifier) not installed"])
        }
        transcriber = t
        format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [t])
    }

    public func reset() async {
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

    public func append(_ s: [Float]) async {
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

    public func transcript() async -> String {
        lock.withLock { [finalized, volatile].filter { !$0.isEmpty }.joined(separator: " ") }
    }

    public func finish() async -> String {
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
