#if os(macOS)
import CoreML
import FluidAudio
import Foundation
import QrtzHelperCore

/// `qrtz-helper --echo-bench <clips dir> [--out results.json] [--engines a,b] [--variants a,b|all] [--speed 1]
///  [--speaker speaker.aiff] [--bench-models <dir>]` (ADR 0099, docs/echo/dictation-tuning.md): every clip
/// (`<name>.wav|aiff|m4a…` + `<name>.txt`) through the real echo path (`DictationEngine`: pre-roll, tail, drain,
/// pad, finalize, echo gate) per engine × variant, at real time by default. Writes one JSON report to `--out`
/// (default `~/qrtz-echo-bench/results/echo-bench-<time>.json`) and prints its path. Local files only: the shipped
/// model from `$QRTZ_MODELS_DIR` (like the app), the bench-only sets from `--bench-models` (fetched and checked by
/// scripts/echo-bench.sh). Nothing here is used by the app's runtime.
///
/// Engines: `nemotron-560` (shipped: the app's own NemotronTranscriber), `nemotron-1120`, `nemotron-2240` (chunk
/// tiers), `nemotron-560+unified` (two-pass: Nemotron partials, Parakeet Unified batch final on release), `apple`
/// (SpeechAnalyzer, macOS 26, installed assets only). No GPU variant: the int8 encoder on the GPU is ~10× slower
/// and can abort in MPSGraph (FluidAudio's own notes), so the Neural Engine stays.
enum EchoBenchCLI {
    static func run(_ args: [String]) -> Never {
        Task {
            do {
                let path = try await measure(args)
                print(path)
                exit(0)
            } catch {
                logErr("echo-bench: \(error)")
                exit(1)
            }
        }
        dispatchMain()
    }

    static func value(_ args: [String], _ flag: String) -> String? {
        guard let i = args.firstIndex(of: flag), i + 1 < args.count else { return nil }
        return args[i + 1]
    }

    static func list(_ args: [String], _ flag: String) -> [String]? {
        value(args, flag).map { $0.split(separator: ",").map { String($0).trimmingCharacters(in: .whitespaces) } }
    }

    static func sysctl(_ name: String) -> String {
        var n = 0
        guard sysctlbyname(name, nil, &n, nil, 0) == 0, n > 0 else { return "?" }
        var b = [CChar](repeating: 0, count: n)
        guard sysctlbyname(name, &b, &n, nil, 0) == 0 else { return "?" }
        return String(cString: b)
    }

    static func benchModels(_ args: [String]) -> URL? {
        let p = value(args, "--bench-models") ?? ProcessInfo.processInfo.environment["QRTZ_BENCH_MODELS_DIR"]
        return p.map { URL(fileURLWithPath: ($0 as NSString).expandingTildeInPath, isDirectory: true) }
    }

    /// Every engine this Mac can run now, by name.
    static func engines(_ names: [String], models: URL?) -> [StreamingTranscriber] {
        var out: [StreamingTranscriber] = []
        func set(_ d: String) -> URL? {
            guard let m = models?.appendingPathComponent(d, isDirectory: true),
                  FileManager.default.fileExists(atPath: m.appendingPathComponent(".qrtz-bench-verified").path) else { return nil }
            return m
        }
        for n in names {
            switch n {
            case "nemotron-560": out.append(NemotronTranscriber())
            case "nemotron-1120", "nemotron-2240":
                let tier = n.dropFirst("nemotron-".count)
                guard let d = set("nemotron-\(tier)ms") else { logErr("echo-bench: \(n): bench models not downloaded (echo-bench.sh --extra)"); continue }
                out.append(BenchNemotron(name: n, dir: d, latencyMs: Double(tier) ?? 1120, units: .cpuAndNeuralEngine))
            case "nemotron-560+unified":
                guard let d = set("parakeet-unified") else { logErr("echo-bench: \(n): bench models not downloaded (echo-bench.sh --extra)"); continue }
                out.append(BenchTwoPass(name: n, streaming: NemotronTranscriber(), batchDir: d))
            case "apple":
                #if compiler(>=6.2) && canImport(Speech)
                if #available(macOS 26.0, *) { out.append(SpeechAnalyzerTranscriber()) } else { logErr("echo-bench: apple needs macOS 26") }
                #else
                logErr("echo-bench: apple needs the macOS 26 SDK")
                #endif
            default: logErr("echo-bench: unknown engine \(n)")
            }
        }
        return out
    }

