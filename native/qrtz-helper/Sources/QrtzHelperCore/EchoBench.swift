import Foundation

// Echo tuning bench (docs/echo/dictation-tuning.md, ADR 0099). Runs fixed clips through the real DictationEngine
// (pre-roll, tail, drain, pad, finalize, echo gate) with a clip-playing mic, once per (engine, variant), and reports
// per-stage timings and WER. Foundation-only so the plumbing is tested on Linux with fake recognisers; the macOS
// executable (`qrtz-helper --echo-bench`, mac/EchoBench.swift) plugs in the real Core ML engines and file decoding.
// Never touches the network; the report is a local file.

// MARK: - WER

public enum EchoWER {
    static let units = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
                        "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen"]
    static let tens = ["twenty": 20, "thirty": 30, "forty": 40, "fifty": 50, "sixty": 60, "seventy": 70, "eighty": 80, "ninety": 90]

    /// Words for the content WER: lowercased, punctuation dropped (apostrophes kept inside words, hyphens split),
    /// number words 0–99 written as digits, so "twenty five" and "25" match (formatting is scored separately).
    public static func normalize(_ s: String) -> [String] {
        var cleaned = ""
        for ch in asciiQuotes(s).lowercased() {
            if ch.isLetter || ch.isNumber || ch == "'" { cleaned.append(ch) }
            else if ch == "%" { cleaned += " percent " }
            else { cleaned.append(" ") }
        }
        let raw = cleaned.split(separator: " ").map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "'")) }.filter { !$0.isEmpty }
        var out: [String] = []
        var i = 0
        while i < raw.count {
            let w = raw[i]
            if let t = tens[w] {
                if i + 1 < raw.count, let u = units.firstIndex(of: raw[i + 1]), u > 0, u < 10 { out.append(String(t + u)); i += 2; continue }
                out.append(String(t)); i += 1; continue
            }
            if let u = units.firstIndex(of: w) { out.append(String(u)); i += 1; continue }
            out.append(w); i += 1
        }
        return out
    }

    /// Words for the formatted WER: whitespace tokens as written (case and punctuation count).
    public static func formatted(_ s: String) -> [String] { asciiQuotes(s).split(whereSeparator: { $0.isWhitespace }).map(String.init) }

    /// ADR 0120 gate: sentence ends (a word ending in `.?!`) of `hyp` against `ref` (on the bench: a segmented
    /// engine against `parakeet-whole` on the same clip), over the words the two share in order (LCS of their
    /// lowercased letters/digits/apostrophes). `spurious`: an end `hyp` has and `ref` doesn't (a cut's false
    /// sentence end); `lost`: the reverse. `spuriousWholeContinues` (ADR 0121, the strict gate count): the spurious
    /// ends where the whole clip's next word starts lowercase, so its sentence clearly goes on (the rest may be the
    /// whole-clip decode dropping a sentence end on a long clip).
    public struct SentenceEnds: Sendable, Equatable {
        public var agreeing = 0, spurious = 0, lost = 0, spuriousWholeContinues = 0
        public init(agreeing: Int = 0, spurious: Int = 0, lost: Int = 0, spuriousWholeContinues: Int = 0) {
            self.agreeing = agreeing; self.spurious = spurious; self.lost = lost; self.spuriousWholeContinues = spuriousWholeContinues
        }
        public mutating func add(_ o: SentenceEnds) {
            agreeing += o.agreeing; spurious += o.spurious; lost += o.lost; spuriousWholeContinues += o.spuriousWholeContinues
        }
        public var json: [String: Any] {
            ["agreeing": agreeing, "spurious": spurious, "lost": lost, "spuriousWholeContinues": spuriousWholeContinues]
        }
    }

    public static func sentenceEnds(hyp: String, ref: String) -> SentenceEnds {
        /// Words (with a key), whether each ends a sentence, and whether the next word starts lowercase.
        func toks(_ s: String) -> [(key: String, end: Bool, nextLower: Bool)] {
            let ws: [(key: String, end: Bool, lower: Bool)] = formatted(s).compactMap { w in
                let k = w.lowercased().filter { $0.isLetter || $0.isNumber || $0 == "'" }
                let t = w.reversed().drop(while: { "\"')]".contains($0) })
                return k.isEmpty ? nil : (k, t.first.map { ".?!".contains($0) } ?? false, w.first(where: \.isLetter)?.isLowercase ?? false)
            }
            return ws.indices.map { (ws[$0].key, ws[$0].end, $0 + 1 < ws.count && ws[$0 + 1].lower) }
        }
        let a = toks(hyp), b = toks(ref)
        guard !a.isEmpty, !b.isEmpty else { return SentenceEnds() }
        // LCS table (dictations are a few hundred words).
        var d = [[Int]](repeating: [Int](repeating: 0, count: b.count + 1), count: a.count + 1)
        for i in stride(from: a.count - 1, through: 0, by: -1) {
            for j in stride(from: b.count - 1, through: 0, by: -1) {
                d[i][j] = a[i].key == b[j].key ? d[i + 1][j + 1] + 1 : max(d[i + 1][j], d[i][j + 1])
            }
        }
        var r = SentenceEnds(), i = 0, j = 0
        while i < a.count, j < b.count {
            if a[i].key == b[j].key {
                switch (a[i].end, b[j].end) {
                case (true, true): r.agreeing += 1
                case (true, false): r.spurious += 1; if b[j].nextLower { r.spuriousWholeContinues += 1 }
                case (false, true): r.lost += 1
                default: break
                }
                i += 1; j += 1
            } else if d[i + 1][j] >= d[i][j + 1] { i += 1 } else { j += 1 }
        }
        return r
    }

    /// Edit distance between word lists: substitutions, deletions, insertions.
    public static func edits(_ ref: [String], _ hyp: [String]) -> (sub: Int, del: Int, ins: Int) {
        let n = ref.count, m = hyp.count
        if n == 0 { return (0, 0, m) }
        if m == 0 { return (0, n, 0) }
        // dp over (cost, sub, del, ins), row by row.
        typealias C = (cost: Int, s: Int, d: Int, i: Int)
        var prev: [C] = (0...m).map { (cost: $0, s: 0, d: 0, i: $0) }
        for a in 1...n {
            var cur: [C] = [(cost: a, s: 0, d: a, i: 0)]
            for b in 1...m {
                let diag = prev[b - 1], up = prev[b], left = cur[b - 1]
                let same = ref[a - 1] == hyp[b - 1]
                var best: C = same ? diag : (diag.cost + 1, diag.s + 1, diag.d, diag.i)
                if up.cost + 1 < best.cost { best = (up.cost + 1, up.s, up.d + 1, up.i) }
                if left.cost + 1 < best.cost { best = (left.cost + 1, left.s, left.d, left.i + 1) }
                cur.append(best)
            }
            prev = cur
        }
        let r = prev[m]
        return (r.s, r.d, r.i)
    }

    public struct Score: Sendable, Equatable {
        public var refWords = 0, sub = 0, del = 0, ins = 0
        public var errors: Int { sub + del + ins }
        public var wer: Double { refWords > 0 ? Double(errors) / Double(refWords) : (errors > 0 ? 1 : 0) }
        public mutating func add(_ o: Score) { refWords += o.refWords; sub += o.sub; del += o.del; ins += o.ins }
        public init() {}
        public var json: [String: Any] { ["refWords": refWords, "sub": sub, "del": del, "ins": ins, "wer": wer] }
    }

    public static func score(ref: [String], hyp: [String]) -> Score {
        let e = edits(ref, hyp)
        var s = Score(); s.refWords = ref.count; s.sub = e.sub; s.del = e.del; s.ins = e.ins
        return s
    }

    /// Words of the reference missing at the very end of the hypothesis (the cut-off tail), after alignment of the
    /// last 3 reference words: 0 when the hypothesis ends with them.
    public static func tailLoss(ref: [String], hyp: [String]) -> Int {
        let k = min(3, ref.count)
        guard k > 0 else { return 0 }
        let refTail = Array(ref.suffix(k))
        var lost = k
        for j in 0...k {
            let want = Array(refTail.prefix(k - j))
            // N163: as words, so "a" is not found inside "cat".
            if want.isEmpty || containsRun(Array(hyp.suffix(want.count + 2)), want) { lost = j; break }
        }
        return lost
    }

    /// `run` appears in `words` as consecutive whole words.
    static func containsRun(_ words: [String], _ run: [String]) -> Bool {
        guard !run.isEmpty else { return true }
        guard words.count >= run.count else { return false }
        return (0...(words.count - run.count)).contains { Array(words[$0..<($0 + run.count)]) == run }
    }

    /// N162: typographic apostrophes and quotes as ASCII (a reference typed in TextEdit or Notes), like
    /// qrtz-talk's `ascii_quotes`.
    public static func asciiQuotes(_ s: String) -> String {
        String(s.map { c -> Character in
            switch c {
            case "\u{2018}", "\u{2019}", "\u{02BC}": "'"
            case "\u{201C}", "\u{201D}": "\""
            default: c
            }
        })
    }

    /// Same for the first words (the clipped head of a cold mic).
    public static func headLoss(ref: [String], hyp: [String]) -> Int {
        tailLoss(ref: ref.reversed(), hyp: hyp.reversed())
    }
}

