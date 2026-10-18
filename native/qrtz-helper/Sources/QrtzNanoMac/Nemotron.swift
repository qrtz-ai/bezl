#if os(macOS)
import AVFoundation
import FluidAudio
import Foundation
import QrtzMacShared
import QrtzShared

// ADR 0146 M2.5: split out of qrtz-helper/mac/Transcribers.swift (owner: nano: the recording's recogniser).

/// ADR 0079: Nemotron Speech Streaming 0.6B (English, 560 ms chunks; FluidAudio StreamingNemotronAsrManager) on the
/// Neural Engine. Loads only from the verified local set (`LocalModels.asr`); never downloads. One instance per use:
/// echo's (ADR 0087: loaded when the helper starts, resident for its life, ~62 MB) and the recording's mic and system
/// channels (loaded per recording, unloaded at its end). Concurrent prepares share one load (`SharedLoad`).
public final class NemotronTranscriber: StreamingTranscriber, @unchecked Sendable {
    public let name = "nemotron-560ms"
    public init() {}
    public let loadsInstantly = false
    public let latencyMs: Double = 560
    private let load = SharedLoad<StreamingNemotronAsrManager>()
    /// The last load of this instance: how long it took and whether Core ML's device specialization was expected
    /// to be cached (`CompiledModelStamp`).
    public private(set) var lastLoad: (ms: Int, expectCached: Bool, why: String)?

    private var current: StreamingNemotronAsrManager? { load.loaded }

    public func prepare() async throws {
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

    public func append(_ s: [Float]) async {
        guard let m = current, let buf = pcmBuffer16k(s) else { return }
        do {
            try await m.appendAudio(buf)
            try await m.processBufferedAudio()
        } catch {
            logErr("nemotron: \(error)")
        }
    }

    public func transcript() async -> String { await current?.getPartialTranscript() ?? "" }

    public func finish() async -> String {
        guard let m = current else { return "" }
        do { return try await m.finish() } catch { logErr("nemotron finish: \(error)"); return await m.getPartialTranscript() }
    }

    public func reset() async { await current?.reset() }

    public func unload() async {
        let m = load.clear()
        await m?.cleanup()
    }
}
#endif