    static func measure(_ args: [String]) async throws -> String {
        guard let dirArg = value(args, "--echo-bench") else { throw Bench.Failure(description: "usage: --echo-bench <clips dir> [--out f.json]") }
        let dir = URL(fileURLWithPath: (dirArg as NSString).expandingTildeInPath, isDirectory: true)
        let pairs = EchoClips.pairs(in: dir)
        guard !pairs.isEmpty else { throw Bench.Failure(description: "no clips in \(dir.path) (<name>.wav|aiff|m4a + <name>.txt)") }
        var clips: [EchoClip] = []
        for p in pairs { clips.append(EchoClip(name: p.name, samples: try Bench.load16k(p.audio.path), reference: p.reference)) }

        var options = EchoBenchOptions()
        options.speed = Double(value(args, "--speed") ?? "1") ?? 1
        if let sp = value(args, "--speaker") ?? defaultSpeaker(dir) { options.speaker = try Bench.load16k(sp) }

        var variants = EchoVariant.standard()
        if let want = list(args, "--variants"), want != ["all"] { variants = variants.filter { want.contains($0.name) } }
        if options.speaker.isEmpty { variants = variants.filter { $0.echoCoupling == nil } }
        let models = benchModels(args)
        let engines = engines(list(args, "--engines") ?? ["nemotron-560"], models: models)
        guard !engines.isEmpty, !variants.isEmpty else { throw Bench.Failure(description: "no engine or variant to run") }

        let started = Date()
        logErr("echo-bench: \(clips.count) clips × \(engines.count) engines × \(variants.count) variants at \(options.speed)× (audio \(clips.map(\.audioMs).reduce(0, +) / 1000) s per pass)")
        var report = await EchoBench.run(clips: clips, engines: engines, variants: variants, options: options, progress: { logErr("echo-bench: " + $0) })
        let v = ProcessInfo.processInfo.operatingSystemVersion
        report["machine"] = [
            "model": sysctl("hw.model"), "chip": sysctl("machdep.cpu.brand_string"), "memGB": ProcessInfo.processInfo.physicalMemory / 1_073_741_824,
            "os": "macOS \(v.majorVersion).\(v.minorVersion).\(v.patchVersion) (\(CompiledModelStamp.osBuild()))",
            "lowPowerMode": ProcessInfo.processInfo.isLowPowerModeEnabled,
        ] as [String: Any]
        report["bench"] = [
            "version": 1, "clipsDir": dir.path, "speed": options.speed, "startedAt": ISO8601DateFormatter().string(from: started),
            "seconds": Int(Date().timeIntervalSince(started)), "engines": engines.map(\.name), "variants": variants.map(\.name),
            "footprintMB": Bench.footprintMB(), "shippedTail": EchoVariant(name: "x").tail.json,
        ] as [String: Any]

        let out: URL
        if let o = value(args, "--out") { out = URL(fileURLWithPath: (o as NSString).expandingTildeInPath) } else {
            let f = DateFormatter(); f.dateFormat = "yyyyMMdd-HHmmss"
            out = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("qrtz-echo-bench/results/echo-bench-\(f.string(from: started)).json")
        }
        try FileManager.default.createDirectory(at: out.deletingLastPathComponent(), withIntermediateDirectories: true)
        let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: out, options: .atomic)
        return out.path
    }

    /// `<clips>/speaker/*.{aiff,wav,m4a}` (the first): the speaker audio for the echo variants.
    static func defaultSpeaker(_ dir: URL) -> String? {
        let d = dir.appendingPathComponent("speaker", isDirectory: true)
        let items = (try? FileManager.default.contentsOfDirectory(at: d, includingPropertiesForKeys: nil)) ?? []
        return items.filter { EchoClips.audioExtensions.contains($0.pathExtension.lowercased()) }.sorted { $0.path < $1.path }.first?.path
    }
}

