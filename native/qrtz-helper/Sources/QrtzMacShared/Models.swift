#if os(macOS)
import CryptoKit
import Foundation

/// ADR 0079: the helper's models are files the host downloaded and verified (pinned sha256s, onboarding or settings).
/// The helper never fetches a model: it reads `$QRTZ_MODELS_DIR/<set>/` only once the host has written the set's
/// `.qrtz-verified` marker, and throws otherwise (the caller falls back or reports "speech not downloaded").
public enum LocalModels {
    /// Recordings (nano) only since echo moved to Parakeet.
    public static let asr = "nemotron-560ms"
    /// Echo dictation: Parakeet TDT v2 English (`local_models.json` set `echo-asr`).
    public static let echoAsr = "parakeet-tdt-v2"
    public static let diarizer = "nemotron3-diarizer"
    public static let echo = "localvqe"
    public static let marker = ".qrtz-verified"

    public struct Missing: Error, CustomStringConvertible {
        public let set: String
        public init(set: String) { self.set = set }
        public var description: String { "speech models not downloaded (\(set))" }
    }

    public static func dir(_ set: String) throws -> URL {
        guard let root = ProcessInfo.processInfo.environment["QRTZ_MODELS_DIR"], !root.isEmpty else { throw Missing(set: set) }
        let d = URL(fileURLWithPath: root, isDirectory: true).appendingPathComponent(set, isDirectory: true)
        guard FileManager.default.fileExists(atPath: d.appendingPathComponent(marker).path) else { throw Missing(set: set) }
        return d
    }
}
/// ADR 0087: what we can and can't keep of a model's compile. The downloaded sets already are compiled Core ML
/// bundles (`.mlmodelc`); what makes a first load slow (~seconds) is Core ML's device specialization for the Neural
/// Engine, which Core ML caches itself, keyed by the bundle's absolute path and the load configuration, across
/// reboots, and drops on an OS update, under disk pressure, or when the bundle moves or changes. There is no public API
/// to export, persist or query that cache, so we keep what decides it: the models stay at one stable path
/// (`<data>/models/<set>`), are loaded with one configuration, and this stamp in `<data>/models/compiled/<set>.json`
/// records the key (model files hash + OS build + path) of the last load and its time. A key change means Core ML will
/// specialize again (logged as such); a slow load under an unchanged key means the OS evicted its cache (also logged).
public struct CompiledModelStamp {
    public let set: String
    public let dir: URL
    public init(set: String, dir: URL) { self.set = set; self.dir = dir }
    public var file: URL { dir.deletingLastPathComponent().appendingPathComponent("compiled", isDirectory: true).appendingPathComponent(set + ".json") }

    public struct Key: Codable, Equatable { var model: String; var osBuild: String; var path: String }
    public struct Saved: Codable { var key: Key; var loadMs: Int; var at: String }

    public static func osBuild() -> String {
        var n = 0
        guard sysctlbyname("kern.osversion", nil, &n, nil, 0) == 0, n > 0 else { return "?" }
        var b = [CChar](repeating: 0, count: n)
        guard sysctlbyname("kern.osversion", &b, &n, nil, 0) == 0 else { return "?" }
        return String(cString: b)
    }

    /// sha256 over the marker (manifest version) and every file's relative path, size and mtime: cheap (no reading
    /// of the weights) and changes whenever a file is replaced or touched, which is also what drops Core ML's cache.
    public func modelHash() -> String {
        var h = SHA256()
        if let m = try? Data(contentsOf: dir.appendingPathComponent(LocalModels.marker)) { h.update(data: m) }
        let fm = FileManager.default
        let keys: [URLResourceKey] = [.fileSizeKey, .contentModificationDateKey, .isRegularFileKey]
        var rows: [String] = []
        if let e = fm.enumerator(at: dir, includingPropertiesForKeys: keys) {
            for case let u as URL in e {
                guard let v = try? u.resourceValues(forKeys: Set(keys)), v.isRegularFile == true else { continue }
                let rel = String(u.path.dropFirst(dir.path.count))
                rows.append("\(rel)|\(v.fileSize ?? 0)|\(v.contentModificationDate?.timeIntervalSince1970 ?? 0)")
            }
        }
        for r in rows.sorted() { h.update(data: Data(r.utf8)) }
        return h.finalize().map { String(format: "%02x", $0) }.joined()
    }

    public func key() -> Key { Key(model: modelHash(), osBuild: Self.osBuild(), path: dir.standardizedFileURL.path) }

    public func saved() -> Saved? { (try? Data(contentsOf: file)).flatMap { try? JSONDecoder().decode(Saved.self, from: $0) } }

    /// Whether this load should find Core ML's specialization cached, and if not, why.
    public func expectCached() -> (cached: Bool, why: String) {
        guard let s = saved() else { return (false, "first load on this Mac") }
        let k = key()
        if s.key.osBuild != k.osBuild { return (false, "os updated (\(s.key.osBuild) -> \(k.osBuild))") }
        if s.key.path != k.path { return (false, "models moved") }
        if s.key.model != k.model { return (false, "models changed") }
        return (true, "")
    }

    public func record(loadMs: Int) {
        let fm = FileManager.default
        let d = file.deletingLastPathComponent()
        try? fm.createDirectory(at: d, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let s = Saved(key: key(), loadMs: loadMs, at: ISO8601DateFormatter().string(from: Date()))
        if let data = try? JSONEncoder().encode(s) { try? data.write(to: file, options: .atomic) }
    }
}
#endif