// MARK: - Clips

public struct EchoClip: Sendable {
    public let name: String
    /// 16 kHz mono.
    public let samples: [Float]
    /// What was said, written the way it should be typed (case and punctuation count for the formatted WER).
    public let reference: String
    public init(name: String, samples: [Float], reference: String) { self.name = name; self.samples = samples; self.reference = reference }
    public var audioMs: Int { samples.count / 16 }
}

public enum EchoClips {
    public static let audioExtensions = ["wav", "aiff", "aif", "caf", "m4a", "mp3", "flac"]

    /// `<dir>/<name>.<audio ext>` with `<dir>/<name>.txt` next to it, sorted by name. Audio without a reference is
    /// skipped. `--speaker` clips (system audio for the echo variants) live in `<dir>/speaker/`.
    public static func pairs(in dir: URL) -> [(name: String, audio: URL, reference: String)] {
        let fm = FileManager.default
        guard let items = try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil) else { return [] }
        var out: [(String, URL, String)] = []
        for u in items where audioExtensions.contains(u.pathExtension.lowercased()) {
            let base = u.deletingPathExtension()
            guard let ref = try? String(contentsOf: base.appendingPathExtension("txt"), encoding: .utf8) else { continue }
            let r = ref.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !r.isEmpty else { continue }
            out.append((base.lastPathComponent, u, r))
        }
        return out.sorted { $0.0 < $1.0 }
    }

    /// Ends the clip on its last loud sample (|x| > 0.01): released exactly as the last word ends, the hardest case
    /// for the tail.
    public static func trimTrailingSilence(_ s: [Float]) -> [Float] {
        guard let last = s.lastIndex(where: { abs($0) > 0.01 }) else { return s }
        return Array(s[...last])
    }

    public static func trimLeadingSilence(_ s: [Float]) -> [Float] {
        guard let first = s.firstIndex(where: { abs($0) > 0.01 }) else { return s }
        return Array(s[first...])
    }
}

