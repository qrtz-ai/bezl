// ADR 0144 / ADR 0154 gate probe mode for the extension helpers (ADR 0146 M1.3, M2.5). Every helper (qrtz-echo,
// qrtz-nano, qrtz-ctrl) runs `--qrtz-gate-probe <ax|mic|socket|ae|sysaudio>` before anything else. Extensions share
// qrtz's identity (ADR 0154): the host spawns a helper directly and TCC charges its requests to qrtz, so this probe
// shows what a helper inherits. scripts/modules/mac-gate.sh's gate (h) spawns a helper from the granted host
// (`probe host spawn <helper> -- --qrtz-gate-probe ax`) and expects qrtz's Accessibility grant; `mic` is its
// companion (the helper's status should match the host's). The result is one line in the stand-in probe's format
// (scripts/modules/gate-probe.swift), `probe <cmd> key=value ...`, written to stdout and to `--out <path>` (default
// <home>/qrtz-gate-probe.out, the container home when sandboxed).
//
//   --qrtz-gate-probe ax [--ask]            AXIsProcessTrusted (gate h: true from qrtz's grant when spawned by qrtz)
//   --qrtz-gate-probe mic [--open] [--ask]  microphone authorization; --open tries the default input (gate h, with
//                                           the helper's own device.audio-input entitlement, ADR 0154)
//   --qrtz-gate-probe socket [<port>]       TCP and UDP connect() to 127.0.0.1:<port> (default 9); loopback only (gate c)
//   --qrtz-gate-probe ae [--ask]            Automation of System Events: permission, then one read-only event
//                                           (diagnostic; gate e is retired by ADR 0154)
//   --qrtz-gate-probe sysaudio [--open] [--ask]
//                                           screen & system audio recording, the grant echo's speaker gate needs
//                                           (ADR 0150 N173; qrtz's, inherited); --open creates and destroys one
//                                           process tap, as a dictation does, only when the grant is already there
//
// It never prompts without --ask. Foundation-only; the Apple calls are behind `GateProbeBackend` (mac/ on macOS).
import Foundation

public let gateProbeFlag = "--qrtz-gate-probe"

public enum GateProbeCommand: String, CaseIterable, Sendable {
    case ax, mic, socket, ae, sysaudio
}

public struct GateProbeRequest: Equatable, Sendable {
    public var command: GateProbeCommand
    public var ask = false
    public var openMic = false
    public var port: UInt16 = 9
    public var out: String?

    public init(command: GateProbeCommand, ask: Bool = false, openMic: Bool = false, port: UInt16 = 9, out: String? = nil) {
        self.command = command
        self.ask = ask
        self.openMic = openMic
        self.port = port
        self.out = out
    }

    public enum Parse: Equatable, Sendable {
        /// No `--qrtz-gate-probe`: the app runs normally.
        case absent
        case probe(GateProbeRequest)
        /// The flag with a missing or unknown command, or a bad port: report it, never fall through to a normal run.
        case invalid(String)
    }

    /// Reads the probe flag from the full argv (argv[0] included or not; only what follows the flag matters).
    public static func parse(_ argv: [String]) -> Parse {
        guard let i = argv.firstIndex(of: gateProbeFlag) else { return .absent }
        var rest = Array(argv[(i + 1)...])
        guard let first = rest.first else { return .invalid("usage: \(gateProbeFlag) <ax|mic|socket|ae|sysaudio>") }
        guard let cmd = GateProbeCommand(rawValue: first) else { return .invalid("unknown probe: \(first)") }
        rest.removeFirst()
        var r = GateProbeRequest(command: cmd)
        var j = 0
        while j < rest.count {
            switch rest[j] {
            case "--ask": r.ask = true
            case "--open": r.openMic = true
            case "--out":
                guard j + 1 < rest.count else { return .invalid("--out needs a path") }
                r.out = rest[j + 1]
                j += 1
            case let a where cmd == .socket && a.first?.isNumber == true:
                guard let p = UInt16(a), p > 0 else { return .invalid("bad port: \(a)") }
                r.port = p
            case let a:
                return .invalid("unexpected argument: \(a)")
            }
            j += 1
        }
        return .probe(r)
    }
}

/// The Apple-side checks. `MacGateProbeBackend` on macOS; tests and Linux use their own.
public protocol GateProbeBackend {
    func axTrusted(ask: Bool) -> Bool
    /// `status`, plus `granted`, `opened`, `status_after` when `open` is set (the stand-in probe's keys).
    func mic(ask: Bool, open: Bool) -> [(String, String)]
    /// `permission` (an OSStatus), `sent` (ok, failed, no) and `ae_error` when an event failed.
    func automateSystemEvents(ask: Bool) -> [(String, String)]
    /// ADR 0150 N173: `screen` (CGPreflightScreenCaptureAccess, what echo's gate checks), `requested` and
    /// `screen_after` with `ask`, `tap` (ok, an OSStatus, or skipped without the grant) with `open`.
    func systemAudio(ask: Bool, open: Bool) -> [(String, String)]
}

