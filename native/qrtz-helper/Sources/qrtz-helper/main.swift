import Foundation
import QrtzHelperCore

// qrtz-helper: stdio JSON-lines helper. stdout carries protocol messages only; diagnostics go to stderr.
// Usage: qrtz-helper --stdio | --version | --dump-ax (macOS: print the AX tree of detected meeting windows)
//        | --bench --mic a.aiff[,b.aiff] [--system c.aiff] (macOS: ADR 0079 model measurements, see mac/Bench.swift)
//        | --echo-bench <clips dir> [--out f.json] (macOS: ADR 0099 dictation tuning bench, see mac/EchoBench.swift)

let helperVersion = "0.1.0"
let args = CommandLine.arguments

if args.contains("--version") { print(helperVersion); exit(0) }
#if os(macOS)
if args.contains("--dump-ax") { AXDump.run(); exit(0) }
if args.contains("--echo-bench") { EchoBenchCLI.run(args) }
if args.contains("--bench") { Bench.run(args) }
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
var platformDeps = makePlatformDeps(version: helperVersion)
// ADR 0046 §1 / N55: the speech socket the host inherited to us (speech.attach fds arrive on it).
platformDeps.speechFds = SocketFdReceiver.fromEnvironment()
let controller = HelperController(deps: platformDeps, send: { out.write($0) }, now: epochMs)

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