// MARK: - Variants

/// One configuration of the dictation path, applied to every clip.
public struct EchoVariant: Sendable {
    public var name: String
    public var tail = DictationTail()
    /// Gain applied to the clip (the mic level: a quiet or far mic).
    public var gain: Float = 1
    /// Peak-normalize the clip to this level (dBFS) before the gain; nil = as recorded.
    public var normalizePeakDb: Float? = nil
    /// White noise at this SNR (dB, against the clip's RMS); nil = none. Deterministic (seeded).
    public var noiseSnrDb: Float? = nil
    /// Drop this much of the clip's start: the mic opening after the press (warm mic off) loses it.
    public var dropHeadMs = 0
    /// Pre-roll available at the press (warm mic on): this much silence before the clip is in the ring buffer.
    public var preRollMs = 0
    /// Echo: mix the speaker clip into the mic at this coupling (mic += coupling × speaker), with the speaker clip
    /// as the gate's reference. nil = no speaker audio.
    public var echoCoupling: Float? = nil
    /// The echo gate on (with `echoCoupling`): gated against the reference. Off: the raw mic.
    public var echoGate = true
    public init(name: String) { self.name = name }

    /// Applies gain, normalization, noise and the head drop.
    public func shape(_ clip: [Float], seed: UInt64 = 0x9E37_79B9_7F4A_7C15) -> [Float] {
        var s = clip
        if let db = normalizePeakDb, let peak = s.map({ abs($0) }).max(), peak > 0 {
            let k = powf(10, db / 20) / peak
            for i in s.indices { s[i] *= k }
        }
        if gain != 1 { for i in s.indices { s[i] *= gain } }
        if let snr = noiseSnrDb {
            let rms = Float(DSP.rms(s))
            let nrms = rms / powf(10, snr / 20)
            var x = seed
            for i in s.indices {
                // xorshift64*, uniform in [-1, 1) → scaled to the noise RMS (uniform RMS = 1/√3).
                x ^= x >> 12; x ^= x << 25; x ^= x >> 27
                let u = Float(Double((x &* 2_685_821_657_736_338_717) >> 11) / Double(1 << 53)) * 2 - 1
                s[i] += u * nrms * 1.7320508
            }
        }
        if dropHeadMs > 0 { s = Array(s.dropFirst(min(s.count, 16 * dropHeadMs))) }
        for i in s.indices { s[i] = max(-1, min(1, s[i])) }
        return s
    }

