#if os(macOS)
import AVFoundation
import CoreAudio
import CoreMedia
import FluidAudio
import Foundation
import QrtzHelperCore
import ScreenCaptureKit

struct AudioError: Error, CustomStringConvertible {
    let description: String
    init(_ d: String) { description = d }
    static func os(_ what: String, _ status: OSStatus) -> AudioError { AudioError("\(what) failed (OSStatus \(status))") }
}

/// The tap's audio (its own rate) → 16 kHz mono float through AVAudioConverter at `.max` quality (ADR 0115:
/// band-limited, so speech energy above 8 kHz is filtered out instead of folding back into the band like linear
/// interpolation does). One converter per input format, kept across blocks (each block is handed over with
/// `.noDataNow`, so the filter's history carries to the next one) and recreated on a format change or `reset()`
/// (a device change). The filter holds back a few ms; `flush()` drains them with end-of-stream (dictation stop,
/// before the pad) and starts the converter afresh.
final class ConverterResampler: @unchecked Sendable {
    let format: AVAudioFormat
    private let conv: AVAudioConverter
    private let outFormat: AVAudioFormat

    init?(format: AVAudioFormat) {
        guard let o = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
              let c = AVAudioConverter(from: format, to: o) else { return nil }
        c.sampleRateConverterAlgorithm = AVSampleRateConverterAlgorithm_Mastering
        c.sampleRateConverterQuality = AVAudioQuality.max.rawValue
        self.format = format; conv = c; outFormat = o
    }

