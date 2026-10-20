import Foundation
import QrtzCtrl
import QrtzHelperCore
#if os(macOS)
import QrtzNanoMac
#endif

// qrtz-helper: the monolith helper (ADR 0146: echo and nano in one app until the module apps take over), stdio
// JSON-lines (HelperStdio). stdout carries protocol messages only; diagnostics go to stderr.
// Usage: qrtz-helper --stdio | --version | --dump-ax (macOS: print the AX tree of detected meeting windows)
//        | --bench --mic a.aiff[,b.aiff] [--system c.aiff] (macOS: ADR 0079 model measurements, see mac/Bench.swift)
//        | --echo-bench <clips dir> [--out f.json] (macOS: ADR 0099 dictation tuning bench, see mac/EchoBench.swift)
//        | --echo-warmup <clip> --expect "<words>" --press-at 0|<ms>|ready (macOS: QRT-270 CI, see mac/EchoWarmup.swift)

let helperVersion = "0.1.0"
let args = CommandLine.arguments

if args.contains("--version") { print(helperVersion); exit(0) }
#if os(macOS)
if args.contains("--dump-ax") { AXDump.run(); exit(0) }
if args.contains("--echo-bench") { EchoBenchCLI.run(args) }
if args.contains("--echo-warmup") { EchoWarmupCLI.run(args) }
if args.contains("--bench") { Bench.run(args) }
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
