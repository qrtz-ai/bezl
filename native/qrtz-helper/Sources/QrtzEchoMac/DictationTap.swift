#if os(macOS)
import AVFoundation
import CoreAudio
import Foundation
import QrtzEcho
import QrtzMacShared
import QrtzShared

// ADR 0146 M2.5: split out of qrtz-helper/mac/Audio.swift (owner: echo: the dictation gate's system tap).

/// N143: what the Mac plays, for echo's gate while dictating without a meeting. A private global process tap on the
/// default output, system audio only, excluding the helper's own process (N152; the helper plays nothing today, and
/// the qrtz app is deliberately not excluded: its meeting playback is echo too); created per dictation and destroyed after it. Started
/// only with the screen & system audio recording grant already given (the controller checks), so it never prompts.
@available(macOS 14.2, *)
public final class MacDictationTap: SystemAudioTap, @unchecked Sendable {
    public let name = "dictation-tap"
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
    public func echoLikely() -> Bool {
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
    public static func ownProcessObject() -> AudioObjectID? {
        var pid = getpid()
        var obj = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
                                              mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        let st = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, UInt32(MemoryLayout<pid_t>.size), &pid, &size, &obj)
        return st == noErr && obj != kAudioObjectUnknown ? obj : nil
    }

    public func start(onSamples: @escaping @Sendable ([Float]) -> Void) async throws {
        let desc = CATapDescription(stereoGlobalTapButExcludeProcesses: Self.ownProcessObject().map { [$0] } ?? [])
        desc.uuid = UUID()
        desc.muteBehavior = .unmuted
        desc.isPrivate = true
        desc.name = "qrtz echo tap"
        var st = AudioHardwareCreateProcessTap(desc, &tapID)
        guard st == noErr else { throw AudioError.os("AudioHardwareCreateProcessTap", st) }
        let outputUID: String
        do { outputUID = try MacAudioDevice.defaultOutputUID() } catch { cleanup(); throw error }
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

    public func stop() async { cleanup() }

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
#endif