    /// The default sweep. `shipped` is what qrtz runs today; every other variant changes one thing.
    public static func standard() -> [EchoVariant] {
        var v: [EchoVariant] = []
        v.append(EchoVariant(name: "shipped"))
        var x = EchoVariant(name: "no-tail"); x.tail = DictationTail(tailMs: 0, drainTimeoutMs: 0, padMs: 0, nextBlockWaitMs: 0); v.append(x)
        x = EchoVariant(name: "pad-0"); x.tail.padMs = 0; v.append(x)
        x = EchoVariant(name: "pad-1120"); x.tail.padMs = 1120; v.append(x)
        x = EchoVariant(name: "tail-150"); x.tail.tailMs = 150; v.append(x)
        x = EchoVariant(name: "cold-mic-150"); x.dropHeadMs = 150; v.append(x)
        x = EchoVariant(name: "cold-mic-300"); x.dropHeadMs = 300; v.append(x)
        x = EchoVariant(name: "warm-preroll-300"); x.preRollMs = 300; v.append(x)
        x = EchoVariant(name: "quiet-0.1"); x.gain = 0.1; v.append(x)
        x = EchoVariant(name: "quiet-0.03"); x.gain = 0.03; v.append(x)
        x = EchoVariant(name: "quiet-0.1-normalized"); x.gain = 0.1; x.normalizePeakDb = -3; v.append(x)
        x = EchoVariant(name: "noise-15db"); x.noiseSnrDb = 15; v.append(x)
        x = EchoVariant(name: "echo-raw"); x.echoCoupling = 0.3; x.echoGate = false; v.append(x)
        x = EchoVariant(name: "echo-gated"); x.echoCoupling = 0.3; x.echoGate = true; v.append(x)
        return v
    }
}

// MARK: - Clip mic and speaker tap

/// Plays a clip at real time (÷ `speed`) in 1360-sample blocks (the tap's ~85 ms), then room silence until stopped.
/// With a speaker clip, each block's system audio is mixed into the mic at the coupling and handed to the tap
/// first, so the gate's reference is aligned with the mic like a real loopback.
public final class EchoClipMic: MicSource, @unchecked Sendable {
    public static let block = 1360
    let clip: [Float]
    let speaker: [Float]
    let coupling: Float
    let speed: Double
    weak var tap: EchoBenchTap?
    private let lock = NSLock()
    private var running = false
    private var clipDone = false
    private var thread: Thread?
    public init(_ clip: [Float], speaker: [Float] = [], coupling: Float = 0, speed: Double = 1, tap: EchoBenchTap? = nil) {
        self.clip = clip; self.speaker = speaker; self.coupling = coupling; self.speed = max(0.1, speed); self.tap = tap
    }
    public var done: Bool { lock.withLock { clipDone } }
    public func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws {
        lock.withLock { running = true; clipDone = false }
        let clip = self.clip, speaker = self.speaker, coupling = self.coupling, period = 0.085 / speed
        let t = Thread { [weak self] in
            var i = 0, n = 0
            let t0 = Date()
            while let self, self.lock.withLock({ self.running }) {
                var block: [Float]
                if i < clip.count { block = Array(clip[i..<min(i + Self.block, clip.count)]) } else { block = [Float](repeating: 0, count: Self.block) }
                if block.count < Self.block, i < clip.count { block += [Float](repeating: 0, count: Self.block - block.count) }
                if !speaker.isEmpty {
                    let sys = (0..<Self.block).map { k in i + k < speaker.count ? speaker[i + k] : 0 }
                    self.tap?.feed(sys)
                    for k in block.indices { block[k] = max(-1, min(1, block[k] + coupling * sys[k])) }
                }
                i += Self.block
                onSamples(block)
                if i >= clip.count { self.lock.withLock { self.clipDone = true } }
                n += 1
                let wait = t0.addingTimeInterval(Double(n) * period).timeIntervalSinceNow
                if wait > 0 { Thread.sleep(forTimeInterval: wait) }
            }
        }
        thread = t
        t.start()
    }
    public func stop() { lock.withLock { running = false } }
}

