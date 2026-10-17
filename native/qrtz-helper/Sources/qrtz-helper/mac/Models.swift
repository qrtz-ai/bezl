#if os(macOS)
import Foundation

/// ADR 0079: the helper's models are files the host downloaded and verified (pinned sha256s, onboarding or settings).
/// The helper never fetches a model: it reads `$QRTZ_MODELS_DIR/<set>/` only once the host has written the set's
/// `.qrtz-verified` marker, and throws otherwise (the caller falls back or reports "speech not downloaded").
enum LocalModels {
    static let asr = "nemotron-560ms"
    static let diarizer = "nemotron3-diarizer"
    static let echo = "localvqe"
    static let marker = ".qrtz-verified"

    struct Missing: Error, CustomStringConvertible {
        let set: String
        var description: String { "speech models not downloaded (\(set))" }
    }

    static func dir(_ set: String) throws -> URL {
        guard let root = ProcessInfo.processInfo.environment["QRTZ_MODELS_DIR"], !root.isEmpty else { throw Missing(set: set) }
        let d = URL(fileURLWithPath: root, isDirectory: true).appendingPathComponent(set, isDirectory: true)
        guard FileManager.default.fileExists(atPath: d.appendingPathComponent(marker).path) else { throw Missing(set: set) }
        return d
    }
}
#endif
