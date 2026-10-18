#if os(macOS)
import AVFoundation
import CoreAudio
import Foundation
import QrtzShared

// ADR 0146 M2.5: split out of qrtz-helper/mac/Audio.swift (owner: shared; echo's and nano's capture both use it).

public struct AudioError: Error, CustomStringConvertible {
    public let description: String
    public init(_ d: String) { description = d }
    public static func os(_ what: String, _ status: OSStatus) -> AudioError { AudioError("\(what) failed (OSStatus \(status))") }
}

/// The tap's audio (its own rate) → 16 kHz mono float through AVAudioConverter at `.max` quality (ADR 0115:
/// band-limited, so speech energy above 8 kHz is filtered out instead of folding back into the band like linear
/// interpolation does). One converter per input format, kept across blocks (each block is handed over with
/// `.noDataNow`, so the filter's history carries to the next one) and recreated on a format change or `reset()`
/// (a device change). The filter holds back a few ms; `flush()` drains them with end-of-stream (dictation stop,
/// before the pad) and starts the converter afresh.
public final class ConverterResampler: @unchecked Sendable {
    public let format: AVAudioFormat
    private let conv: AVAudioConverter
    private let outFormat: AVAudioFormat

    public init?(format: AVAudioFormat) {
        guard let o = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
              let c = AVAudioConverter(from: format, to: o) else { return nil }
        c.sampleRateConverterAlgorithm = AVSampleRateConverterAlgorithm_Mastering
        c.sampleRateConverterQuality = AVAudioQuality.max.rawValue
        self.format = format; conv = c; outFormat = o
    }

    public func process(_ x: [Float]) -> [Float] {
        guard !x.isEmpty, let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(x.count)) else { return [] }
        input.frameLength = AVAudioFrameCount(x.count)
        x.withUnsafeBufferPointer { input.floatChannelData![0].update(from: $0.baseAddress!, count: x.count) }
        return run(input, frames: x.count)
    }

    /// End of stream: what the filter still holds, then a fresh start (the next block primes it again).
    public func flush() -> [Float] {
        let out = run(nil, frames: 0)
        conv.reset()
        return out
    }

    private func run(_ input: AVAudioPCMBuffer?, frames: Int) -> [Float] {
        let cap = AVAudioFrameCount((Double(frames) * 16_000 / format.sampleRate).rounded(.up)) + 1_024
        guard let out = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: cap) else { return [] }
        var given = false
        var err: NSError?
        let st = conv.convert(to: out, error: &err) { _, status in
            guard let input, !given else { status.pointee = input == nil ? .endOfStream : .noDataNow; return nil }
            given = true; status.pointee = .haveData; return input
        }
        if st == .error || err != nil { logErr("resampler: \(err.map { "\($0)" } ?? "convert failed")") }
        return Array(UnsafeBufferPointer(start: out.floatChannelData![0], count: Int(out.frameLength)))
    }
}

/// How MonoConverter resamples: `converter` (AVAudioConverter, the default) or `linear` (the old `DSP.Resampler`,
/// kept for the bench's comparison). `QRTZ_ECHO_RESAMPLER=linear|converter` overrides it.
public enum ResampleMode: String, Sendable {
    case converter, linear
    public static var fromEnvironment: ResampleMode {
        ProcessInfo.processInfo.environment["QRTZ_ECHO_RESAMPLER"].flatMap { ResampleMode(rawValue: $0.lowercased()) } ?? .converter
    }
}

/// Downmixes any non-interleaved/interleaved float or int16 PCM buffer to mono (the mean of the channels) and
/// resamples it to 16 kHz: AVAudioConverter, or the linear `DSP.Resampler` in `.linear` mode or when no converter
/// can be made for the rate. Thread-safe: the tap's thread converts, a dictation stop flushes.
public final class MonoConverter: @unchecked Sendable {
    public let mode: ResampleMode
    private let lock = NSLock()
    private var resampler: ConverterResampler?
    private var linear: DSP.Resampler?
    private var rate: Double = 0
    public init(mode: ResampleMode = .fromEnvironment) { self.mode = mode }

