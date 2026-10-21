#if os(macOS) && QRTZ_DIAG  // QRT-299: diagnostic CLI, never in a release or bundled build
import AVFoundation
import Foundation
import QrtzEchoMac  // QrtzEcho comes re-exported through QrtzHelperCore (the offline_graph allowlist)
import QrtzHelperCore
import QrtzMacShared
import QrtzShared

/// QRT-270: the echo warm-up race, scripted, for CI (scripts/echo-warmup-ci.sh). No mic, no Accessibility, no key:
///
///   qrtz-helper --echo-warmup <clip.wav> --expect "<words>" --press-at 0|<ms>|ready [--out result.json]
///
/// Launches the real helper controller cold (the startup preload through the "asr" gate, Parakeet TDT v2 from
/// `QRTZ_MODELS_DIR`), then makes one push-to-talk press the way the fn key does: the hotkey monitor's `.down` edge
/// (which arms the mic), the host's `dictation.start`, the key's `.up` at the clip's end and the host's
/// `dictation.stop`, handled one at a time like HelperStdio does. Only the hotkey monitor, the mic (the clip played
/// in real time from the press, then silence) and the insertion are stand-ins; the dictation engine, its ready gate
/// and the recogniser are the shipped ones. Exits non-zero when the transcript is empty or wrong, or when a press
/// before ready was not held by the gate ("still warming"). Nothing here is used by the app's runtime.
enum EchoWarmupCLI {
    static func run(_ args: [String]) -> Never {
        Task {
            let ok = await measure(args)
            exit(ok ? 0 : 1)
        }
        dispatchMain()
    }

    static func value(_ args: [String], _ flag: String) -> String? {
        guard let i = args.firstIndex(of: flag), i + 1 < args.count else { return nil }
        return args[i + 1]
    }

    /// The press's key: the fn monitor's edges, fired by the script.
    final class ScriptedKey: HotkeyMonitor, @unchecked Sendable {
        private let lock = NSLock()
        private var cb: (@Sendable (HotkeyAction) -> Void)?
        func start(key: String, consumeFn: Bool, onChange: @escaping @Sendable (HotkeyAction) -> Void) -> Bool {
            lock.withLock { cb = onChange }; return true
        }
        func stop() {}
        func press(_ a: HotkeyAction) { lock.withLock { cb }?(a) }
    }

    /// The mic: from its start (the press's arm), the clip in real time in 85 ms blocks, then silence.
    final class ClipMic: MicSource, @unchecked Sendable {
        let clip: [Float]
        private let lock = NSLock()
        private var gen = 0
        init(_ clip: [Float]) { self.clip = clip }
        func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws {
            let g = lock.withLock { gen += 1; return gen }
            let clip = self.clip
            Thread.detachNewThread { [weak self] in
                let block = 1_360
                var i = 0
                let t0 = Date()
                while let s = self, s.lock.withLock({ s.gen == g }) {
                    var b = [Float](repeating: 0, count: block)
                    if i < clip.count { let k = min(block, clip.count - i); b.replaceSubrange(0..<k, with: clip[i..<(i + k)]) }
                    i += block
                    onSamples(b)
                    let due = t0.addingTimeInterval(Double(i) / 16_000)
                    let wait = due.timeIntervalSinceNow
                    if wait > 0 { Thread.sleep(forTimeInterval: wait) }
                }
            }
        }
        func stop() { lock.withLock { gen += 1 } }
    }

    final class NoInsertion: InsertionBackend, @unchecked Sendable {
        func focusedAppName() -> String? { "ci" }
        func axSetSelectedText(_ text: String) -> Bool { true }
        func axInsertIntoValue(_ text: String) -> Bool { true }
        func pasteboardSnapshot() -> Any? { nil }
        func pasteboardSetString(_ text: String) {}
        func pasteboardRestore(_ snapshot: Any?) {}
        func pasteboardChangeCount() -> Int { 0 }
        func postPaste() -> Bool { true }
        func focusedFieldIsSecure() -> Bool { false }
    }

    struct Granted: PermissionsProvider {
        func status() -> [OsPermission: PermissionState] { [.accessibility: .granted, .microphone: .granted, .screenRecording: .granted] }
        func request(_ p: OsPermission) async {}
    }

