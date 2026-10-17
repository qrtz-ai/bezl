#if os(macOS)
import AVFoundation
import CoreAudio
import CoreMedia
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
final class CoreAudioTapSource: AudioSource, @unchecked Sendable {
    let name = "core-audio-tap"
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var procID: AudioDeviceIOProcID?
    private let queue = DispatchQueue(label: "qrtz.tap", qos: .userInteractive)
    private let mixer: DSP.Mixer
    init(context: MeetingContext) { mixer = DSP.Mixer(echo: EchoGate(context: context)) }
    private let mic = MicCapture()
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

    func start(onSamples: @escaping @Sendable ([Float]) -> Void) async throws {
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

        let mixer = self.mixer, conv = self.conv
        st = AudioDeviceCreateIOProcIDWithBlock(&procID, aggregateID, queue) { _, inInput, _, _, _ in
            guard let buf = AVAudioPCMBuffer(pcmFormat: format, bufferListNoCopy: inInput, deallocator: nil) else { return }
            let mixed = mixer.pushSystem(conv.convert(buf))
            if !mixed.isEmpty { onSamples(mixed) }
        }
        guard st == noErr else { cleanup(); throw AudioError.os("AudioDeviceCreateIOProcIDWithBlock", st) }
        st = AudioDeviceStart(aggregateID, procID)
        guard st == noErr else { cleanup(); throw AudioError.os("AudioDeviceStart", st) }

        do {
            try mic.start { [queue] s in queue.async { let m = mixer.pushMic(s); if !m.isEmpty { onSamples(m) } } }
        } catch {
            logErr("mic unavailable, recording system audio only: \(error)")
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

/// Fallback: ScreenCaptureKit audio-only stream of the main display (macOS 13+; needs Screen Recording). Mic mixed in.
final class ScreenCaptureKitSource: NSObject, AudioSource, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    let name = "screencapturekit"
    private var stream: SCStream?
    private let queue = DispatchQueue(label: "qrtz.sck", qos: .userInteractive)
    private let mixer: DSP.Mixer
    init(context: MeetingContext) {
        mixer = DSP.Mixer(echo: EchoGate(context: context))
        super.init()
    }
    private let mic = MicCapture()
    private let conv = MonoConverter()
    private var onSamples: (@Sendable ([Float]) -> Void)?

    func start(onSamples: @escaping @Sendable ([Float]) -> Void) async throws {
        self.onSamples = onSamples
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
        let mixer = self.mixer
        do {
            try mic.start { [queue] x in queue.async { let m = mixer.pushMic(x); if !m.isEmpty { onSamples(m) } } }
        } catch {
            logErr("mic unavailable, recording system audio only: \(error)")
        }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, sb.isValid, let onSamples else { return }
        try? sb.withAudioBufferList { abl, _ in
            guard let desc = sb.formatDescription?.audioStreamBasicDescription else { return }
            var asbd = desc
            guard let fmt = AVAudioFormat(streamDescription: &asbd),
                  let buf = AVAudioPCMBuffer(pcmFormat: fmt, bufferListNoCopy: abl.unsafePointer, deallocator: nil)
            else { return }
            let m = mixer.pushSystem(conv.convert(buf))
            if !m.isEmpty { onSamples(m) }
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
