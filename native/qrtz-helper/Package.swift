// swift-tools-version: 6.0
// qrtz-helper: the native macOS side of qrtz. It speaks newline-delimited JSON over stdio, using the
// protocol defined in packages/core/src/helper/protocol.ts (golden fixtures live in Fixtures/protocol).
//
// QrtzHelperCore is Foundation-only, so it builds and tests on Linux (it is checked from the repo on Linux).
// The executable adds the Apple-only pieces: AX, Core Audio taps, ScreenCaptureKit, FluidAudio and Speech.
import PackageDescription

#if os(macOS)
let platformDeps: [Package.Dependency] = [
    // Pre-1.0: minor bumps may break API, so stay on 0.17.x (APIs checked against the v0.17.7 sources).
    .package(url: "https://github.com/FluidInference/FluidAudio.git", .upToNextMinor(from: "0.17.7"))
]
let execDeps: [Target.Dependency] = [
    "QrtzHelperCore",
    "QrtzCtrl",
    .product(name: "FluidAudio", package: "FluidAudio"),
]
#else
let platformDeps: [Package.Dependency] = []
let execDeps: [Target.Dependency] = ["QrtzHelperCore", "QrtzCtrl"]
#endif

let package = Package(
    name: "qrtz-helper",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "qrtz-helper", targets: ["qrtz-helper"])],
    dependencies: platformDeps,
    targets: [
        .target(name: "QrtzHelperCore"),
        .executableTarget(
            name: "qrtz-helper", dependencies: execDeps,
            // Apple framework types (AVAudioPCMBuffer, AXUIElement, SCStream) are not Sendable-annotated consistently
            // across SDKs; Swift 5 mode keeps strict-concurrency findings as warnings so SDK drift can't break the build.
            swiftSettings: [.swiftLanguageMode(.v5)]),
        .testTarget(name: "QrtzHelperCoreTests", dependencies: ["QrtzHelperCore"]),
        // ctrl (ADR 0100): the never-takes-over-the-ui guard, policy and background actions. Its own module, kept
        // apart from the echo/nano code; Foundation-only parts build on Linux, Sources/QrtzCtrl/mac on macOS only.
        // The executable links it only to route `ctrl.*` lines to it (ADR 0110).
        .target(name: "QrtzCtrl", swiftSettings: [.swiftLanguageMode(.v5)]),
        .testTarget(name: "QrtzCtrlTests", dependencies: ["QrtzCtrl"], swiftSettings: [.swiftLanguageMode(.v5)]),
    ]
)