public extension GateProbeBackend {
    func systemAudio(ask: Bool, open: Bool) -> [(String, String)] { [("screen", "unsupported")] }
}

/// Off macOS: there's no TCC, so every Apple check reports `unsupported`.
public struct UnsupportedGateProbeBackend: GateProbeBackend {
    public init() {}
    public func axTrusted(ask: Bool) -> Bool { false }
    public func mic(ask: Bool, open: Bool) -> [(String, String)] { [("status", "unsupported")] }
    public func automateSystemEvents(ask: Bool) -> [(String, String)] { [("permission", "unsupported"), ("sent", "no")] }
}

public enum GateProbe {
    /// One result line, without the newline. Values never contain spaces (the gate script splits on them).
    public static func line(_ cmd: String, _ fields: [(String, String)]) -> String {
        let f = fields.map { "\($0.0)=\($0.1.replacingOccurrences(of: " ", with: "_"))" }
        return (["probe", cmd] + f).joined(separator: " ")
    }

    /// The fields for one request. `module` is the app's module id, so a mixed-up bundle shows in the line.
    public static func fields(_ r: GateProbeRequest, module: String, bundleId: String, backend: GateProbeBackend) -> [(String, String)] {
        let head = [("bundle", bundleId), ("module", module)]
        switch r.command {
        case .ax:
            return head + [("trusted", String(backend.axTrusted(ask: r.ask)))]
        case .mic:
            return head + backend.mic(ask: r.ask, open: r.openMic)
        case .socket:
            return head + [
                ("tcp", loopbackConnect(port: r.port, stream: true)),
                ("udp", loopbackConnect(port: r.port, stream: false)),
            ]
        case .ae:
            return head + backend.automateSystemEvents(ask: r.ask)
        case .sysaudio:
            return head + backend.systemAudio(ask: r.ask, open: r.openMic)
        }
    }

    public static func defaultOut(home: String = NSHomeDirectory()) -> String {
        (home as NSString).appendingPathComponent("qrtz-gate-probe.out")
    }

    /// Runs the probe mode if argv asks for it: prints and stores the line, then exits 0 (2 for a bad request).
    /// Returns only when argv has no `--qrtz-gate-probe`. Call it first in every module app's main.
    public static func runIfRequested(
        _ argv: [String] = CommandLine.arguments, module: String,
        bundleId: String = Bundle.main.bundleIdentifier ?? "none",
        backend: GateProbeBackend = makeGateProbeBackend()
    ) {
        switch GateProbeRequest.parse(argv) {
        case .absent:
            return
        case .invalid(let why):
            let l = line("invalid", [("bundle", bundleId), ("module", module), ("error", why)]) + "\n"
            FileHandle.standardError.write(Data(l.utf8))
            exit(2)
        case .probe(let r):
            let l = line(r.command.rawValue, fields(r, module: module, bundleId: bundleId, backend: backend)) + "\n"
            FileHandle.standardOutput.write(Data(l.utf8))
            try? l.write(toFile: r.out ?? defaultOut(), atomically: true, encoding: .utf8)
            exit(0)
        }
    }

    /// errno names the gates read: EPERM/EACCES = the sandbox refused the call, ECONNREFUSED = it reached the stack.
    static func errnoName(_ e: Int32) -> String {
        switch e {
        case EPERM: return "EPERM"
        case EACCES: return "EACCES"
        case ECONNREFUSED: return "ECONNREFUSED"
        case ENETUNREACH: return "ENETUNREACH"
        case 0: return "connected"
        default: return "errno\(e)"
        }
    }

    /// One connect() to 127.0.0.1:<port>. Nothing leaves the Mac; the answer is whether the sandbox let it try.
    public static func loopbackConnect(port: UInt16, stream: Bool) -> String {
        let fd = socket(AF_INET, sockType(stream: stream), 0)
        if fd < 0 { return "socket-" + errnoName(errno) }
        defer { close(fd) }
        var a = sockaddr_in()
        #if canImport(Darwin)
        a.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        #endif
        a.sin_family = sa_family_t(AF_INET)
        a.sin_port = port.bigEndian
        a.sin_addr.s_addr = UInt32(0x7f00_0001).bigEndian
        let r = withUnsafePointer(to: &a) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        return errnoName(r == 0 ? 0 : errno)
    }

    #if canImport(Darwin)
    static func sockType(stream: Bool) -> Int32 { stream ? SOCK_STREAM : SOCK_DGRAM }
    #else
    static func sockType(stream: Bool) -> Int32 { Int32(stream ? SOCK_STREAM.rawValue : SOCK_DGRAM.rawValue) }
    #endif
}

/// The backend this platform has.
public func makeGateProbeBackend() -> GateProbeBackend {
    #if os(macOS)
    return MacGateProbeBackend()
    #else
    return UnsupportedGateProbeBackend()
    #endif
}
