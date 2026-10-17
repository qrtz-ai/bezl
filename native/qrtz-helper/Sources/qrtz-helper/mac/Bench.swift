#if os(macOS)
import AVFoundation
import Foundation
import QrtzHelperCore

/// `qrtz-helper --bench --mic a.aiff[,b.aiff…] [--system c.aiff[,…]]` (ADR 0079 verification on a Mac): runs the real
/// Nemotron transcribers and Nemotron 3 diarizers from `$QRTZ_MODELS_DIR` on audio files (several files per channel
/// are joined with 0.5 s of silence, e.g. two `say` voices) and prints one JSON object: load times, per-channel text
/// and speaker turns, real-time factor, talk latency (the mic clip fed at real time to a warm instance: first
/// partial, and release to final) and the process footprint. Never downloads; stdout is the JSON only.
enum Bench {
    static func run(_ args: [String]) -> Never {
        Task {
            do { print(try await measure(args)) } catch { logErr("bench: \(error)"); exit(1) }
            exit(0)
        }
        dispatchMain()
    }

    struct Failure: Error, CustomStringConvertible { let description: String }

    static func value(_ args: [String], _ flag: String) -> [String] {
        guard let i = args.firstIndex(of: flag), i + 1 < args.count else { return [] }
        return args[i + 1].split(separator: ",").map(String.init)
    }

    static func footprintMB() -> Double {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        return kr == KERN_SUCCESS ? (Double(info.phys_footprint) / 1_048_576).rounded() : -1
    }

    static func load16k(_ path: String) throws -> [Float] {
        let f = try AVAudioFile(forReading: URL(fileURLWithPath: path))
        let src = f.processingFormat
        guard let buf = AVAudioPCMBuffer(pcmFormat: src, frameCapacity: AVAudioFrameCount(f.length)),
              let dst = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
              let conv = AVAudioConverter(from: src, to: dst)
        else { throw Failure(description: "can't read \(path)") }
        try f.read(into: buf)
        conv.downmix = true
        let cap = AVAudioFrameCount(Double(buf.frameLength) * 16_000 / src.sampleRate) + 4096
        guard let out = AVAudioPCMBuffer(pcmFormat: dst, frameCapacity: cap) else { throw Failure(description: "buffer") }
        var fed = false
        var err: NSError?
        _ = conv.convert(to: out, error: &err) { _, status in
            if fed { status.pointee = .endOfStream; return nil }
            fed = true; status.pointee = .haveData; return buf
        }
        if let err { throw err }
        return Array(UnsafeBufferPointer(start: out.floatChannelData![0], count: Int(out.frameLength)))
    }

    static func joined(_ paths: [String]) throws -> [Float] {
        var all: [Float] = []
        for (i, p) in paths.enumerated() {
            if i > 0 { all += [Float](repeating: 0, count: 8_000) }
            all += try load16k(p)
        }
        return all
    }

    static func ms(since t: Date) -> Double { (Date().timeIntervalSince(t) * 1000).rounded() }

    static func channel(_ name: String, _ samples: [Float], _ asr: NemotronTranscriber, _ diar: NemotronChannelDiarizer)
        async -> [String: Any]
    {
        await asr.reset(); await diar.reset()
        var turns: [DiarizedTurn] = []
        let t = Date()
        var i = 0
        while i < samples.count {
            let c = Array(samples[i..<min(i + 2_560, samples.count)])
            await asr.append(c)
            turns += await diar.append(c)
            i += 2_560
        }
        let text = await asr.finish()
        turns += await diar.finish()
        let audioMs = Double(samples.count) / 16
        let took = ms(since: t)
        return [
            "channel": name, "audioMs": audioMs.rounded(), "processMs": took,
            "rtf": (took / max(audioMs, 1) * 1000).rounded() / 1000, "text": text,
            "turns": turns.map { ["speaker": "\(name):\($0.speaker)", "startMs": $0.startMs, "endMs": $0.endMs] as [String: Any] },
            "speakers": Set(turns.map(\.speaker)).count,
        ]
    }

    static func measure(_ args: [String]) async throws -> String {
        let micPaths = value(args, "--mic"), sysPaths = value(args, "--system")
        guard !micPaths.isEmpty else { throw Failure(description: "usage: --bench --mic a.aiff[,b.aiff] [--system c.aiff]") }
        let mic = try joined(micPaths)
        let sys = try sysPaths.isEmpty ? [] : joined(sysPaths)
        var r: [String: Any] = ["footprintStartMB": footprintMB()]

        // Talk's own instance (ASR only, no diarizer), loaded cold, then a dictation of the mic clip at real time.
        let talk = NemotronTranscriber()
        var t = Date()
        try await talk.prepare()
        r["talkLoadMs"] = ms(since: t)
        r["footprintTalkLoadedMB"] = footprintMB()
        let dictation = Array(mic.prefix(16_000 * 12))
        await talk.reset()
        t = Date()
        var firstPartial: Double?
        var i = 0
        while i < dictation.count {
            let chunkStart = Date()
            await talk.append(Array(dictation[i..<min(i + 1_600, dictation.count)]))
            if firstPartial == nil, !(await talk.transcript()).isEmpty { firstPartial = ms(since: t) }
            i += 1_600
            let left = 0.1 - Date().timeIntervalSince(chunkStart)
            if left > 0 { try? await Task.sleep(nanoseconds: UInt64(left * 1e9)) }
        }
        let released = Date()
        let talkText = await talk.finish()
        r["talk"] = [
            "audioMs": (Double(dictation.count) / 16).rounded(), "firstPartialMs": firstPartial ?? -1,
            "releaseToFinalMs": ms(since: released), "text": talkText,
        ] as [String: Any]

        // Note: two Nemotron instances and two diarizers, as in a recording.
        let micAsr = NemotronTranscriber(), sysAsr = NemotronTranscriber()
        let micDiar = NemotronChannelDiarizer(), sysDiar = NemotronChannelDiarizer()
        t = Date()
        try await micAsr.prepare(); try await sysAsr.prepare()
        r["noteAsrLoadMs"] = ms(since: t)
        t = Date()
        try await micDiar.prepare(); try await sysDiar.prepare()
        r["noteDiarizerLoadMs"] = ms(since: t)
        r["footprintAllLoadedMB"] = footprintMB()
        var channels = [await channel("mic", mic, micAsr, micDiar)]
        if !sys.isEmpty { channels.append(await channel("system", sys, sysAsr, sysDiar)) }
        r["note"] = channels
        r["footprintAfterNoteMB"] = footprintMB()
        await micAsr.unload(); await sysAsr.unload(); await micDiar.unload(); await sysDiar.unload()
        r["footprintAfterUnloadMB"] = footprintMB()
        let data = try JSONSerialization.data(withJSONObject: r, options: [.prettyPrinted, .sortedKeys])
        return String(decoding: data, as: UTF8.self)
    }
}
#endif
