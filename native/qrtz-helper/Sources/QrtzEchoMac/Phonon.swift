#if os(macOS) && canImport(PhononCoreML)
import Foundation
import PhononCoreML
import QrtzEcho
import QrtzMacShared
import QrtzShared

/// Experimental (Jared, 2026-10-26): Phonon-2.5 English (community Core ML build Kayetic/Phonon-2.5-English-CoreML,
/// CC-BY-4.0, on Fermion's phonon-coreml 1.1.2 unchanged). Only `RoutedRecognizer` drives it, for 3...35 s clips,
/// once loaded; Parakeet stays echo's recogniser. Loads only from the verified local set (`LocalModels.echoAsrPhonon`),
/// never downloads. The first load compiles six encoder functions (minutes, once per Mac); later loads ~2 s.
public final class PhononBatch: BatchRecognizer, @unchecked Sendable {
    public let name = "phonon-2.5-en"
    public init() {}
    private let lock = NSLock()
    private var transcriber: Transcriber?

    public func prepare() async throws {
        if lock.withLock({ transcriber }) != nil { return }
        let dir = try LocalModels.dir(LocalModels.echoAsrPhonon)
        let t0 = Date()
        let t: Transcriber = try await Task.detached(priority: .utility) {
            var o = Transcriber.Options()
            o.singleShotMaxSeconds = EngineRoute.maxSeconds
            let t = try Transcriber(bundle: dir, options: o)
            try t.preloadAllFunctions()
            return t
        }.value
        logErr("phonon: loaded in \(Int(Date().timeIntervalSince(t0) * 1000)) ms (compile \(Int(t.compileSeconds * 1000)) ms)")
        lock.withLock { transcriber = t }
    }

    public func decode(_ samples: [Float]) async throws -> BatchResult {
        guard let t = lock.withLock({ transcriber }) else { throw LocalModels.Missing(set: LocalModels.echoAsrPhonon) }
        let r = try await Task.detached(priority: .userInitiated) { try t.transcribe(samples) }.value
        let words = r.words.map { BatchResult.Word($0.text, start: $0.start) }
        return BatchResult(text: r.text, words: words.isEmpty ? nil : words)
    }

    public func unload() async { lock.withLock { transcriber = nil } }
}
#endif
