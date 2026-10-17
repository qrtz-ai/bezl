// swift-tools-version: 6.0
// qrtz-helper: the native macOS side of qrtz. It speaks newline-delimited JSON over stdio, using the
// protocol defined in packages/core/src/helper/protocol.ts (golden fixtures live in Fixtures/protocol).
//
// QrtzHelperCore is Foundation-only, so it builds and tests on Linux (it is checked from the repo on Linux).
// The executable adds the Apple-only pieces: AX, Core Audio taps, ScreenCaptureKit, FluidAudio and Speech.
import PackageDescription

#if os(macOS)
let platformDeps: [Package.Dependency] = [
    .package(url: "https://github.com/FluidInference/FluidAudio.git", from: "0.17.7", traits: [])
]
let execDeps: [Target.Dependency] = [
    "QrtzHelperCore",
    .product(name: "FluidAudio", package: "FluidAudio"),
]
#else
let platformDeps: [Package.Dependency] = []
let execDeps: [Target.Dependency] = ["QrtzHelperCore"]
#endif

let package = Package(
    name: "qrtz-helper",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "qrtz-helper", targets: ["qrtz-helper"])],
    dependencies: platformDeps,
    targets: [
        .target(name: "QrtzHelperCore"),
        .executableTarget(name: "qrtz-helper", dependencies: execDeps),
        .testTarget(name: "QrtzHelperCoreTests", dependencies: ["QrtzHelperCore"]),
    ]
)