/// The speaker side of the echo variants: what "the Mac plays" (fed by the clip mic, block by block).
public final class EchoBenchTap: SystemAudioTap, @unchecked Sendable {
    public let name = "bench-tap"
    private let lock = NSLock()
    private var cb: (@Sendable ([Float]) -> Void)?
    public init() {}
    public func echoLikely() -> Bool { true }
    public func start(onSamples: @escaping @Sendable ([Float]) -> Void) async throws { lock.withLock { cb = onSamples } }
    public func stop() async { lock.withLock { cb = nil } }
    func feed(_ s: [Float]) { let c = lock.withLock { cb }; c?(s) }
}

// MARK: - Runner

/// Collects the engine's messages with arrival times (ms since the run's start).
final class EchoOutBox: @unchecked Sendable {
    private let lock = NSLock()
    private var msgs: [(Double, HelperMessage)] = []
    let t0 = ContinuousClock.now
    func add(_ m: HelperMessage) { let t = t0.msToNow(); lock.withLock { msgs.append((t, m)) } }
    var all: [(Double, HelperMessage)] { lock.withLock { msgs } }
}

public struct EchoBenchOptions: Sendable {
    /// Playback speed (1 = real time, what latency means; >1 for quick WER sweeps on a fast recogniser).
    public var speed: Double = 1
    /// Trim the clip's trailing silence so the release lands on the last word's end (the shipped bench case).
    public var trimTrailing = true
    /// The speaker clip for the echo variants (16 kHz), looped/cut to each clip's length.
    public var speaker: [Float] = []
    public init() {}
}

public enum EchoBench {
    /// One clip through one engine and variant: the engine's own timing record, the text, and its scores.
    public static func runClip(_ clip: EchoClip, engine: StreamingTranscriber, variant: EchoVariant, options: EchoBenchOptions) async -> [String: Any] {
        var samples = options.trimTrailing ? EchoClips.trimTrailingSilence(clip.samples) : clip.samples
        samples = variant.shape(samples)
        let out = EchoOutBox()
        let tap = EchoBenchTap()
        var speaker: [Float] = []
        if variant.echoCoupling != nil, !options.speaker.isEmpty {
            speaker = (0..<samples.count).map { options.speaker[$0 % options.speaker.count] }
        }
        let echo: DictationEcho? = variant.echoCoupling == nil ? nil : DictationEcho(reference: EchoReference(), tap: tap, permitted: { true })
        echo?.enabled = variant.echoGate
        let mic = EchoClipMic(samples, speaker: speaker, coupling: variant.echoCoupling ?? 0, speed: options.speed, tap: tap)
        let engineActor = DictationEngine(mic: mic, transcribers: [engine], inserter: nil, hotkeys: nil,
                                          send: { out.add($0) }, now: { Date().timeIntervalSince1970 * 1000 }, tail: variant.tail, echo: echo)
        // Pre-roll: a warm mic has been filling the ring buffer with room silence before the press.
        if variant.preRollMs > 0 {
            await engineActor.configure(warmMic: false, preRollMs: variant.preRollMs, streamAudio: false, pushKey: "")
            await engineActor.onSamples([Float](repeating: 0, count: 16 * variant.preRollMs))
        }
        let pressed = out.t0.msToNow()
        await engineActor.start(id: clip.name)
        while !mic.done { try? await Task.sleep(nanoseconds: 2_000_000) }
        let releasedAt = out.t0.msToNow()
        await engineActor.stop(id: clip.name)
        let stoppedAt = out.t0.msToNow()
        await echo?.settled()
        let msgs = out.all
        let text = msgs.compactMap { m -> String? in if case .dictationStopped(_, let t, _, _, _) = m.1 { t } else { nil } }.first ?? ""
        let firstPartialEvent = msgs.first { m in if case .dictationAsr(_, .partial(_, _, let t)) = m.1 { !t.isEmpty } else { false } }?.0
        let timing = await engineActor.lastTiming
        let refN = EchoWER.normalize(clip.reference), hypN = EchoWER.normalize(text)
        var r: [String: Any] = [
            "clip": clip.name, "engine": engine.name, "variant": variant.name, "audioMs": samples.count / 16,
            "hypothesis": text, "reference": clip.reference,
            "wer": EchoWER.score(ref: refN, hyp: hypN).json,
            "formattedWer": EchoWER.score(ref: EchoWER.formatted(clip.reference), hyp: EchoWER.formatted(text)).json,
            "tailWordsLost": EchoWER.tailLoss(ref: refN, hyp: hypN), "headWordsLost": EchoWER.headLoss(ref: refN, hyp: hypN),
            "releaseToStoppedMs": (stoppedAt - releasedAt).rounded(),
            "firstPartialMs": firstPartialEvent.map { ($0 - pressed).rounded() } ?? -1,
            "speed": options.speed,
        ]
        if let timing { r["stages"] = timing.json }
        return r
    }