    func process(_ x: [Float]) -> [Float] {
        guard !x.isEmpty, let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(x.count)) else { return [] }
        input.frameLength = AVAudioFrameCount(x.count)
        x.withUnsafeBufferPointer { input.floatChannelData![0].update(from: $0.baseAddress!, count: x.count) }
        return run(input, frames: x.count)
    }

    /// End of stream: what the filter still holds, then a fresh start (the next block primes it again).
    func flush() -> [Float] {
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
enum ResampleMode: String, Sendable {
    case converter, linear
    static var fromEnvironment: ResampleMode {
        ProcessInfo.processInfo.environment["QRTZ_ECHO_RESAMPLER"].flatMap { ResampleMode(rawValue: $0.lowercased()) } ?? .converter
    }
}

/// Downmixes any non-interleaved/interleaved float or int16 PCM buffer to mono (the mean of the channels) and
/// resamples it to 16 kHz: AVAudioConverter, or the linear `DSP.Resampler` in `.linear` mode or when no converter
/// can be made for the rate. Thread-safe: the tap's thread converts, a dictation stop flushes.
final class MonoConverter: @unchecked Sendable {
    let mode: ResampleMode
    private let lock = NSLock()
    private var resampler: ConverterResampler?
    private var linear: DSP.Resampler?
    private var rate: Double = 0
    init(mode: ResampleMode = .fromEnvironment) { self.mode = mode }

    func convert(_ buf: AVAudioPCMBuffer) -> [Float] {
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
    func flush() -> [Float] { lock.withLock { resampler?.flush() ?? [] } }

    /// A device change: the next block makes a new converter (no filter history across devices).
    func reset() { lock.withLock { rate = 0; resampler = nil; linear = nil } }

    static func downmix(_ buf: AVAudioPCMBuffer) -> [Float] {
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
final class MicCapture: @unchecked Sendable {
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
    var onLost: (@Sendable (String) -> Void)?

    private static var defaultInputAddress = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDefaultInputDevice, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)

    func start(_ onSamples: @escaping @Sendable ([Float]) -> Void) throws {
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

    static func defaultInputName() -> String? {
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
    func flush() {
        deliver.withLock {
            guard let cb = lock.withLock({ onSamples }) else { return }
            let s = conv.flush()
            if !s.isEmpty { cb(s) }
        }
    }

    func stop() {
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

/// System audio through a Core Audio process tap (macOS 14.2+): a private global stereo tap attached to a private
/// aggregate device whose IOProc delivers what every app is playing. Requires the "System Audio Recording" TCC grant
/// (prompted on first use). Mic audio is mixed in. [MAC-VERIFY]
@available(macOS 14.2, *)
final class CoreAudioTapSource: QrtzHelperCore.AudioSource, @unchecked Sendable {
    let name = "core-audio-tap"
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var procID: AudioDeviceIOProcID?
    private let queue = DispatchQueue(label: "qrtz.tap", qos: .userInteractive)
    private let aligner = DSP.Aligner()
    /// The meeting's view of the shared mic tap (MicHub, H6): dictation can read the same device concurrently.
    private let mic: MicSource
    init(mic: MicSource) { self.mic = mic }
    private let conv = MonoConverter()

    static func defaultOutputUID() throws -> String {
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

    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void) async throws {
        try await start(onBlock: onBlock, onDegraded: { _, _ in })
    }

    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void,
               onDegraded: @escaping @Sendable (RevocationDegradation, String) -> Void) async throws {
        let desc = CATapDescription(stereoGlobalTapButExcludeProcesses: [])
        desc.uuid = UUID()
        desc.muteBehavior = .unmuted
        desc.isPrivate = true
        desc.name = "qrtz meeting tap"
        var st = AudioHardwareCreateProcessTap(desc, &tapID)
        guard st == noErr else { throw AudioError.os("AudioHardwareCreateProcessTap", st) }

        let outputUID = try Self.defaultOutputUID()
        let agg: [String: Any] = [
            kAudioAggregateDeviceNameKey: "qrtz-tap",
            kAudioAggregateDeviceUIDKey: "ai.qrtz.tap.\(UUID().uuidString)",
            kAudioAggregateDeviceMainSubDeviceKey: outputUID,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: outputUID]],
            kAudioAggregateDeviceTapListKey: [[kAudioSubTapDriftCompensationKey: true, kAudioSubTapUIDKey: desc.uuid.uuidString]],
        ]
        st = AudioHardwareCreateAggregateDevice(agg as CFDictionary, &aggregateID)
        guard st == noErr else { cleanup(); throw AudioError.os("AudioHardwareCreateAggregateDevice", st) }

        var asbd = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        var addr = AudioObjectPropertyAddress(mSelector: kAudioTapPropertyFormat, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        st = AudioObjectGetPropertyData(tapID, &addr, 0, nil, &size, &asbd)
        guard st == noErr, let format = AVAudioFormat(streamDescription: &asbd) else { cleanup(); throw AudioError.os("tap format", st) }

        let aligner = self.aligner, conv = self.conv
        st = AudioDeviceCreateIOProcIDWithBlock(&procID, aggregateID, queue) { _, inInput, _, _, _ in
            guard let buf = AVAudioPCMBuffer(pcmFormat: format, bufferListNoCopy: inInput, deallocator: nil) else { return }
            for b in aligner.pushSystem(conv.convert(buf)) { onBlock(b) }
        }
        guard st == noErr else { cleanup(); throw AudioError.os("AudioDeviceCreateIOProcIDWithBlock", st) }
        st = AudioDeviceStart(aggregateID, procID)
        guard st == noErr else { cleanup(); throw AudioError.os("AudioDeviceStart", st) }

        do {
            try mic.start(onSamples: { [queue] s in queue.async { for b in aligner.pushMic(s) { onBlock(b) } } })
        } catch {
            // N35: the source keeps recording system audio; the controller sends capture.degraded{no-mic}.
            onDegraded(.noMic, "mic unavailable, recording system audio only: \(error)")
        }
    }

    func stop() async {
        mic.stop()
        cleanup()
    }

    private func cleanup() {
        if aggregateID != kAudioObjectUnknown {
            if let procID { AudioDeviceStop(aggregateID, procID); AudioDeviceDestroyIOProcID(aggregateID, procID) }
            AudioHardwareDestroyAggregateDevice(aggregateID)
        }
        if tapID != kAudioObjectUnknown { AudioHardwareDestroyProcessTap(tapID) }
        procID = nil
        aggregateID = AudioObjectID(kAudioObjectUnknown)
        tapID = AudioObjectID(kAudioObjectUnknown)
    }
}

/// N143: what the Mac plays, for echo's gate while dictating without a meeting. A private global process tap on the
/// default output, system audio only, excluding the helper's own process (N152; the helper plays nothing today, and
/// the qrtz app is deliberately not excluded: its meeting playback is echo too); created per dictation and destroyed after it. Started
/// only with the screen & system audio recording grant already given (the controller checks), so it never prompts.
@available(macOS 14.2, *)
final class MacDictationTap: SystemAudioTap, @unchecked Sendable {
    let name = "dictation-tap"
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var procID: AudioDeviceIOProcID?
    private let queue = DispatchQueue(label: "qrtz.dictation-tap", qos: .userInteractive)
    private let conv = MonoConverter()

    private static func prop<T>(_ obj: AudioObjectID, _ sel: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope,
                                _ initial: T) -> T? {
        var v = initial
        var size = UInt32(MemoryLayout<T>.size)
        var addr = AudioObjectPropertyAddress(mSelector: sel, mScope: scope, mElement: kAudioObjectPropertyElementMain)
        return AudioObjectGetPropertyData(obj, &addr, 0, nil, &size, &v) == noErr ? v : nil
    }

    /// Speakers (built-in speakers, a display, HDMI, AirPlay), not headphones (the built-in jack's headphones,
    /// Bluetooth, USB), and some app is playing on the device right now.
    func echoLikely() -> Bool {
        let sys = AudioObjectID(kAudioObjectSystemObject)
        guard let dev = Self.prop(sys, kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal,
                                  AudioObjectID(kAudioObjectUnknown)), dev != kAudioObjectUnknown else { return false }
        let running: UInt32 = Self.prop(dev, kAudioDevicePropertyDeviceIsRunningSomewhere, kAudioObjectPropertyScopeGlobal, UInt32(0)) ?? 0
        guard running != 0 else { return false }
        let transport: UInt32 = Self.prop(dev, kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal, UInt32(0)) ?? 0
        switch transport {
        case kAudioDeviceTransportTypeBuiltIn:
            // The built-in output is speakers or headphones on the jack: 'ispk' vs 'hdpn'.
            let source: UInt32 = Self.prop(dev, kAudioDevicePropertyDataSource, kAudioDevicePropertyScopeOutput, UInt32(0)) ?? 0
            return source != 0x6864_706E  // 'hdpn'
        case kAudioDeviceTransportTypeHDMI, kAudioDeviceTransportTypeDisplayPort, kAudioDeviceTransportTypeAirPlay,
             kAudioDeviceTransportTypeThunderbolt:
            return true
        default:
            return false  // Bluetooth, USB, aggregate, virtual: most likely headphones or a headset
        }
    }

    /// The helper's own Core Audio process object (nil until it has one: a process that never touched audio).
    static func ownProcessObject() -> AudioObjectID? {
        var pid = getpid()
        var obj = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
                                              mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        let st = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, UInt32(MemoryLayout<pid_t>.size), &pid, &size, &obj)
        return st == noErr && obj != kAudioObjectUnknown ? obj : nil
    }

    func start(onSamples: @escaping @Sendable ([Float]) -> Void) async throws {
        let desc = CATapDescription(stereoGlobalTapButExcludeProcesses: Self.ownProcessObject().map { [$0] } ?? [])
        desc.uuid = UUID()
        desc.muteBehavior = .unmuted
        desc.isPrivate = true
        desc.name = "qrtz echo tap"
        var st = AudioHardwareCreateProcessTap(desc, &tapID)
        guard st == noErr else { throw AudioError.os("AudioHardwareCreateProcessTap", st) }
        let outputUID: String
        do { outputUID = try CoreAudioTapSource.defaultOutputUID() } catch { cleanup(); throw error }
        let agg: [String: Any] = [
            kAudioAggregateDeviceNameKey: "qrtz-echo-tap",
            kAudioAggregateDeviceUIDKey: "ai.qrtz.echo-tap.\(UUID().uuidString)",
            kAudioAggregateDeviceMainSubDeviceKey: outputUID,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: outputUID]],
            kAudioAggregateDeviceTapListKey: [[kAudioSubTapDriftCompensationKey: true, kAudioSubTapUIDKey: desc.uuid.uuidString]],
        ]
        st = AudioHardwareCreateAggregateDevice(agg as CFDictionary, &aggregateID)
        guard st == noErr else { cleanup(); throw AudioError.os("AudioHardwareCreateAggregateDevice", st) }
        var asbd = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        var addr = AudioObjectPropertyAddress(mSelector: kAudioTapPropertyFormat, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        st = AudioObjectGetPropertyData(tapID, &addr, 0, nil, &size, &asbd)
        guard st == noErr, let format = AVAudioFormat(streamDescription: &asbd) else { cleanup(); throw AudioError.os("tap format", st) }
        let conv = self.conv
        st = AudioDeviceCreateIOProcIDWithBlock(&procID, aggregateID, queue) { _, inInput, _, _, _ in
            guard let buf = AVAudioPCMBuffer(pcmFormat: format, bufferListNoCopy: inInput, deallocator: nil) else { return }
            let s = conv.convert(buf)
            if !s.isEmpty { onSamples(s) }
        }
        guard st == noErr else { cleanup(); throw AudioError.os("AudioDeviceCreateIOProcIDWithBlock", st) }
        st = AudioDeviceStart(aggregateID, procID)
        guard st == noErr else { cleanup(); throw AudioError.os("AudioDeviceStart", st) }
    }

    func stop() async { cleanup() }

    private func cleanup() {
        if aggregateID != kAudioObjectUnknown {
            if let procID { AudioDeviceStop(aggregateID, procID); AudioDeviceDestroyIOProcID(aggregateID, procID) }
            AudioHardwareDestroyAggregateDevice(aggregateID)
        }
        if tapID != kAudioObjectUnknown { AudioHardwareDestroyProcessTap(tapID) }
        procID = nil
        aggregateID = AudioObjectID(kAudioObjectUnknown)
        tapID = AudioObjectID(kAudioObjectUnknown)
    }
}

/// Echo cancellation for the mic using the system tap as far-end reference: FluidAudio LocalVQE (v1.3, 16 ms
/// streaming hop, CPU), from the onboarding-downloaded model only (ADR 0079). Any load/inference error makes
/// MicPipeline fall back to the AX/energy echo gate. Set QRTZ_AEC=0 to disable.
/// [MAC-VERIFY: reference/mic clock alignment on target devices]
final class LocalVqeEnhancer: MicEnhancer, @unchecked Sendable {
    let name = "localvqe-v1.3"
    private var stream: LocalVqeStream?

    func prepare() async throws {
        if ProcessInfo.processInfo.environment["QRTZ_AEC"] == "0" { throw AudioError("disabled by QRTZ_AEC=0") }
        if stream != nil { return }
        // ADR 0078 / ADR 0079: only the verified local file from onboarding; never FluidAudio's first-use download.
        // Missing = the echo gate does the job (MicPipeline falls back on a throw).
        let dir = try LocalModels.dir(LocalModels.echo)
        let vqe = try LocalVqeManager(config: LocalVqeConfig(chunk: .realtime16ms), modelDirectory: dir)
        stream = try await vqe.makeStream()
    }

    func enhance(mic: [Float], reference: [Float]) async throws -> [Float] {
        guard let stream else { throw AudioError("LocalVQE not prepared") }
        return try await stream.enhance(mic: mic, reference: reference)
    }

    func reset() async { try? await stream?.reset() }
}

/// Fallback: ScreenCaptureKit audio-only stream of the main display (macOS 13+; needs Screen Recording). Mic mixed in.
final class ScreenCaptureKitSource: NSObject, QrtzHelperCore.AudioSource, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    let name = "screencapturekit"
    private var stream: SCStream?
    private let queue = DispatchQueue(label: "qrtz.sck", qos: .userInteractive)
    private let aligner = DSP.Aligner()
    /// The meeting's view of the shared mic tap (MicHub, H6): dictation can read the same device concurrently.
    private let mic: MicSource
    init(mic: MicSource) { self.mic = mic }
    private let conv = MonoConverter()
    private var onBlock: (@Sendable (CaptureBlock) -> Void)?

    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void) async throws {
        try await start(onBlock: onBlock, onDegraded: { _, _ in })
    }

    func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void,
               onDegraded: @escaping @Sendable (RevocationDegradation, String) -> Void) async throws {
        self.onBlock = onBlock
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        guard let display = content.displays.first else { throw AudioError("no display for ScreenCaptureKit") }
        let filter = SCContentFilter(display: display, excludingWindows: [])
        let cfg = SCStreamConfiguration()
        cfg.capturesAudio = true
        cfg.excludesCurrentProcessAudio = true
        cfg.sampleRate = 48_000
        cfg.channelCount = 1
        // Video can't be disabled; make it as cheap as possible.
        cfg.width = 2
        cfg.height = 2
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        let s = SCStream(filter: filter, configuration: cfg, delegate: self)
        try s.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)
        try await s.startCapture()
        stream = s
        let aligner = self.aligner
        do {
            try mic.start(onSamples: { [queue] x in queue.async { for b in aligner.pushMic(x) { onBlock(b) } } })
        } catch {
            // N35: the source keeps recording system audio; the controller sends capture.degraded{no-mic}.
            onDegraded(.noMic, "mic unavailable, recording system audio only: \(error)")
        }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, sb.isValid, let onBlock else { return }
        try? sb.withAudioBufferList { abl, _ in
            guard let desc = sb.formatDescription?.audioStreamBasicDescription else { return }
            var asbd = desc
            guard let fmt = AVAudioFormat(streamDescription: &asbd),
                  let buf = AVAudioPCMBuffer(pcmFormat: fmt, bufferListNoCopy: abl.unsafePointer, deallocator: nil)
            else { return }
            for b in aligner.pushSystem(conv.convert(buf)) { onBlock(b) }
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) { logErr("ScreenCaptureKit stopped: \(error)") }

    func stop() async {
        mic.stop()
        try? await stream?.stopCapture()
        stream = nil
    }
}
#endif
