#if os(macOS)
import AVFoundation
import FluidAudio
import Foundation
import QrtzEcho
import QrtzMacShared
import QrtzShared

// ADR 0146 M2.5: split out of qrtz-helper/mac/Transcribers.swift (owner: echo: the dictation recogniser).

/// Echo's recogniser (Jared, 2026-10-18): Parakeet TDT v2 English (FluidAudio `AsrManager`, Core ML on the Neural
/// Engine), a batch decoder driven in segments by `SegmentedTranscriber` (pauses / 15 s cap; no partials). Loads only
/// from the verified local set (`LocalModels.echoAsr`), never downloads; resident for the helper's life (ADR 0087).
/// Decodes are serialised by the segmenter (one at a time), and the decoder state is fresh per segment.
public final class ParakeetBatch: BatchRecognizer, @unchecked Sendable {
    public let name = "parakeet-tdt-v2"
    public init() {}
    private let load = SharedLoad<AsrManager>()
    public private(set) var lastLoad: (ms: Int, expectCached: Bool, why: String)?

    public func prepare() async throws {
        _ = try await load.get { [weak self] in
            let dir = try LocalModels.dir(LocalModels.echoAsr)
            let stamp = CompiledModelStamp(set: LocalModels.echoAsr, dir: dir)
            let expect = stamp.expectCached()
            let t0 = Date()
            let models = try AsrModels.loadLocal(from: dir, version: .v2)
            let m = AsrManager(config: .default, models: models)
            let ms = Int(Date().timeIntervalSince(t0) * 1000)
            stamp.record(loadMs: ms)
            self?.lastLoad = (ms, expect.cached, expect.why)
            logErr("parakeet: loaded in \(ms) ms (\(expect.cached ? "specialization cached" : "specializing: " + expect.why))")
            // QRT-270: one decode of silence before `prepare` returns (so before echo reports ready): Core ML's first
            // predict on the Neural Engine is slow, and it was landing on the user's first dictation. A failure here
            // is only logged; the model is loaded and a real decode still works.
            let tw = Date()
            var warm = TdtDecoderState.make(decoderLayers: await m.decoderLayerCount)
            do {
                _ = try await m.transcribe([Float](repeating: 0, count: ASRConstants.minimumRequiredSamples(forSampleRate: 16_000)),
                                           decoderState: &warm)
                logErr("parakeet: warm-up decode \(Int(Date().timeIntervalSince(tw) * 1000)) ms")
            } catch {
                logErr("parakeet: warm-up decode failed: \(error)")
            }
            return m
        }
    }

    public func decode(_ samples: [Float]) async throws -> BatchResult {
        guard let m = load.loaded else { throw LocalModels.Missing(set: LocalModels.echoAsr) }
        // The model needs 0.3 s at least: a shorter stretch gets silence after it (end context anyway).
        let minimum = ASRConstants.minimumRequiredSamples(forSampleRate: 16_000)
        let audio = samples.count >= minimum ? samples : samples + [Float](repeating: 0, count: minimum - samples.count)
        var state = TdtDecoderState.make(decoderLayers: await m.decoderLayerCount)
        let r = try await m.transcribe(audio, decoderState: &state)
        // Word starts from the token timings (for the stitch). The words themselves are the final text's when they
        // line up one to one (its spacing and punctuation as the manager formatted it), else the tokens' own.
        let timed = buildWordTimings(from: r.tokenTimings ?? [])
        // ADR 0131: the mean TDT token confidence, for the release tail's floor (tail_conf= on the timing line).
        // ADR 0132 (N170): nil for FluidAudio's placeholders (blank text: 0.1; no token timings: 0.5).
        let conf = BatchResult.reportedConfidence(Float(r.confidence), text: r.text, timedTokens: r.tokenTimings?.count ?? 0)
        guard !timed.isEmpty else { return BatchResult(text: r.text, confidence: conf) }
        let textWords = r.text.split(whereSeparator: { $0.isWhitespace }).map(String.init)
        let words = textWords.count == timed.count
            ? zip(textWords, timed).map { BatchResult.Word($0.0, start: $0.1.startTime) }
            : timed.map { BatchResult.Word($0.word, start: $0.startTime) }
        return BatchResult(text: r.text, words: words, confidence: conf)
    }

    public func unload() async {
        let m = load.clear()
        await m?.cleanup()
    }
}
#endif
