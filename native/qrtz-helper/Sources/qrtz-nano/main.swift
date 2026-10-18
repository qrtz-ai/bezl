import Foundation
import QrtzGateProbe
import QrtzNano
import QrtzShared
#if os(macOS)
import QrtzMacShared
import QrtzNanoMac
#endif

// qrtz-nano: nano's extension helper (ADR 0146 M2.5), for meetings (detection, recording, transcripts, speakers,
// summaries). An executable inside the installed nano extension, not an app: qrtz spawns it directly and stays its
// responsible process, so it uses qrtz's own TCC grants. Its host has its own arbiter. The Rust host runs it with
// QRTZ_MODULE_HELPERS=split (ADR 0146 M2.4); qrtz-helper still runs echo and nano together by default.
// Usage: qrtz-nano [--stdio] | --version | --qrtz-gate-probe <ax|mic|socket|ae|sysaudio>

let nanoAppVersion = "0.1.0"
GateProbe.runIfRequested(module: "nano")
if CommandLine.arguments.contains("--version") { print(nanoAppVersion); exit(0) }

#if os(macOS)
logTag = "qrtz-nano"
let micHub: MicHub? = MicHub(source: DictationMicSource())
let platform = makeNanoPlatform(micHub: micHub!)
let permissions: PermissionsProvider = MacPermissions()
let osName = macOSVersionString()
#else
let micHub: MicHub? = nil
let platform = (deps: NanoDeps(), features: ["stub"])
let permissions: PermissionsProvider = UnsupportedPermissions()
let osName = "linux"
#endif

let out = StdoutLines()
let link = HostLink(send: { out.write($0) })
let speech = SpeechSinks()
let nano = NanoController(deps: platform.deps, link: link, now: HelperStdio.epochMs, speech: speech, echoReference: EchoReference())
let host = ModuleHost(
    deps: HostDeps(permissions: permissions, micHub: micHub, speechFds: SocketFdReceiver.fromEnvironment(),
                   helperVersion: nanoAppVersion, os: osName, features: platform.features + ["permissions.watch", "module:nano"], module: "nano"),
    link: link, now: HelperStdio.epochMs, speech: speech, parts: [nano])
HelperStdio.run(host, out: out)