extension DictationTail {
    var json: [String: Any] {
        ["tailMs": tailMs, "drainTimeoutMs": drainTimeoutMs, "padMs": padMs, "finalizeTimeoutMs": finalizeTimeoutMs,
         "maxWaitMs": maxWaitMs, "nextBlockWaitMs": nextBlockWaitMs]
    }
}

/// Bench-only Nemotron: any tier directory (the app's NemotronTranscriber is fixed to the shipped 560 ms set).
final class BenchNemotron: StreamingTranscriber, @unchecked Sendable {
    let name: String
    let loadsInstantly = false
    let latencyMs: Double
    let dir: URL
    let units: MLComputeUnits
    private var m: StreamingNemotronAsrManager?

    init(name: String, dir: URL, latencyMs: Double, units: MLComputeUnits) {
        self.name = name; self.dir = dir; self.latencyMs = latencyMs; self.units = units
    }

    func prepare() async throws {
        if m != nil { return }
        let mgr = StreamingNemotronAsrManager(configuration: MLModelConfigurationUtils.defaultConfiguration(computeUnits: units))
        try await mgr.loadModels(from: dir)
        m = mgr
    }

    func append(_ s: [Float]) async {
        guard let m, let buf = pcmBuffer16k(s) else { return }
        do {
            try await m.appendAudio(buf)
            try await m.processBufferedAudio()
        } catch {
            logErr("\(name): \(error)")
        }
    }

    func transcript() async -> String { await m?.getPartialTranscript() ?? "" }

    func finish() async -> String {
        guard let m else { return "" }
        do { return try await m.finish() } catch { logErr("\(name) finish: \(error)"); return await m.getPartialTranscript() }
    }

    func reset() async { await m?.reset() }

    func unload() async {
        let x = m
        m = nil
        await x?.cleanup()
    }
}

/// Two-pass: the streaming recogniser gives the live partials; on release the whole dictation (≤ a few seconds is
/// typical) is decoded again by Parakeet Unified's batch (full-attention) encoder, and that is the final text.
final class BenchTwoPass: StreamingTranscriber, @unchecked Sendable {
    let name: String
    let loadsInstantly = false
    var latencyMs: Double { streaming.latencyMs }
    let streaming: StreamingTranscriber
    let batchDir: URL
    private var batch: UnifiedAsrManager?
    private let lock = NSLock()
    private var samples: [Float] = []

    init(name: String, streaming: StreamingTranscriber, batchDir: URL) {
        self.name = name; self.streaming = streaming; self.batchDir = batchDir
    }

    func prepare() async throws {
        try await streaming.prepare()
        if batch == nil {
            let b = UnifiedAsrManager()
            try await b.loadModels(from: batchDir)
            batch = b
        }
    }

    func append(_ s: [Float]) async {
        lock.withLock { samples += s }
        await streaming.append(s)
    }

    func transcript() async -> String { await streaming.transcript() }

    func finish() async -> String {
        let streamed = await streaming.finish()
        let all = lock.withLock { samples }
        guard let b = batch, all.count >= 1_600 else { return streamed }
        do { return try await b.transcribe(all) } catch { logErr("\(name) batch: \(error)"); return streamed }
    }

    func reset() async {
        lock.withLock { samples.removeAll() }
        await streaming.reset()
        try? await batch?.reset()
    }

    func unload() async {
        await streaming.unload()
        let b = batch
        batch = nil
        await b?.cleanup()
    }
}
#endif
