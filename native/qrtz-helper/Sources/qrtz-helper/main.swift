import Foundation
import QrtzHelperCore

// qrtz-helper: stdio JSON-lines helper. stdout carries protocol messages only; diagnostics go to stderr.
// Usage: qrtz-helper --stdio | --version | --dump-ax (macOS: print the AX tree of detected meeting windows)

let helperVersion = "0.1.0"
let args = CommandLine.arguments

if args.contains("--version") { print(helperVersion); exit(0) }
#if os(macOS)
if args.contains("--dump-ax") { AXDump.run(); exit(0) }
#endif

signal(SIGPIPE, SIG_IGN)

final class StdoutWriter: @unchecked Sendable {
    private let lock = NSLock()
    func write(_ m: HelperMessage) {
        let line = m.line + "\n"
        lock.lock(); defer { lock.unlock() }
        FileHandle.standardOutput.write(Data(line.utf8))
    }
}

let out = StdoutWriter()
let epochMs: @Sendable () -> Double = { Date().timeIntervalSince1970 * 1000 }
let controller = HelperController(deps: makePlatformDeps(version: helperVersion), send: { out.write($0) }, now: epochMs)

// Host messages are handled strictly in order on one consumer task.
let (lines, linesIn) = AsyncStream<String>.makeStream()

Task {
    await controller.start()
    for await line in lines {
        switch HostMessage.parse(line: line) {
        case .success(let m):
            // N2: an abort is the host's way out of a wedged stop, so it must never queue behind it.
            if case .recordingAbort = m { Task { await controller.handle(m) }; continue }
            await controller.handle(m)
            if m == .shutdown { exit(0) }
        case .failure(let e):
            out.write(.log(level: .warn, msg: "ignored host line: \(e.description)"))
        }
    }
    // stdin closed: the host went away. Finish the WAV so audio is not lost, then exit.
    await controller.shutdown()
    exit(0)
}

Task {
    while true {
        try? await Task.sleep(nanoseconds: 5_000_000_000)
        await controller.heartbeat()
    }
}

let reader = Thread {
    var splitter = LineSplitter()
    while true {
        let chunk = FileHandle.standardInput.availableData
        if chunk.isEmpty { break }
        for l in splitter.feed(chunk) { linesIn.yield(l) }
    }
    linesIn.finish()
}
reader.start()

// AXObserver callbacks and AVAudioEngine need a running main run loop.
RunLoop.main.run()
