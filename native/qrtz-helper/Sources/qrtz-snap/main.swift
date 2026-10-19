import Foundation
import QrtzGateProbe
import QrtzSnap
#if os(macOS)
import AppKit
#endif

// qrtz-snap: qrtz snap's extension helper (ADR 0159), Spectacle's window snapping as a qrtz module. A plain
// executable inside the installed snap extension, spawned by qrtz, so it moves windows under qrtz's own Accessibility
// grant (ADR 0154). It speaks `snap.*` JSON lines on stdio; its first line is a hello.
// Usage: qrtz-snap --stdio | --version | --qrtz-gate-probe <ax|mic|socket|ae|sysaudio>

let snapAppVersion = "0.1.0"
GateProbe.runIfRequested(module: "snap")
if CommandLine.arguments.contains("--version") { print(snapAppVersion); exit(0) }

signal(SIGPIPE, SIG_IGN)

final class SnapLineWriter: @unchecked Sendable {
    private let lock = NSLock()
    func write(_ s: String) {
        lock.lock(); defer { lock.unlock() }
        FileHandle.standardOutput.write(Data((s + "\n").utf8))
    }
}

let out = SnapLineWriter()
out.write(SnapLines.hello(version: snapAppVersion))

#if os(macOS)
// Carbon hotkeys and the status item need the AppKit run loop; an accessory app has no Dock icon and no menu bar.
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let backend = MacSnapBackend(send: { out.write($0) })
#else
let backend = UnsupportedSnapBackend()
#endif
let server = SnapServer(backend: backend, write: { out.write($0) })

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
            // Every request runs on the main thread (AX, Carbon and AppKit all want it).
            var keepGoing = true
            DispatchQueue.main.sync { keepGoing = server.handle(line) }
            if !keepGoing { exit(0) }
        }
    }
    // stdin closed: the host went away.
    exit(0)
}
reader.start()

#if os(macOS)
app.run()
#else
RunLoop.main.run()
#endif
