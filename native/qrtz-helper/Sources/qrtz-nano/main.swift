import Foundation
import QrtzGateProbe

// qrtz-nano: nano's own module app (ADR 0146 M1.3, ADR 0144), for meetings (detection, recording,
// transcripts, speakers, summaries).
// Its bundle (ai.qrtz.nano, ai.qrtz.nano.entitlements) holds nano's own TCC grants. The probe mode is live now, so
// scripts/modules/mac-gate.sh can run its gates against the real bundle. The nano code itself still runs inside
// qrtz-helper until it is carved out of the monolith QrtzHelperCore (this app may import only shared and nano
// targets; the offline_graph ratchet holds that), so a plain run says so and exits.
// Usage: qrtz-nano --version | --qrtz-gate-probe <ax|mic|socket|ae>

let nanoAppVersion = "0.1.0"
GateProbe.runIfRequested(module: "nano")
if CommandLine.arguments.contains("--version") { print(nanoAppVersion); exit(0) }
FileHandle.standardError.write(Data("qrtz-nano: nano still runs in qrtz-helper (ADR 0146 M1.3); only --qrtz-gate-probe and --version for now\n".utf8))
exit(64)
