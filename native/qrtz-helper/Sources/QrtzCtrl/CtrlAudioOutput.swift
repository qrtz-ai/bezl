// ADR 0198: "switch the sound to my airpods" / "play sound through the macbook speakers". The act makes the one
// connected output device of a kind the Mac's sound output (Core Audio: `kAudioHardwarePropertyDefaultOutputDevice` on
// the system object, what picking it in the menu bar's Sound item does), in the helper's own process: no process
// spawned, no Apple Event, no key event, nothing played, no app told anything; the input device, the volume and the
// alert device stay. Only the kind comes over the wire (a closed set, Rust `AudioOutput`); the helper lists the devices
// and picks, so no device name, id or UID is ever sent, logged or matched against text. The measure is the current
// output's kind, read in this process. Nothing here needs the microphone. The names, the kind mapping and the pick live
// here (Foundation) so Linux tests see them; the Core Audio calls are in mac/CtrlAudioOutputMac.swift.
import Foundation

/// The kinds of sound output a task can name. Mirrors Rust `AudioOutput` (snake_case); anything else is a bad request.
public enum CtrlAudioKind: String, CaseIterable, Sendable {
    case builtIn = "built_in"
    case bluetooth
    case usb
    case display

    /// The code `audio_output` reads back for an output of this kind (Rust `AudioOutput::code`, spaced by 10 so the
    /// host's ±1 level tolerance never confuses two kinds).
    public var code: Int {
        switch self {
        case .builtIn: return 10
        case .bluetooth: return 20
        case .usb: return 30
        case .display: return 40
        }
    }
}

public enum CtrlAudioOutput {
    public static let op = "set_audio_output"
    public static let check = "audio_output"

    /// Core Audio's transport types (`kAudioDeviceTransportType…`, four-char codes; a Mac test checks them against the
    /// SDK's constants).
    public static let transportBuiltIn: UInt32 = 0x626C_746E  // 'bltn'
    public static let transportBluetooth: UInt32 = 0x626C_7565  // 'blue'
    public static let transportBluetoothLE: UInt32 = 0x626C_6561  // 'blea'
    public static let transportUSB: UInt32 = 0x7573_6220  // 'usb '
    public static let transportHDMI: UInt32 = 0x6864_6D69  // 'hdmi'
    public static let transportDisplayPort: UInt32 = 0x6470_7274  // 'dprt'

    /// The kind of a device's transport; nil for every other one (AirPlay, aggregate, virtual, Thunderbolt, PCI,
    /// FireWire, unknown): those are never picked and read back as 0.
    public static func kind(transport: UInt32) -> CtrlAudioKind? {
        switch transport {
        case transportBuiltIn: return .builtIn
        case transportBluetooth, transportBluetoothLE: return .bluetooth
        case transportUSB: return .usb
        case transportHDMI, transportDisplayPort: return .display
        default: return nil
        }
    }

    /// One device as the Mac lists it: only what the pick needs (no name, no UID).
    public struct Device: Equatable, Sendable {
        public let id: UInt32
        public let transport: UInt32
        /// It has output streams (a microphone-only device has none).
        public let outputs: Bool
        /// `kAudioDevicePropertyIsHidden`: not shown in Sound settings, never picked.
        public let hidden: Bool
        public init(id: UInt32, transport: UInt32, outputs: Bool, hidden: Bool) {
            self.id = id; self.transport = transport; self.outputs = outputs; self.hidden = hidden
        }
    }

    /// The one device of `kind` to switch to: `audio_output.none` without one (AirPods not connected),
    /// `audio_output.not_one` with several (the task can't say which; never the first, never by name).
    public static func pick(_ devices: [Device], kind: CtrlAudioKind) throws -> UInt32 {
        let of = devices.filter { $0.outputs && !$0.hidden && Self.kind(transport: $0.transport) == kind }
        switch of.count {
        case 0: throw CtrlFailure("audio_output.none")
        case 1: return of[0].id
        default: throw CtrlFailure("audio_output.not_one")
        }
    }

    /// The measure's value for the current output's transport: its kind's code, 0 for another kind, nil with no
    /// output device.
    public static func value(transport: UInt32?) -> Int? {
        transport.map { kind(transport: $0)?.code ?? 0 }
    }

    /// `{"op": "set_audio_output", "kind": "bluetooth"}`: the kind's exact spelling only; nothing else is read from the
    /// request (a device name riding along is ignored, never used).
    static func parse(_ o: [String: Any]) -> CtrlRoute? {
        guard let k = o["kind"] as? String, let kind = CtrlAudioKind(rawValue: k) else { return nil }
        return .setAudioOutput(kind: kind)
    }
}
