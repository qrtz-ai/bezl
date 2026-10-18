// ADR 0144 / ADR 0154 gate probe: one tiny Foundation-only program that scripts/modules/mac-gate.sh compiles with
// plain swiftc (no SwiftPM, no FluidAudio, no network) and wraps in stand-in bundles (ai.qrtz.gate.host-probe for
// the qrtz host, ai.qrtz.gate.inference-probe for a sandboxed inference XPC service, ai.qrtz.gate.ctrl-probe for the
// signing-identity check) launched through LaunchServices (`open`), and in a bare extension helper binary that the
// host spawns, as qrtz spawns a downloaded extension's helper. It never prompts unless given --ask. Every result is
// one line `probe <cmd> key=value ...` written to --out <path> (or <home>/qrtz-gate-probe.out, the container home
// when sandboxed) and stdout.
//
// Commands:
//   ax [--ask]            AXIsProcessTrusted (gates d and h)
//   mic [--open] [--ask]  microphone authorization; --open tries to open the default input (gate h, info)
//   socket <port>         TCP and UDP connect() to 127.0.0.1:<port>; loopback only, nothing leaves the Mac (gate c)
//   ae [--ask]            Automation of System Events: permission, then one read-only Apple Event (kept for manual
//                         checks; its gate (e) is retired by ADR 0154)
//   xpc <mach-name>       one message to a mach service; reports replied or refused (gate f)
//   spawn <path> -- <args..>  run <path> as a child of this process (posix_spawn through Process, as the qrtz host runs
//                         an extension helper) and report its exit status; the child writes its own --out (gate h)
//   info                  bundle id, pid, build
// Linux: scripts/modules/probe-typecheck.sh typechecks this file against scripts/modules/probe-stubs.swift.
import Foundation
#if os(macOS)
import ApplicationServices
import AVFoundation
import CoreServices
import XPC
#endif

var args = Array(CommandLine.arguments.dropFirst())
/// Everything after `--` goes to the spawned child untouched (spawn), so its --out and --ask aren't read here.
var passthrough: [String] = []
if let i = args.firstIndex(of: "--") {
    passthrough = Array(args[(i + 1)...])
    args.removeSubrange(i...)
}
func flag(_ f: String) -> Bool {
    if let i = args.firstIndex(of: f) { args.remove(at: i); return true }
    return false
}
func option(_ f: String) -> String? {
    guard let i = args.firstIndex(of: f), i + 1 < args.count else { return nil }
    let v = args[i + 1]
    args.removeSubrange(i...(i + 1))
    return v
}
let outPath = option("--out") ?? (NSHomeDirectory() as NSString).appendingPathComponent("qrtz-gate-probe.out")
let ask = flag("--ask")
let openMic = flag("--open")
let cmd = args.first ?? "info"

func emit(_ fields: [(String, String)]) -> Never {
    let line = "probe \(cmd) " + fields.map { "\($0.0)=\($0.1)" }.joined(separator: " ") + "\n"
    FileHandle.standardOutput.write(line.data(using: .utf8)!)
    try? line.write(toFile: outPath, atomically: true, encoding: .utf8)
    exit(0)
}

let bundleId = Bundle.main.bundleIdentifier ?? "none"

/// errno names the gates read: EPERM/EACCES = the sandbox refused the call, ECONNREFUSED = it reached the stack.
func errnoName(_ e: Int32) -> String {
    switch e {
    case EPERM: return "EPERM"
    case EACCES: return "EACCES"
    case ECONNREFUSED: return "ECONNREFUSED"
    case ENETUNREACH: return "ENETUNREACH"
    case 0: return "connected"
    default: return "errno\(e)"
    }
}

func loopbackConnect(port: UInt16, stream: Bool) -> String {
    let fd = socket(AF_INET, probeSockType(stream: stream), 0)
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
func probeSockType(stream: Bool) -> Int32 { stream ? SOCK_STREAM : SOCK_DGRAM }
#else
func probeSockType(stream: Bool) -> Int32 { Int32(stream ? SOCK_STREAM.rawValue : SOCK_DGRAM.rawValue) }
#endif

#if os(macOS)
func micStatus() -> String {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized: return "authorized"
    case .denied: return "denied"
    case .restricted: return "restricted"
    case .notDetermined: return "notDetermined"
    @unknown default: return "unknown"
    }
}