    public func convert(_ buf: AVAudioPCMBuffer) -> [Float] {
        let frames = Int(buf.frameLength)
        guard frames > 0 else { return [] }
        let mono = Self.downmix(buf)
        let sr = buf.format.sampleRate
        return lock.withLock {
            if rate != sr {
                // A new input format (another device, or the same one at another rate): a new converter.
                rate = sr
                resampler = nil; linear = nil
                if sr != 16_000 {
                    if mode == .converter,
                       let f = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: sr, channels: 1, interleaved: false) {
                        resampler = ConverterResampler(format: f)
                    }
                    if resampler == nil { linear = DSP.Resampler(from: sr, to: 16_000) }
                }
            }
            return resampler?.process(mono) ?? linear?.process(mono) ?? mono
        }
    }

    /// The converter's held-back tail (end of stream), for a dictation stop before its pad. The stream goes on:
    /// the next block starts a fresh filter. Linear and 16 kHz input hold nothing back.
    public func flush() -> [Float] { lock.withLock { resampler?.flush() ?? [] } }

    /// A device change: the next block makes a new converter (no filter history across devices).
    public func reset() { lock.withLock { rate = 0; resampler = nil; linear = nil } }

    public static func downmix(_ buf: AVAudioPCMBuffer) -> [Float] {
        let frames = Int(buf.frameLength)
        let ch = Int(buf.format.channelCount)
        var mono = [Float](repeating: 0, count: frames)
        if let data = buf.floatChannelData {
            if buf.format.isInterleaved {
                let p = data[0]
                for i in 0..<frames { var acc: Float = 0; for c in 0..<ch { acc += p[i * ch + c] }; mono[i] = acc / Float(ch) }
            } else {
                for c in 0..<ch { let p = data[c]; for i in 0..<frames { mono[i] += p[i] } }
                if ch > 1 { for i in 0..<frames { mono[i] /= Float(ch) } }
            }
        } else if let data = buf.int16ChannelData {
            for c in 0..<ch { let p = data[c]; for i in 0..<frames { mono[i] += Float(p[i]) / 32768 } }
            if ch > 1 { for i in 0..<frames { mono[i] /= Float(ch) } }
        }
        return mono
    }
}

/// The user's microphone via AVAudioEngine. Mixed with system audio so both sides of the call are captured.
/// N138: a device switch (AirPods connect or drop, a USB mic, the input picked in Control Center) stops Apple's
/// engine. Both `AVAudioEngineConfigurationChange` and a change of the default input device re-open the input on
/// a fresh engine (`MicRestart`: a few tries over ~1 s), delivering to the same `onSamples`, so every MicHub reader
/// keeps its subscription. A device that doesn't come back is reported once through `onLost`.
public final class MicCapture: @unchecked Sendable {
    private let lock = NSLock()
    private var engine = AVAudioEngine()
    private let conv = MonoConverter()
    /// The tap's blocks and a stop's flush reach `onSamples` one at a time, in order.
    private let deliver = NSLock()
    private var onSamples: (@Sendable ([Float]) -> Void)?
    private var configObserver: NSObjectProtocol?
    private var defaultInputListener: AudioObjectPropertyListenerBlock?
    private let restartQueue = DispatchQueue(label: "qrtz.mic.restart")
    private lazy var restart = MicRestart(queue: restartQueue)
    private var stopped = false
    public var onLost: (@Sendable (String) -> Void)?

