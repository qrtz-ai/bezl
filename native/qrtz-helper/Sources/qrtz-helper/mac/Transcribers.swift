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

/// ADR 0079: Nemotron Speech Streaming 0.6B (English, 560 ms chunks; FluidAudio StreamingNemotronAsrManager) on the
/// Neural Engine. Loads only from the verified local set (`LocalModels.asr`); never downloads. One instance per use:
/// echo's (ADR 0087: loaded when the helper starts, resident for its life, ~62 MB) and the recording's mic and system
/// channels (loaded per recording, unloaded at its end). Concurrent prepares share one load (`SharedLoad`).
final class NemotronTranscriber: StreamingTranscriber, @unchecked Sendable {
    let name = "nemotron-560ms"
    let loadsInstantly = false
    let latencyMs: Double = 560
    private let load = SharedLoad<StreamingNemotronAsrManager>()
    /// The last load of this instance: how long it took and whether Core ML's device specialization was expected
    /// to be cached (`CompiledModelStamp`).
    private(set) var lastLoad: (ms: Int, expectCached: Bool, why: String)?

    private var current: StreamingNemotronAsrManager? { load.loaded }

    func prepare() async throws {
        _ = try await load.get { [weak self] in
            let dir = try LocalModels.dir(LocalModels.asr)
            let stamp = CompiledModelStamp(set: LocalModels.asr, dir: dir)
            let expect = stamp.expectCached()
            let t0 = Date()
            let m = StreamingNemotronAsrManager(requestedChunkSize: .ms560)
            try await m.loadModels(from: dir)
            let ms = Int(Date().timeIntervalSince(t0) * 1000)
            stamp.record(loadMs: ms)
            self?.lastLoad = (ms, expect.cached, expect.why)
            logErr("nemotron: loaded in \(ms) ms (\(expect.cached ? "specialization cached" : "specializing: " + expect.why))")
            return m
        }
    }

    func append(_ s: [Float]) async {
        guard let m = current, let buf = pcmBuffer16k(s) else { return }
        do {
            try await m.appendAudio(buf)
            try await m.processBufferedAudio()
        } catch {
            logErr("nemotron: \(error)")
        }
    }

    func transcript() async -> String { await current?.getPartialTranscript() ?? "" }

    func finish() async -> String {
        guard let m = current else { return "" }
        do { return try await m.finish() } catch { logErr("nemotron finish: \(error)"); return await m.getPartialTranscript() }
    }

    func reset() async { await current?.reset() }

    func unload() async {
        let m = load.clear()
        await m?.cleanup()
    }
}

#if compiler(>=6.2) && canImport(Speech)
/// Backup: Apple SpeechAnalyzer + SpeechTranscriber (macOS 26), on-device, with volatile (partial) results, used
/// while Nemotron isn't downloaded and only if the OS already has the assets.
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
        // ADR 0079: only when macOS already has this language's speech assets. qrtz never asks the OS to download
        // them (no download outside onboarding/settings); without them this backup is simply not used.
        guard await AssetInventory.status(forModules: [t]) == .installed else {
            throw NSError(domain: "qrtz.speechanalyzer", code: 1, userInfo: [NSLocalizedDescriptionKey: "speech assets for \(Locale.current.identifier) not installed"])
        }
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
