#if os(macOS)
import Foundation
import QrtzEchoMac
import QrtzHelperCore
import QrtzMacShared
import QrtzNanoMac

/// The monolith's platform: echo's and nano's macOS parts (QrtzEchoMac, QrtzNanoMac) on one shared mic tap (H6 mux:
/// one AVAudioEngine tap for meeting capture and dictation) and one model-load gate (N137).
func makePlatformDeps(version: String) -> HelperDeps {
    let micHub = MicHub(source: DictationMicSource())
    let loads = ModelLoads()
    let nano = makeNanoPlatform(micHub: micHub, modelLoads: loads)
    let echo = makeEchoPlatform(micHub: micHub, modelLoads: loads)
    let host = HostDeps(permissions: MacPermissions(), micHub: micHub, helperVersion: version, os: macOSVersionString(),
                        features: nano.features + echo.features + ["helper.mux", "permissions.watch"], module: "helper")
    return HelperDeps(host: host, echo: echo.deps, nano: nano.deps)
}
#endif