    private static var defaultInputAddress = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDefaultInputDevice, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)

    public func start(_ onSamples: @escaping @Sendable ([Float]) -> Void) throws {
        lock.withLock { self.onSamples = onSamples; stopped = false }
        try open()
        let l: AudioObjectPropertyListenerBlock = { [weak self] _, _ in self?.changed("default input") }
        if AudioObjectAddPropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &Self.defaultInputAddress, restartQueue, l) == noErr {
            defaultInputListener = l
        }
    }

    /// Opens the input on the current engine (installs the tap at the device's own format) and starts it.
    private func open() throws {
        let (e, cb) = lock.withLock { (engine, onSamples) }
        guard let cb else { return }
        let input = e.inputNode
        let fmt = input.outputFormat(forBus: 0)
        guard fmt.sampleRate > 0, fmt.channelCount > 0 else { throw AudioError("no microphone input device") }
        input.installTap(onBus: 0, bufferSize: 4096, format: fmt) { [conv, deliver] buf, _ in deliver.withLock { cb(conv.convert(buf)) } }
        e.prepare()
        do { try e.start() } catch { input.removeTap(onBus: 0); throw error }
        if let o = configObserver { NotificationCenter.default.removeObserver(o) }
        configObserver = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: e, queue: nil) { [weak self] _ in
            self?.changed("configuration")
        }
    }

    private func changed(_ why: String) {
        guard !lock.withLock({ stopped }) else { return }
        restart.deviceChanged(attempt: { [weak self] in
            guard let self else { return nil }
            if self.lock.withLock({ self.stopped }) { return nil }
            let old = self.lock.withLock { self.engine }
            old.inputNode.removeTap(onBus: 0)
            old.stop()
            self.conv.reset()  // ADR 0115: a new device, a new converter
            // A fresh engine binds to the new default input; the old one keeps the device it was built on.
            self.lock.withLock { self.engine = AVAudioEngine() }
            do { try self.open(); return nil } catch { return "\(error)" }
        }, done: { [weak self] r in
            guard let self else { return }
            switch r {
            case .success(let ms):
                let fmt = self.lock.withLock { self.engine }.inputNode.outputFormat(forBus: 0)
                logErr("mic: input changed (\(why)) to \(Self.defaultInputName() ?? "?") (\(Int(fmt.sampleRate)) Hz), restarted in \(ms) ms")
            case .failure(let e):
                logErr("mic: input lost (\(why)): \(e.reason)")
                // Off the restart queue: the loss stops this capture (stop() syncs on that queue).
                let lost = self.onLost
                DispatchQueue.global().async { lost?(e.reason) }
            }
        })
    }

    public static func defaultInputName() -> String? {
        var dev = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &defaultInputAddress, 0, nil, &size, &dev) == noErr else { return nil }
        var addr = AudioObjectPropertyAddress(mSelector: kAudioObjectPropertyName, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var name: CFString = "" as CFString
        size = UInt32(MemoryLayout<CFString>.size)
        let st = withUnsafeMutablePointer(to: &name) { AudioObjectGetPropertyData(dev, &addr, 0, nil, &size, $0) }
        return st == noErr ? name as String : nil
    }

    /// ADR 0115: the resampler's held-back few ms, delivered now (end of stream) so a dictation stop has them
    /// before its pad. Capture goes on.
    public func flush() {
        deliver.withLock {
            guard let cb = lock.withLock({ onSamples }) else { return }
            let s = conv.flush()
            if !s.isEmpty { cb(s) }
        }
    }

    public func stop() {
        lock.withLock { stopped = true; onSamples = nil }
        restart.cancel()
        if let l = defaultInputListener {
            AudioObjectRemovePropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &Self.defaultInputAddress, restartQueue, l)
            defaultInputListener = nil
        }
        if let o = configObserver { NotificationCenter.default.removeObserver(o); configObserver = nil }
        restartQueue.sync {}  // no restart attempt runs past here
        let e = lock.withLock { engine }
        e.inputNode.removeTap(onBus: 0)
        e.stop()
    }
}

/// The default output device (what the speakers play), for the meeting's and echo's process taps.
public enum MacAudioDevice {
    public static func defaultOutputUID() throws -> String {
        var dev = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDefaultSystemOutputDevice, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var st = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &dev)
        guard st == noErr else { throw AudioError.os("default output device", st) }
        var uid: CFString = "" as CFString
        size = UInt32(MemoryLayout<CFString>.size)
        addr.mSelector = kAudioDevicePropertyDeviceUID
        st = withUnsafeMutablePointer(to: &uid) { AudioObjectGetPropertyData(dev, &addr, 0, nil, &size, $0) }
        guard st == noErr else { throw AudioError.os("output device UID", st) }
        return uid as String
    }
}
#endif
