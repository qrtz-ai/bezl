import Foundation
import QrtzCtrl
import QrtzHelperCore
#if os(macOS)
import QrtzNanoMac
#endif

// qrtz-helper: the monolith helper (ADR 0146: echo and nano in one app until the module apps take over), stdio
// JSON-lines (HelperStdio). stdout carries protocol messages only; diagnostics go to stderr.
// Usage: qrtz-helper --stdio | --version | --dump-ax (macOS: print the AX tree of detected meeting windows)
//        | (QRTZ_DIAG builds only, QRT-299:) --bench --mic a.aiff[,b.aiff] [--system c.aiff] (macOS: ADR 0079 model measurements, see mac/Bench.swift)
//        | --echo-bench <clips dir> [--out f.json] (macOS: ADR 0099 dictation tuning bench, see mac/EchoBench.swift)
//        | --echo-warmup <clip> --expect "<words>" --press-at 0|<ms>|ready (macOS: QRT-270 CI, see mac/EchoWarmup.swift)

let helperVersion = "0.1.0"
let args = CommandLine.arguments

if args.contains("--version") { print(helperVersion); exit(0) }
#if os(macOS)
if args.contains("--dump-ax") { AXDump.run(); exit(0) }
#endif
#if os(macOS) && QRTZ_DIAG
// QRT-299: the diagnostic CLIs exist only when built with -DQRTZ_DIAG, which only scripts/echo-warmup-ci.sh sets.
// Release, build-packages and the bundle never set it, so a shipped helper carries neither the code nor the flags.
if args.contains("--echo-bench") { EchoBenchCLI.run(args) }
if args.contains("--echo-warmup") { EchoWarmupCLI.run(args) }
if args.contains("--bench") { Bench.run(args) }
#else
// QRT-299: without QRTZ_DIAG the helper takes only the options above (and --stdio); anything else is refused, so the
// diagnostic flags fail as unknown instead of falling through to stdio. scripts/release-flags-check.sh tests this.
if let bad = args.dropFirst().first(where: { $0.hasPrefix("-") && !["--stdio", "--version", "--dump-ax"].contains($0) }) {
    FileHandle.standardError.write(Data("qrtz-helper: unknown option \(bad)\n".utf8))
    exit(64)
}
#endif

let out = StdoutLines()
var platformDeps = makePlatformDeps(version: helperVersion)
// ADR 0046 §1 / N55: the speech socket the host inherited to us (speech.attach fds arrive on it).
platformDeps.speechFds = SocketFdReceiver.fromEnvironment()
let controller = HelperController(deps: platformDeps, send: { out.write($0) }, now: HelperStdio.epochMs)

// ADR 0110: `ctrl.*` lines go to ctrl's own queue, never through the echo/nano consumer.
let ctrl = CtrlStdio(backend: makeCtrlBackend(), write: { out.writeLine($0) })

HelperStdio.run(out: out, start: { await controller.start() }, handle: { await controller.handle($0) },
                heartbeat: { await controller.heartbeat() }, shutdown: { await controller.shutdown() },
                divert: { l in
                    guard CtrlStdio.isCtrlLine(l) else { return false }
                    ctrl.submit(l)
                    return true
                })
