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
    products: [
        .executable(name: "qrtz-helper", targets: ["qrtz-helper"]),
        // ADR 0146 M1.3: the module apps, one bundle each (ai.qrtz.echo, ai.qrtz.nano, ai.qrtz.ctrl).
        .executable(name: "qrtz-echo", targets: ["qrtz-echo"]),
        .executable(name: "qrtz-nano", targets: ["qrtz-nano"]),
        .executable(name: "qrtz-ctrl", targets: ["qrtz-ctrl"]),
    ],
    dependencies: platformDeps,
    targets: [
        // ADR 0146 M1.5: the wire protocol and the module-agnostic helpers, shared by every module app (owner: shared).
        .target(name: "QrtzShared"),
        .testTarget(name: "QrtzSharedTests", dependencies: ["QrtzShared"]),
        .target(name: "QrtzHelperCore", dependencies: ["QrtzShared"]),
        .executableTarget(
            name: "qrtz-helper", dependencies: execDeps,
            // Apple framework types (AVAudioPCMBuffer, AXUIElement, SCStream) are not Sendable-annotated consistently
            // across SDKs; Swift 5 mode keeps strict-concurrency findings as warnings so SDK drift can't break the build.
            swiftSettings: [.swiftLanguageMode(.v5)]),
        .testTarget(name: "QrtzHelperCoreTests", dependencies: ["QrtzHelperCore", "QrtzShared"]),
        // ctrl (ADR 0100): the never-takes-over-the-ui guard, policy and background actions. Its own module, kept
        // apart from the echo/nano code; Foundation-only parts build on Linux, Sources/QrtzCtrl/mac on macOS only.
        // The executable links it only to route `ctrl.*` lines to it (ADR 0110).
        .target(name: "QrtzCtrl", swiftSettings: [.swiftLanguageMode(.v5)]),
        .testTarget(name: "QrtzCtrlTests", dependencies: ["QrtzCtrl"], swiftSettings: [.swiftLanguageMode(.v5)]),
        // ADR 0146 M1.3 / ADR 0144: `--qrtz-gate-probe <ax|mic|socket|ae>`, shared by every module app (owner: shared).
        .target(name: "QrtzGateProbe", swiftSettings: [.swiftLanguageMode(.v5)]),
        .testTarget(name: "QrtzGateProbeTests", dependencies: ["QrtzGateProbe"]),
        // The module apps. Each imports only shared targets and its own module's (the offline_graph ratchet): echo and
        // nano are probe-only until their code leaves the monolith QrtzHelperCore; ctrl runs QrtzCtrl's stdio.
        .executableTarget(name: "qrtz-echo", dependencies: ["QrtzGateProbe"], swiftSettings: [.swiftLanguageMode(.v5)]),
        .executableTarget(name: "qrtz-nano", dependencies: ["QrtzGateProbe"], swiftSettings: [.swiftLanguageMode(.v5)]),
        .executableTarget(
            name: "qrtz-ctrl", dependencies: ["QrtzCtrl", "QrtzGateProbe"], swiftSettings: [.swiftLanguageMode(.v5)]),
    ]
)