    /// Every clip through every engine × variant. `progress` gets one line per clip (stderr in the CLI).
    public static func run(clips: [EchoClip], engines: [StreamingTranscriber], variants: [EchoVariant], options: EchoBenchOptions,
                           progress: @Sendable (String) -> Void = { _ in },
                           footprintMB: @Sendable () -> Double? = { nil }) async -> [String: Any] {
        var runs: [[String: Any]] = []
        var summary: [[String: Any]] = []
        for engine in engines {
            let t0 = ContinuousClock.now
            do { try await engine.prepare() } catch {
                progress("\(engine.name): unavailable (\(error))")
                summary.append(["engine": engine.name, "error": "\(error)"])
                continue
            }
            let loadMs = t0.msToNow().rounded()
            // The process footprint with this engine loaded (engines run one at a time, each unloaded after).
            let loadedMB = footprintMB()
            for variant in variants {
                var wer = EchoWER.Score(), fwer = EchoWER.Score()
                var rel: [Double] = [], first: [Double] = [], fin: [Double] = [], rtf: [Double] = []
                var tailDecode: [Double] = [], segments: [Double] = [], bgSegments: [Double] = [], backlog: [Double] = []
                var windows: [Double] = [], windowFixes: [Double] = [], windowsSkipped: [Double] = [], peakGained: [Double] = []
                var tailLost = 0, headLost = 0
                for clip in clips {
                    let r = await runClip(clip, engine: engine, variant: variant, options: options)
                    runs.append(r)
                    let refN = EchoWER.normalize(clip.reference), hyp = r["hypothesis"] as? String ?? ""
                    wer.add(EchoWER.score(ref: refN, hyp: EchoWER.normalize(hyp)))
                    fwer.add(EchoWER.score(ref: EchoWER.formatted(clip.reference), hyp: EchoWER.formatted(hyp)))
                    tailLost += r["tailWordsLost"] as? Int ?? 0; headLost += r["headWordsLost"] as? Int ?? 0
                    if let v = r["releaseToStoppedMs"] as? Double { rel.append(v) }
                    if let v = r["firstPartialMs"] as? Double, v >= 0 { first.append(v) }
                    if let st = r["stages"] as? [String: Any] {
                        if let f = st["finalizeMs"] as? Int, let c = st["closeMs"] as? Int, f >= 0, c >= 0 { fin.append(Double(f - c)) }
                        if let x = st["liveRtf"] as? Double { rtf.append(x) }
                        // A segmented recogniser (parakeet): the release's tail decode and the segment counts.
                        if let x = st["tailDecodeMs"] as? Double, x >= 0 { tailDecode.append(x) }
                        if let x = st["segments"] as? Int { segments.append(Double(x)) }
                        if let x = st["backgroundSegments"] as? Int { bgSegments.append(Double(x)) }
                        if let x = st["backlogWaitMs"] as? Double { backlog.append(x) }
                        if let x = st["cutWindows"] as? Int { windows.append(Double(x)) }
                        if let x = st["windowFixes"] as? Int { windowFixes.append(Double(x)) }
                        if let x = st["windowsSkipped"] as? Int { windowsSkipped.append(Double(x)) }
                        if let x = st["peakGained"] as? Int { peakGained.append(Double(x)) }
                    }
                    progress("\(engine.name) \(variant.name) \(clip.name): wer \(String(format: "%.3f", EchoWER.score(ref: refN, hyp: EchoWER.normalize(hyp)).wer)) release→final \(Int(r["releaseToStoppedMs"] as? Double ?? -1)) ms")
                }
                var row: [String: Any] = [
                    "engine": engine.name, "variant": variant.name, "clips": clips.count, "loadMs": loadMs,
                    "wer": wer.json, "formattedWer": fwer.json, "tailWordsLost": tailLost, "headWordsLost": headLost,
                    "releaseToStoppedMs": stats(rel), "firstPartialMs": stats(first), "finalDecodeMs": stats(fin), "liveRtf": stats(rtf),
                ]
                if let loadedMB { row["footprintMBLoaded"] = loadedMB }
                if !segments.isEmpty {
                    row["tailDecodeMs"] = stats(tailDecode); row["segments"] = stats(segments)
                    row["backgroundSegments"] = stats(bgSegments); row["backlogWaitMs"] = stats(backlog)
                }
                if windows.reduce(0, +) + windowsSkipped.reduce(0, +) > 0 {
                    row["cutWindows"] = stats(windows); row["windowFixes"] = stats(windowFixes); row["windowsSkipped"] = stats(windowsSkipped)
                }
                // ADR 0127: segments the gated peak scaled (present only with the gate on).
                if !peakGained.isEmpty { row["peakGained"] = stats(peakGained) }
                summary.append(row)
            }
            await engine.unload()
        }
        return ["summary": sentenceEndsVsWhole(summary, runs: runs), "runs": runs]
    }

