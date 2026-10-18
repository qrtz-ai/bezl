import Foundation
import QrtzGateProbe

// qrtz-echo: echo's own module app (ADR 0146 M1.3, ADR 0144), for dictation (talk, the pill, the push key).
// Its bundle (ai.qrtz.echo, ai.qrtz.echo.entitlements) holds echo's own TCC grants. The probe mode is live now, so
// scripts/modules/mac-gate.sh can run its gates against the real bundle. The echo code itself still runs inside
// qrtz-helper until it is carved out of the monolith QrtzHelperCore (this app may import only shared and echo
// targets; the offline_graph ratchet holds that), so a plain run says so and exits.
// Usage: qrtz-echo --version | --qrtz-gate-probe <ax|mic|socket|ae>

let echoAppVersion = "0.1.0"
GateProbe.runIfRequested(module: "echo")
if CommandLine.arguments.contains("--version") { print(echoAppVersion); exit(0) }
FileHandle.standardError.write(Data("qrtz-echo: echo still runs in qrtz-helper (ADR 0146 M1.3); only --qrtz-gate-probe and --version for now\n".utf8))
exit(64)
