#if os(macOS)
import AVFoundation
import CoreAudio
import CoreMedia
import FluidAudio
import Foundation
import QrtzMacShared
import QrtzNano
import QrtzShared
import ScreenCaptureKit

// ADR 0146 M2.5: split out of qrtz-helper/mac/Audio.swift (owner: nano: the meeting's capture sources and LocalVQE).

/// System audio through a Core Audio process tap (macOS 14.2+): a private global stereo tap attached to a private
/// aggregate device whose IOProc delivers what every app is playing. Requires the "System Audio Recording" TCC grant
/// (prompted on first use). Mic audio is mixed in. [MAC-VERIFY]
@available(macOS 14.2, *)
public final class CoreAudioTapSource: QrtzNano.AudioSource, @unchecked Sendable {
    public let name = "core-audio-tap"
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var procID: AudioDeviceIOProcID?
    private let queue = DispatchQueue(label: "qrtz.tap", qos: .userInteractive)
    private let aligner = DSP.Aligner()
    /// The meeting's view of the shared mic tap (MicHub, H6): dictation can read the same device concurrently.
    private let mic: MicSource
    public init(mic: MicSource) { self.mic = mic }
    private let conv = MonoConverter()

    public func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void) async throws {
        try await start(onBlock: onBlock, onDegraded: { _, _ in })
    }

    public func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void,
               onDegraded: @escaping @Sendable (RevocationDegradation, String) -> Void) async throws {
        let desc = CATapDescription(stereoGlobalTapButExcludeProcesses: [])
        desc.uuid = UUID()
        desc.muteBehavior = .unmuted
        desc.isPrivate = true
        desc.name = "qrtz meeting tap"
        var st = AudioHardwareCreateProcessTap(desc, &tapID)
        guard st == noErr else { throw AudioError.os("AudioHardwareCreateProcessTap", st) }

        let outputUID = try MacAudioDevice.defaultOutputUID()
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

    public func stop() async {
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
public final class LocalVqeEnhancer: MicEnhancer, @unchecked Sendable {
    public let name = "localvqe-v1.3"
    private var stream: LocalVqeStream?

    public func prepare() async throws {
        if ProcessInfo.processInfo.environment["QRTZ_AEC"] == "0" { throw AudioError("disabled by QRTZ_AEC=0") }
        if stream != nil { return }
        // ADR 0078 / ADR 0079: only the verified local file from onboarding; never FluidAudio's first-use download.
        // Missing = the echo gate does the job (MicPipeline falls back on a throw).
        let dir = try LocalModels.dir(LocalModels.echo)
        let vqe = try LocalVqeManager(config: LocalVqeConfig(chunk: .realtime16ms), modelDirectory: dir)
        stream = try await vqe.makeStream()
    }

    public func enhance(mic: [Float], reference: [Float]) async throws -> [Float] {
        guard let stream else { throw AudioError("LocalVQE not prepared") }
        return try await stream.enhance(mic: mic, reference: reference)
    }

    public func reset() async { try? await stream?.reset() }
}

/// Fallback: ScreenCaptureKit audio-only stream of the main display (macOS 13+; needs Screen Recording). Mic mixed in.
public final class ScreenCaptureKitSource: NSObject, QrtzNano.AudioSource, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    public let name = "screencapturekit"
    private var stream: SCStream?
    private let queue = DispatchQueue(label: "qrtz.sck", qos: .userInteractive)
    private let aligner = DSP.Aligner()
    /// The meeting's view of the shared mic tap (MicHub, H6): dictation can read the same device concurrently.
    private let mic: MicSource
    public init(mic: MicSource) { self.mic = mic }
    private let conv = MonoConverter()
    private var onBlock: (@Sendable (CaptureBlock) -> Void)?

    public func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void) async throws {
        try await start(onBlock: onBlock, onDegraded: { _, _ in })
    }

    public func start(onBlock: @escaping @Sendable (CaptureBlock) -> Void,
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

    public func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
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

    public func stream(_ stream: SCStream, didStopWithError error: Error) { logErr("ScreenCaptureKit stopped: \(error)") }

    public func stop() async {
        mic.stop()
        try? await stream?.stopCapture()
        stream = nil
    }
}
#endif
