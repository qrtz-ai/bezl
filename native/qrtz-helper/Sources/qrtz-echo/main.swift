import Foundation
import QrtzEcho
import QrtzGateProbe
import QrtzShared
#if os(macOS)
import QrtzEchoMac
import QrtzMacShared
#endif

// qrtz-echo: echo's extension helper (ADR 0146 M2.5), for dictation (talk, the pill, the push key). An executable
// inside the installed echo extension, not an app: qrtz spawns it directly and stays its responsible process, so it
// uses qrtz's own TCC grants (one qrtz entry in Privacy settings; the speaker gate's system tap rides qrtz's screen &
// system audio grant). Its host has its own arbiter. The Rust host runs it with QRTZ_MODULE_HELPERS=split (ADR 0146
// M2.4); qrtz-helper still runs echo and nano together by default.
// Usage: qrtz-echo [--stdio] | --version | --qrtz-gate-probe <ax|mic|socket|ae|sysaudio>

let echoAppVersion = "0.1.0"
GateProbe.runIfRequested(module: "echo")
if CommandLine.arguments.contains("--version") { print(echoAppVersion); exit(0) }

#if os(macOS)
logTag = "qrtz-echo"
let micHub: MicHub? = MicHub(source: DictationMicSource())
let platform = makeEchoPlatform(micHub: micHub!)
let permissions: PermissionsProvider = MacPermissions()
let osName = macOSVersionString()
#else
let micHub: MicHub? = nil
let platform = (deps: EchoDeps(), features: ["stub"])
let permissions: PermissionsProvider = UnsupportedPermissions()
let osName = "linux"
#endif

let out = StdoutLines()
let link = HostLink(send: { out.write($0) })
let speech = SpeechSinks()
// N174: no nano in this app to warm first, so echo answers models.warm itself (a prewarm of its recogniser).
let echo = EchoController(deps: platform.deps, link: link, now: HelperStdio.epochMs, speech: speech,
                          echoReference: EchoReference(), permissions: permissions, ownsWarm: true)
let host = ModuleHost(
    deps: HostDeps(permissions: permissions, micHub: micHub, speechFds: SocketFdReceiver.fromEnvironment(),
                   helperVersion: echoAppVersion, os: osName, features: platform.features + ["permissions.watch", "module:echo"], module: "echo"),
    link: link, now: HelperStdio.epochMs, speech: speech, parts: [echo])
HelperStdio.run(host, out: out)
