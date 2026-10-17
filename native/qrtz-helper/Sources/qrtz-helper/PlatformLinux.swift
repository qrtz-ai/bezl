#if !os(macOS)
import Foundation
import QrtzHelperCore

/// Non-macOS build: speaks the protocol (so contract/integration tests can run it) but has no capture backends.
struct UnsupportedPermissions: PermissionsProvider {
    func status() -> [OsPermission: PermissionState] {
        Dictionary(uniqueKeysWithValues: OsPermission.allCases.map { ($0, PermissionState.unsupported) })
    }
    func request(_ p: OsPermission) async {}
}

func makePlatformDeps(version: String) -> HelperDeps {
    HelperDeps(permissions: UnsupportedPermissions(), meetings: nil, audio: [], transcribers: [], helperVersion: version, os: "linux", features: ["stub"])
}
#endif
