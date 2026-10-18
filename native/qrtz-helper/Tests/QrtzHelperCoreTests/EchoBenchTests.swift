import Foundation
import XCTest
@testable import QrtzHelperCore

/// Hears "hello world" once it has been fed any non-silent audio; its partial appears after 0.2 s of audio.
final class BenchFakeASR: StreamingTranscriber, @unchecked Sendable {
    let name = "bench-fake"
    private let lock = NSLock()
    private var loud = 0, total = 0
    func prepare() async throws {}
    func append(_ s: [Float]) async { lock.withLock { total += s.count; loud += s.filter { abs($0) > 0.01 }.count } }
    func transcript() async -> String { lock.withLock { total >= 3_200 && loud > 0 ? "hello" : "" } }
    func finish() async -> String { lock.withLock { loud > 0 ? "hello world" : "" } }
    func reset() async { lock.withLock { loud = 0; total = 0 } }
}

final class EchoBenchTests: XCTestCase {
    func testNormalizeFoldsCasePunctuationAndNumbers() {
        XCTAssertEqual(EchoWER.normalize("Move it to Thursday, at twenty five past 3!"), ["move", "it", "to", "thursday", "at", "25", "past", "3"])
        XCTAssertEqual(EchoWER.normalize("Don't stop—it's 50%."), ["don't", "stop", "it's", "50", "percent"])
    }

    func testEditsCountEachKind() {
        let e = EchoWER.edits(["a", "b", "c", "d"], ["a", "x", "c", "d", "e"])
        XCTAssertEqual(e.sub, 1); XCTAssertEqual(e.del, 0); XCTAssertEqual(e.ins, 1)
        let d = EchoWER.edits(["a", "b", "c"], ["a"])
        XCTAssertEqual(d.del, 2)
        XCTAssertEqual(EchoWER.score(ref: ["a", "b"], hyp: ["a", "b"]).wer, 0)
        XCTAssertEqual(EchoWER.score(ref: [], hyp: []).wer, 0)
    }

    func testTailAndHeadLoss() {
        let ref = EchoWER.normalize("add a note about pineapple")
        XCTAssertEqual(EchoWER.tailLoss(ref: ref, hyp: EchoWER.normalize("add a note about pineapple.")), 0)
        XCTAssertEqual(EchoWER.tailLoss(ref: ref, hyp: EchoWER.normalize("add a note about")), 1)
        XCTAssertEqual(EchoWER.tailLoss(ref: ref, hyp: EchoWER.normalize("add a note")), 2)
        XCTAssertEqual(EchoWER.headLoss(ref: ref, hyp: EchoWER.normalize("a note about pineapple")), 1)
    }

    func testTailLossMatchesWholeWords() {
        // N163: "pin" is not the end of "pineapple".
        XCTAssertEqual(EchoWER.tailLoss(ref: EchoWER.normalize("drop the pin"), hyp: EchoWER.normalize("drop the pineapple")), 1)
        XCTAssertEqual(EchoWER.headLoss(ref: EchoWER.normalize("pin it there"), hyp: EchoWER.normalize("spin it there")), 1)
        XCTAssertEqual(EchoWER.tailLoss(ref: EchoWER.normalize("drop the pin"), hyp: EchoWER.normalize("drop the pin, ok")), 0)
    }

    func testTypographicQuotesScoreAsAscii() {
        // N162: a reference typed in TextEdit or Notes.
        XCTAssertEqual(EchoWER.normalize("Don\u{2019}t stop"), EchoWER.normalize("don't stop"))
        XCTAssertEqual(EchoWER.formatted("\u{201C}Don\u{2019}t\u{201D}"), ["\"Don't\""])
        XCTAssertEqual(EchoWER.score(ref: EchoWER.normalize("It\u{2019}s fine."), hyp: EchoWER.normalize("it's fine")).errors, 0)
    }

    func testVariantShapingIsDeterministicAndBounded() {
        let clip = (0..<16_000).map { Float(sin(Double($0) / 10)) * 0.5 }
        var v = EchoVariant(name: "x"); v.noiseSnrDb = 10; v.dropHeadMs = 100; v.gain = 3
        let a = v.shape(clip), b = v.shape(clip)
        XCTAssertEqual(a, b)
        XCTAssertEqual(a.count, 16_000 - 1_600)
        XCTAssertLessThanOrEqual(a.map { abs($0) }.max() ?? 0, 1)
        var n = EchoVariant(name: "n"); n.gain = 0.1; n.normalizePeakDb = -6
        let peak = n.shape(clip).map { abs($0) }.max() ?? 0
        XCTAssertEqual(peak, 0.1 * powf(10, -6.0 / 20), accuracy: 1e-3)
    }

    func testTimingLineIsContentFree() {
        var t = DictationTiming()
        t.asr = "nemotron-560ms"; t.audioMs = 2_000; t.appendTotalMs = 40; t.words = 7; t.tailMs = 50
        let line = t.logLine
        XCTAssertTrue(line.hasPrefix("echo timing: asr=nemotron-560ms audio_ms=2000"))
        XCTAssertTrue(line.contains("live_rtf=0.020"))
        XCTAssertTrue(line.contains("word_count=7"))
        t.asr = "evil name with words\nnext"
        XCTAssertFalse(t.logLine.contains("\n"))
        XCTAssertFalse(t.logLine.contains(" with "))
    }