    /// Everything the controller sent, stamped with ms since launch.
    final class Tape: @unchecked Sendable {
        private let lock = NSLock()
        private var items: [(ms: Double, m: HelperMessage)] = []
        let t0 = ContinuousClock.now
        func ms() -> Double { let d = t0.duration(to: .now); return Double(d.components.seconds) * 1000 + Double(d.components.attoseconds) / 1e15 }
        func add(_ m: HelperMessage) {
            let t = ms()
            lock.withLock { items.append((t, m)) }
            if case .log(_, let s) = m { logErr(String(format: "%8.0f ms  %@", t, s)) }
        }
        func first(_ f: (HelperMessage) -> Bool) -> (ms: Double, m: HelperMessage)? { lock.withLock { items.first { f($0.m) } } }
        func logMs(_ prefix: String) -> Double? {
            first { if case .log(_, let s) = $0 { s.hasPrefix(prefix) } else { false } }?.ms
        }
        func logs(containing s: String) -> Bool { lock.withLock { items.contains { if case .log(_, let x) = $0.m { x.contains(s) } else { false } } } }
        func waitFor(_ timeoutMs: Double, _ f: @escaping (HelperMessage) -> Bool) async -> (ms: Double, m: HelperMessage)? {
            let end = ms() + timeoutMs
            while ms() < end {
                if let x = first(f) { return x }
                try? await Task.sleep(nanoseconds: 5_000_000)
            }
            return first(f)
        }
    }

