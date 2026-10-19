#if os(macOS)
// ADR 0198: "switch the sound to my airpods", macOS side. The device list and the switch are Core Audio's own
// (`kAudioHardwarePropertyDevices`, `kAudioHardwarePropertyDefaultOutputDevice` on the system object), in the helper's
// own process: no process spawned, no Apple Event, no key event, nothing played. Only each device's transport type,
// output streams and hidden flag are read (never a name or UID); the input device, the volume and the alert device stay.
import CoreAudio
import Foundation

enum CtrlAudioOutputMac {
    private static func address(_ selector: AudioObjectPropertySelector,
                                _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    }

    private static func uint32(_ obj: AudioObjectID, _ selector: AudioObjectPropertySelector) -> UInt32? {
        var addr = address(selector)
        var v: UInt32 = 0
        var size = UInt32(MemoryLayout<UInt32>.size)
        return AudioObjectGetPropertyData(obj, &addr, 0, nil, &size, &v) == noErr ? v : nil
    }

    /// The Mac's audio devices, with only what the pick needs; nil if Core Audio can't list them.
    static func devices() -> [CtrlAudioOutput.Device]? {
        let sys = AudioObjectID(kAudioObjectSystemObject)
        var addr = address(kAudioHardwarePropertyDevices)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(sys, &addr, 0, nil, &size) == noErr else { return nil }
        var ids = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
        guard AudioObjectGetPropertyData(sys, &addr, 0, nil, &size, &ids) == noErr else { return nil }
        return ids.map { id in
            var streams = address(kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput)
            var bytes: UInt32 = 0
            let outputs = AudioObjectGetPropertyDataSize(id, &streams, 0, nil, &bytes) == noErr && bytes > 0
            return CtrlAudioOutput.Device(id: id, transport: uint32(id, kAudioDevicePropertyTransportType) ?? 0,
                                          outputs: outputs, hidden: (uint32(id, kAudioDevicePropertyIsHidden) ?? 0) != 0)
        }
    }

    /// Make the one device of `kind` the sound output (`audio_output.unreadable` / `.none` / `.not_one` /
    /// `.refused`).
    static func switchTo(_ kind: CtrlAudioKind) throws {
        guard let all = devices() else { throw CtrlFailure("audio_output.unreadable") }
        var id = try CtrlAudioOutput.pick(all, kind: kind)
        var addr = address(kAudioHardwarePropertyDefaultOutputDevice)
        let st = AudioObjectSetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil,
                                            UInt32(MemoryLayout<AudioObjectID>.size), &id)
        if st != noErr { throw CtrlFailure("audio_output.refused") }
    }

    /// The current sound output's transport type; nil with no output device.
    static func currentTransport() -> UInt32? {
        guard let dev = uint32(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice),
              dev != kAudioObjectUnknown else { return nil }
        return uint32(dev, kAudioDevicePropertyTransportType)
    }
}
#endif