/// One read-only Apple Event to System Events (its name), only after the permission check allows it.
func automateSystemEvents() -> [(String, String)] {
    let target = "com.apple.systemevents"
    var desc = AEDesc()
    let bytes = Array(target.utf8)
    _ = bytes.withUnsafeBufferPointer {
        AECreateDesc(DescType(typeApplicationBundleID), $0.baseAddress, $0.count, &desc)
    }
    defer { AEDisposeDesc(&desc) }
    let perm = AEDeterminePermissionToAutomateTarget(&desc, AEEventClass(typeWildCard), AEEventID(typeWildCard), ask)
    var out: [(String, String)] = [("permission", String(perm))]
    // -600 = System Events isn't running: the gate script starts it (open -g -b) and asks again.
    guard perm == noErr else { out.append(("sent", "no")); return out }
    var err: NSDictionary?
    let r = NSAppleScript(source: "tell application id \"\(target)\" to get name")?.executeAndReturnError(&err)
    out.append(("sent", r == nil ? "failed" : "ok"))
    if let n = err?[NSAppleScript.errorNumber] { out.append(("ae_error", "\(n)")) }
    return out
}

func ping(_ name: String) -> String {
    let c = xpc_connection_create_mach_service(name, nil, 0)
    xpc_connection_set_event_handler(c) { _ in }
    xpc_connection_resume(c)
    let msg = xpc_dictionary_create(nil, nil, 0)
    xpc_dictionary_set_string(msg, "op", "ping")
    let reply = xpc_connection_send_message_with_reply_sync(c, msg)
    defer { xpc_connection_cancel(c) }
    if xpc_get_type(reply) == XPC_TYPE_ERROR {
        return xpc_equal(reply, XPC_ERROR_CONNECTION_INVALID) ? "refused-invalid" : "refused-interrupted"
    }
    return "replied"
}
#endif

switch cmd {
case "info":
    emit([("bundle", bundleId), ("pid", String(getpid())), ("build", probeBuild)])
case "socket":
    let port = UInt16(args.count > 1 ? args[1] : "9") ?? 9
    emit([
        ("bundle", bundleId), ("tcp", loopbackConnect(port: port, stream: true)),
        ("udp", loopbackConnect(port: port, stream: false)),
    ])
#if os(macOS)
case "ax":
    let trusted = ask
        ? AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as NSDictionary as CFDictionary)
        : AXIsProcessTrusted()
    emit([("bundle", bundleId), ("trusted", String(trusted)), ("build", probeBuild)])
case "mic":
    var fields = [("bundle", bundleId), ("status", micStatus())]
    if openMic {
        final class Answer: @unchecked Sendable { var granted = false }
        let answer = Answer()
        answer.granted = AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
        if !answer.granted && ask {
            let s = DispatchSemaphore(value: 0)
            AVCaptureDevice.requestAccess(for: .audio) { answer.granted = $0; s.signal() }
            _ = s.wait(timeout: .now() + 60)
        }
        let granted = answer.granted
        var opened = "no-device"
        if let d = AVCaptureDevice.default(for: .audio) {
            opened = granted && (try? AVCaptureDeviceInput(device: d)) != nil ? "yes" : "no"
        }
        fields += [("granted", String(granted)), ("opened", opened), ("status_after", micStatus())]
    }
    emit(fields)
case "ae":
    emit([("bundle", bundleId)] + automateSystemEvents())
case "xpc":
    guard args.count > 1 else { emit([("error", "usage: xpc <mach-name>")]) }
    emit([("bundle", bundleId), ("service", args[1]), ("result", ping(args[1]))])
#endif
case "spawn":
    // A plain child, no LaunchServices and no responsibility disclaim, so TCC attributes it to this process (ADR 0154).
    guard args.count > 1 else { emit([("error", "usage: spawn <path> -- <args..>")]) }
    let child = Process()
    child.executableURL = URL(fileURLWithPath: args[1])
    child.arguments = passthrough
    do {
        try child.run()
        child.waitUntilExit()
        emit([("bundle", bundleId), ("pid", String(getpid())), ("child_exit", String(child.terminationStatus))])
    } catch {
        emit([("bundle", bundleId), ("child_exit", "spawn-failed")])
    }
default:
    emit([("error", "unknown or not on this platform: \(cmd)")])
}