    /// The clip as 16 kHz mono (the live tap's converter).
    static func load(_ path: String) throws -> [Float] {
        let f = try AVAudioFile(forReading: URL(fileURLWithPath: path))
        let fmt = f.processingFormat
        guard let all = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: AVAudioFrameCount(f.length)) else { throw Bench.Failure(description: "buffer") }
        try f.read(into: all)
        let conv = MonoConverter(mode: .converter)
        return conv.convert(all) + conv.flush()
    }

    /// Lowercased words, punctuation dropped.
    static func words(_ s: String) -> [String] {
        var kept = ""
        for ch in s.lowercased() { kept.append(ch.isLetter || ch.isNumber || ch == "'" ? ch : " ") }
        return kept.split(separator: " ").map { String($0) }
    }

    /// Word error rate (Levenshtein over words).
    static func wer(_ ref: [String], _ hyp: [String]) -> Double {
        guard !ref.isEmpty else { return hyp.isEmpty ? 0 : 1 }
        var d = Array(0...hyp.count)
        for i in 1...ref.count {
            var prev = d[0]; d[0] = i
            for j in stride(from: 1, through: hyp.count, by: 1) {
                let cur = d[j]
                d[j] = min(d[j] + 1, d[j - 1] + 1, prev + (ref[i - 1] == hyp[j - 1] ? 0 : 1))
                prev = cur
            }
        }
        return Double(d[hyp.count]) / Double(ref.count)
    }

    static func measure(_ args: [String]) async -> Bool {
        let tape = Tape()
        guard let clipPath = value(args, "--echo-warmup"), let expect = value(args, "--expect") else {
            logErr("usage: --echo-warmup <clip.wav> --expect \"<words>\" --press-at 0|<ms>|ready [--out f.json] [--max-wer 0.15]"); return false
        }
        let pressAt = value(args, "--press-at") ?? "0"
        let maxWer = Double(value(args, "--max-wer") ?? "") ?? 0.15
        let clip: [Float]
        do { clip = try load(clipPath) } catch { logErr("echo-warmup: \(clipPath): \(error)"); return false }
        let clipMs = Double(clip.count) / 16

        let key = ScriptedKey()
        var deps = HelperDeps(permissions: Granted(), meetings: nil, audio: [], transcribers: [], helperVersion: helperVersion,
                              os: macOSVersionString(), features: [], dictationMic: ClipMic(clip),
                              dictationTranscribers: [SegmentedTranscriber(ParakeetBatch(), rule: .fromEnvironment())],
                              insertion: NoInsertion(), hotkeys: key, permissionPollMs: 0)
        deps.preloadDictationAsr = true
        let c = HelperController(deps: deps, send: { tape.add($0) }, now: HelperStdio.epochMs)
        await c.start()  // cold: the startup preload starts now
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 300, streamAudio: false, pushKey: "fn"))

        // When to press: at launch, a fixed time in, or as soon as echo says it is ready.
        if pressAt == "ready" {
            guard await tape.waitFor(180_000, { if case .log(_, let s) = $0 { s.hasPrefix("echo asr: ready") } else { false } }) != nil else {
                logErr("echo-warmup: FAIL: echo never became ready"); return false
            }
        } else if let ms = Double(pressAt), ms > 0 {
            let wait = ms - tape.ms()
            if wait > 0 { try? await Task.sleep(nanoseconds: UInt64(wait * 1_000_000)) }
        }
        let readyBeforePress = tape.logMs("echo asr: ready") != nil
        let pressMs = tape.ms()
        key.press(.down)
        // The host's dictation.start (after its admit round trip), then the release when the clip is over, and the
        // host's dictation.stop. HelperStdio handles lines one at a time, so the stop waits behind a start that waits.
        let host = Task {
            await c.handle(.dictationStart(id: "w1"))
            let releaseAt = pressMs + clipMs + 250
            let wait = releaseAt - tape.ms()
            if wait > 0 { try? await Task.sleep(nanoseconds: UInt64(wait * 1_000_000)) }
            let rel = tape.ms()
            key.press(.up)
            await c.handle(.dictationStop(id: "w1"))
            return rel
        }
        let releaseMs = await host.value
        let stopped = await tape.waitFor(60_000) { if case .dictationStopped(let id, _, _, _, _) = $0 { id == "w1" } else { false } }
        let readyMs = tape.logMs("echo asr: ready")
        let startedMs = tape.first { if case .dictationStarted(let id, _) = $0 { id == "w1" } else { false } }?.ms
        let warmingLogged = tape.logs(containing: "dictation w1: echo asr still warming")
        let asrError = tape.first { if case let .error(code, _, _, _, _, _, _, _) = $0 { code == "dictation.asr" } else { false } }
        var text = ""
        if let s = stopped, case .dictationStopped(_, let t, _, _, _) = s.m { text = t }
        let w = wer(words(expect), words(text))

        var failures: [String] = []
        if stopped == nil { failures.append("no dictation.stopped within 60 s of the release") }
        if asrError != nil { failures.append("the press was refused (error dictation.asr) instead of waiting for the load") }
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { failures.append("empty transcript") }
        else if w > maxWer { failures.append(String(format: "transcript does not match the clip (WER %.2f > %.2f)", w, maxWer)) }
        // The ready gate: a press before ready must be held ("still warming") and must not start before ready.
        if !readyBeforePress {
            if !warmingLogged { failures.append("press before ready was not held by the gate (no 'still warming')") }
            if let s = startedMs, let r = readyMs, s < r { failures.append("dictation started before echo was ready") }
        }

        let result: [String: Any] = [
            "pressAt": pressAt, "pressMs": pressMs.rounded(), "readyMs": readyMs.map { $0.rounded() } ?? NSNull(),
            "readyBeforePress": readyBeforePress, "stillWarming": warmingLogged,
            "startedMs": startedMs.map { $0.rounded() } ?? NSNull(), "releaseMs": releaseMs.rounded(),
            "textMs": stopped.map { $0.ms.rounded() } ?? NSNull(),
            "releaseToTextMs": stopped.map { ($0.ms - releaseMs).rounded() } ?? NSNull(),
            "pressToTextMs": stopped.map { ($0.ms - pressMs).rounded() } ?? NSNull(),
            "clipMs": clipMs.rounded(), "text": text, "expect": expect, "wer": w, "ok": failures.isEmpty, "failures": failures,
        ]
        if let data = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) {
            if let out = value(args, "--out") { try? data.write(to: URL(fileURLWithPath: out)) }
            print(String(decoding: data, as: UTF8.self))
        }
        for f in failures { logErr("echo-warmup: FAIL: \(f)") }
        await c.shutdown()
        return failures.isEmpty
    }
}
#endif