    /// ADR 0120 gate: each engine's sentence ends against `parakeet-whole` on the same clip and variant (when
    /// that engine ran too), as `sentenceEndsVsWhole` on its summary row.
    static let wholeEngine = "parakeet-whole"
    static func sentenceEndsVsWhole(_ summary: [[String: Any]], runs: [[String: Any]]) -> [[String: Any]] {
        var whole: [String: String] = [:]
        for r in runs where r["engine"] as? String == wholeEngine {
            if let c = r["clip"] as? String, let v = r["variant"] as? String { whole["\(c)\u{1}\(v)"] = r["hypothesis"] as? String ?? "" }
        }
        guard !whole.isEmpty else { return summary }
        var ends: [String: EchoWER.SentenceEnds] = [:]
        for r in runs {
            guard let e = r["engine"] as? String, e != wholeEngine, let c = r["clip"] as? String, let v = r["variant"] as? String,
                  let w = whole["\(c)\u{1}\(v)"] else { continue }
            ends["\(e)\u{1}\(v)", default: EchoWER.SentenceEnds()].add(EchoWER.sentenceEnds(hyp: r["hypothesis"] as? String ?? "", ref: w))
        }
        return summary.map { row in
            guard let e = row["engine"] as? String, let v = row["variant"] as? String, let t = ends["\(e)\u{1}\(v)"] else { return row }
            var row = row; row["sentenceEndsVsWhole"] = t.json; return row
        }
    }

    /// p50 / p90 / max / mean of a sample.
    public static func stats(_ xs: [Double]) -> [String: Any] {
        guard !xs.isEmpty else { return ["n": 0] }
        let s = xs.sorted()
        func q(_ p: Double) -> Double { s[min(s.count - 1, Int((Double(s.count - 1) * p).rounded()))] }
        return ["n": s.count, "p50": q(0.5), "p90": q(0.9), "max": s.last!, "mean": (s.reduce(0, +) / Double(s.count) * 1000).rounded() / 1000]
    }
}

