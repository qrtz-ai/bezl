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

/// Downmixes any non-interleaved/interleaved float PCM buffer to mono and resamples to 16 kHz.
final class MonoConverter: @unchecked Sendable {
    private var resampler: DSP.Resampler?
    func convert(_ buf: AVAudioPCMBuffer) -> [Float] {
        let frames = Int(buf.frameLength)
        guard frames > 0 else { return [] }
        let sr = buf.format.sampleRate
        if resampler == nil || resampler!.from != sr { resampler = DSP.Resampler(from: sr, to: 16_000) }
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
        return resampler!.process(mono)
    }
}

/// The user's microphone via AVAudioEngine. Mixed with system audio so both sides of the call are captured.
final class MicCapture: @unchecked Sendable {
    private let engine = AVAudioEngine()
    private let conv = MonoConverter()
    func start(_ onSamples: @escaping @Sendable ([Float]) -> Void) throws {
        let input = engine.inputNode
        let fmt = input.outputFormat(forBus: 0)
        guard fmt.sampleRate > 0 else { throw AudioError("no microphone input device") }
        input.installTap(onBus: 0, bufferSize: 4096, format: fmt) { [conv] buf, _ in onSamples(conv.convert(buf)) }
        engine.prepare()
        try engine.start()
    }
    func stop() {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
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

    private static func defaultOutputUID() throws -> String {
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