    func testEngineLogsOneTimingLinePerDictation() async {
        let out = Box<[HelperMessage]>([])
        let mic = FakeMic(), asr = RecordingASR()
        let e = DictationEngine(mic: mic, transcribers: [asr], inserter: nil, hotkeys: nil, send: { m in out.mutate { $0.append(m) } }, now: { 0 },
                                tail: DictationTail(tailMs: 0, drainTimeoutMs: 50, padMs: 0, nextBlockWaitMs: 0))
        await e.start(id: "d1")
        mic.cb?([Float](repeating: 0.2, count: 1_600))
        try? await Task.sleep(nanoseconds: 50_000_000)
        await e.stop(id: "d1")
        let lines = out.value.compactMap { m -> String? in if case .log(.info, let s) = m, s.hasPrefix("echo timing:") { s } else { nil } }
        XCTAssertEqual(lines.count, 1)
        XCTAssertFalse(lines[0].contains("hello"), "never the text")
        XCTAssertTrue(lines[0].contains("word_count=2"))
        let t = await e.lastTiming
        XCTAssertEqual(t?.asr, "rec")
        XCTAssertEqual(t?.audioMs, 100)
        XCTAssertGreaterThanOrEqual(t?.releaseToStoppedMs ?? -1, 0)
        XCTAssertGreaterThanOrEqual(t?.finalizeMs ?? -1, 0)
        // The stopped answer comes first; the timing line never delays it.
        let iStop = out.value.firstIndex { if case .dictationStopped = $0 { true } else { false } }
        let iLog = out.value.firstIndex { if case .log(.info, let s) = $0 { s.hasPrefix("echo timing:") } else { false } }
        XCTAssertLessThan(iStop ?? .max, iLog ?? -1)
    }

    func testBenchRunsAClipThroughTheEngine() async {
        let clip = EchoClip(name: "c1", samples: [Float](repeating: 0.3, count: 8_000) + [Float](repeating: 0, count: 800), reference: "Hello world.")
        var o = EchoBenchOptions(); o.speed = 8
        let r = await EchoBench.run(clips: [clip], engines: [BenchFakeASR()], variants: [EchoVariant(name: "shipped")], options: o)
        let runs = r["runs"] as? [[String: Any]] ?? []
        XCTAssertEqual(runs.count, 1)
        XCTAssertEqual(runs.first?["hypothesis"] as? String, "hello world")
        XCTAssertEqual((runs.first?["wer"] as? [String: Any])?["wer"] as? Double, 0)
        XCTAssertEqual((runs.first?["formattedWer"] as? [String: Any])?["wer"] as? Double, 1)
        XCTAssertNotNil(runs.first?["stages"])
        let s = (r["summary"] as? [[String: Any]])?.first
        XCTAssertEqual(s?["variant"] as? String, "shipped")
        XCTAssertNoThrow(try JSONSerialization.data(withJSONObject: r))
    }

    func testEchoVariantsGateTheSpeakerAudio() async {
        // The "user" is silent; only the speakers play. Raw, the fake hears it; gated, it is held down to silence.
        let clip = EchoClip(name: "e", samples: [Float](repeating: 0, count: 16_000), reference: "")
        var o = EchoBenchOptions(); o.speed = 8; o.trimTrailing = false
        o.speaker = (0..<16_000).map { Float(sin(Double($0) / 7)) * 0.5 }
        var raw = EchoVariant(name: "echo-raw"); raw.echoCoupling = 0.3; raw.echoGate = false
        var gated = EchoVariant(name: "echo-gated"); gated.echoCoupling = 0.3; gated.echoGate = true
        let a = await EchoBench.runClip(clip, engine: BenchFakeASR(), variant: raw, options: o)
        let b = await EchoBench.runClip(clip, engine: BenchFakeASR(), variant: gated, options: o)
        XCTAssertEqual(a["hypothesis"] as? String, "hello world")
        let st = b["stages"] as? [String: Any]
        XCTAssertGreaterThan(st?["gateDucked"] as? Int ?? 0, 0)
    }

    func testClipPairsNeedAReference() throws {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("echo-clips-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: d) }
        try Data().write(to: d.appendingPathComponent("b.wav"))
        try "Second.".write(to: d.appendingPathComponent("b.txt"), atomically: true, encoding: .utf8)
        try Data().write(to: d.appendingPathComponent("a.aiff"))
        try "First.".write(to: d.appendingPathComponent("a.txt"), atomically: true, encoding: .utf8)
        try Data().write(to: d.appendingPathComponent("orphan.wav"))
        let p = EchoClips.pairs(in: d)
        XCTAssertEqual(p.map(\.name), ["a", "b"])
        XCTAssertEqual(p.first?.reference, "First.")
    }
}
