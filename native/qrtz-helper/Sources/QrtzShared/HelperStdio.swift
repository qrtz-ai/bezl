import Foundation

// ADR 0146 M2.5: the stdio loop every helper app runs (qrtz-helper, qrtz-echo, qrtz-nano): stdout carries protocol
// lines only (diagnostics go to stderr), host lines are handled strictly in order on one consumer, a heartbeat every
// 5 s, and the app exits after `shutdown` or when stdin closes (the host went away).

/// Writes whole lines to stdout, one at a time.
public final class StdoutLines: @unchecked Sendable {
    private let lock = NSLock()
    public init() {}
    public func write(_ m: HelperMessage) { writeLine(m.line) }
    /// A line that is already encoded (ctrl's replies, ADR 0110).
    public func writeLine(_ s: String) {
        lock.lock(); defer { lock.unlock() }
        FileHandle.standardOutput.write(Data((s + "\n").utf8))
    }
}

/// Non-macOS build: speaks the protocol (so contract and integration tests can run an app) but has no TCC.
public struct UnsupportedPermissions: PermissionsProvider {
    public init() {}
    public func status() -> [OsPermission: PermissionState] {
        Dictionary(uniqueKeysWithValues: OsPermission.allCases.map { ($0, PermissionState.unsupported) })
    }
    public func request(_ p: OsPermission) async {}
}

public enum HelperStdio {
    /// Wall-clock ms, the clock every app's lines use.
    public static let epochMs: @Sendable () -> Double = { Date().timeIntervalSince1970 * 1000 }

    /// Runs a module app's host until the host says shutdown or stdin closes. Never returns.
    /// `mainLoop`: what drives the main thread (an AppKit app passes `NSApplication.shared.run()`; see `run(out:)`).
    public static func run(_ host: ModuleHost, out: StdoutLines, mainLoop: () -> Void = { RunLoop.main.run() }) -> Never {
        run(out: out, start: { await host.start() }, handle: { await host.handle($0) },
            heartbeat: { await host.heartbeat() }, shutdown: { await host.shutdown() }, mainLoop: mainLoop)
    }

    /// `divert`: lines another queue takes before the ordered consumer (ctrl's, ADR 0110). Never returns.
    public static func run(out: StdoutLines,
                           start: @escaping @Sendable () async -> Void,
                           handle: @escaping @Sendable (HostMessage) async -> Void,
                           heartbeat: @escaping @Sendable () async -> Void,
                           shutdown: @escaping @Sendable () async -> Void,
                           divert: (@Sendable (String) -> Bool)? = nil,
                           mainLoop: () -> Void = { RunLoop.main.run() }) -> Never {
        signal(SIGPIPE, SIG_IGN)
        let (lines, linesIn) = AsyncStream<String>.makeStream()
        Task {
            await start()
            for await line in lines {
                switch HostMessage.parse(line: line) {
                case .success(let m):
                    // N2: an abort is the host's way out of a wedged stop, so it must never queue behind it.
                    if case .recordingAbort = m { Task { await handle(m) }; continue }
                    await handle(m)
                    if m == .shutdown { exit(0) }
                case .failure(let e):
                    out.write(.log(level: .warn, msg: "ignored host line: \(e.description)"))
                }
            }
            // stdin closed: the host went away. Finish a recording's WAV so audio is not lost, then exit.
            await shutdown()
            exit(0)
        }
        Task {
            while true {
                try? await Task.sleep(nanoseconds: 5_000_000_000)
                await heartbeat()
            }
        }
        let reader = Thread {
            var splitter = LineSplitter()
            while true {
                let chunk = FileHandle.standardInput.availableData
                if chunk.isEmpty { break }
                for l in splitter.feed(chunk) {
                    if let divert, divert(l) { continue }
                    linesIn.yield(l)
                }
            }
            linesIn.finish()
        }
        reader.start()
        // AXObserver callbacks and AVAudioEngine need a running main run loop. An app with windows passes
        // NSApplication.run here instead, so AppKit also processes its events (screen changes update NSScreen.screens
        // and post didChangeScreenParameters only then); it spins the same main run loop and main queue.
        mainLoop()
        exit(0)
    }
}
