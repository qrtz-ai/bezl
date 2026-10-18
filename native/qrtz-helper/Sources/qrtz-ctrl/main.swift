import Foundation
import QrtzCtrl
import QrtzGateProbe

// qrtz-ctrl: ctrl's own module app (ADR 0146 M1.3, ADR 0144). It links only QrtzCtrl and the shared gate probe, so
// its bundle (ai.qrtz.ctrl, ai.qrtz.ctrl.entitlements) holds ctrl's Accessibility and Automation grants and nothing
// else. It speaks ctrl's `ctrl.*` JSON lines on stdio exactly as qrtz-helper's ctrl queue does (ADR 0110); any other
// line is ignored with a note on stderr, since echo and nano lines belong to their own apps.
// Usage: qrtz-ctrl --stdio | --version | --qrtz-gate-probe <ax|mic|socket|ae>

let ctrlAppVersion = "0.1.0"
GateProbe.runIfRequested(module: "ctrl")
if CommandLine.arguments.contains("--version") { print(ctrlAppVersion); exit(0) }

signal(SIGPIPE, SIG_IGN)

final class LineWriter: @unchecked Sendable {
    private let lock = NSLock()
    func write(_ s: String) {
        lock.lock(); defer { lock.unlock() }
        FileHandle.standardOutput.write(Data((s + "\n").utf8))
    }
}

let out = LineWriter()
let ctrl = CtrlStdio(backend: makeCtrlBackend(), write: { out.write($0) })

let reader = Thread {
    var buf = Data()
    while true {
        let chunk = FileHandle.standardInput.availableData
        if chunk.isEmpty { break }
        buf.append(chunk)
        while let nl = buf.firstIndex(of: 0x0A) {
            let line = String(decoding: buf[buf.startIndex..<nl], as: UTF8.self)
            buf.removeSubrange(buf.startIndex...nl)
            if line.isEmpty { continue }
            if CtrlStdio.isCtrlLine(line) {
                ctrl.submit(line)
            } else {
                FileHandle.standardError.write(Data("qrtz-ctrl: ignored a non-ctrl line\n".utf8))
            }
        }
    }
    // stdin closed: the host went away. Finish the queued requests, then exit.
    ctrl.drain()
    exit(0)
}
reader.start()

// AX observers and Apple Events need a running main run loop.
RunLoop.main.run()
