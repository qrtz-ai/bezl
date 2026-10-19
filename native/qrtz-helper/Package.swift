// swift-tools-version: 6.0
// qrtz-helper: the native macOS side of qrtz. It speaks newline-delimited JSON over stdio, using the
// protocol defined in packages/core/src/helper/protocol.ts (golden fixtures live in Fixtures/protocol).
//
// QrtzHelperCore is Foundation-only, so it builds and tests on Linux (it is checked from the repo on Linux).
// The Apple-only pieces (AX, Core Audio taps, ScreenCaptureKit, FluidAudio, Speech) are QrtzMacShared, QrtzEchoMac
// and QrtzNanoMac (ADR 0146 M2.5).
import PackageDescription

#if os(macOS)
let platformDeps: [Package.Dependency] = [
    // Pinned exactly (audit P1-5): the reviewed v0.17.7 sources only (tag v0.17.7 = 503b4bd1bbf7220882de39fe8ae6716aae4132da,
    // locked in the committed Package.resolved). A bump is a reviewed change: re-check the local-only loaders
    // (no ModelHub / loadFromHuggingFace) and update both files together.
    .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.17.7")
]
let fluidAudio: [Target.Dependency] = [.product(name: "FluidAudio", package: "FluidAudio")]
#else
let platformDeps: [Package.Dependency] = []
let fluidAudio: [Target.Dependency] = []
#endif
// The monolith links both module parts' macOS seams (ADR 0146 M2.5; allowlisted until qrtz-helper goes).
let execDeps: [Target.Dependency] = ["QrtzHelperCore", "QrtzCtrl", "QrtzMacShared", "QrtzEchoMac", "QrtzNanoMac"] + fluidAudio
// Apple framework types (AVAudioPCMBuffer, AXUIElement, SCStream) are not Sendable-annotated consistently across SDKs;
// Swift 5 mode keeps strict-concurrency findings as warnings so SDK drift can't break the build.
let macSeams: [SwiftSetting] = [.swiftLanguageMode(.v5)]

let package = Package(
    name: "qrtz-helper",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "qrtz-helper", targets: ["qrtz-helper"]),
        // ADR 0146 M1.3 / M2.5: the extension helpers, plain executables inside each signed extension (no app bundle;
        // spawned by qrtz, which stays their responsible process for TCC).
        .executable(name: "qrtz-echo", targets: ["qrtz-echo"]),
        .executable(name: "qrtz-nano", targets: ["qrtz-nano"]),
        .executable(name: "qrtz-ctrl", targets: ["qrtz-ctrl"]),
        // ADR 0159: qrtz snap, Spectacle's window snapping as an extension.
        .executable(name: "qrtz-snap", targets: ["qrtz-snap"]),
    ],
    dependencies: platformDeps,
    targets: [
        // ADR 0146 M1.5: the wire protocol and the module-agnostic helpers, shared by every module app (owner: shared).
        .target(name: "QrtzShared"),
        .testTarget(name: "QrtzSharedTests", dependencies: ["QrtzShared"]),
        // ADR 0146 M2.5: echo's and nano's code, each its own target (owner: echo / nano). Foundation-only, so it builds
        // and tests on Linux; their macOS seams are QrtzEchoMac / QrtzNanoMac.
        .target(name: "QrtzEcho", dependencies: ["QrtzShared"]),
        .target(name: "QrtzNano", dependencies: ["QrtzShared"]),
        // The monolith: both parts behind one host until qrtz-helper goes (its module imports are allowlisted).
        .target(name: "QrtzHelperCore", dependencies: ["QrtzShared", "QrtzEcho", "QrtzNano"]),
        // ADR 0146 M2.5: the macOS seams, Apple-only (empty on Linux). QrtzMacShared (owner: shared) is what both modules'
        // capture uses (the mic, converters, TCC, local model dirs, SpeechAnalyzer); QrtzEchoMac and QrtzNanoMac are
        // each module's own. Both link FluidAudio statically as the interim step (ADR 0144 note; allowlisted edges)
        // until the voice service (ai.qrtz.inference.voice) serves the models.
        .target(name: "QrtzMacShared", dependencies: ["QrtzShared"], swiftSettings: macSeams),
        .target(name: "QrtzEchoMac", dependencies: ["QrtzEcho", "QrtzMacShared", "QrtzShared"] + fluidAudio, swiftSettings: macSeams),
        .target(name: "QrtzNanoMac", dependencies: ["QrtzNano", "QrtzMacShared", "QrtzShared"] + fluidAudio, swiftSettings: macSeams),
        .executableTarget(name: "qrtz-helper", dependencies: execDeps, swiftSettings: macSeams),
        .testTarget(name: "QrtzHelperCoreTests", dependencies: ["QrtzHelperCore", "QrtzShared", "QrtzEcho", "QrtzNano"]),
        // ctrl (ADR 0100): the never-takes-over-the-ui guard, policy and background actions. Its own module, kept
        // apart from the echo/nano code; Foundation-only parts build on Linux, Sources/QrtzCtrl/mac on macOS only.
        // The executable links it only to route `ctrl.*` lines to it (ADR 0110).
        .target(name: "QrtzCtrl", dependencies: ["QrtzShared"], swiftSettings: [.swiftLanguageMode(.v5)]),
        .testTarget(name: "QrtzCtrlTests", dependencies: ["QrtzCtrl", "QrtzShared"], swiftSettings: [.swiftLanguageMode(.v5)]),
        // ADR 0146 M1.3 / ADR 0144: `--qrtz-gate-probe <ax|mic|socket|ae>`, shared by every module app (owner: shared).
        .target(name: "QrtzGateProbe", swiftSettings: [.swiftLanguageMode(.v5)]),
        .testTarget(name: "QrtzGateProbeTests", dependencies: ["QrtzGateProbe"]),
        // The module apps. Each imports only shared targets and its own module's (the offline_graph ratchet): echo and
        // nano run their controller behind a host of their own (ADR 0146 M2.5); ctrl runs QrtzCtrl's stdio.
        .executableTarget(name: "qrtz-echo", dependencies: ["QrtzGateProbe", "QrtzShared", "QrtzEcho", "QrtzMacShared", "QrtzEchoMac"],
                          swiftSettings: macSeams),
        .executableTarget(name: "qrtz-nano", dependencies: ["QrtzGateProbe", "QrtzShared", "QrtzNano", "QrtzMacShared", "QrtzNanoMac"],
                          swiftSettings: macSeams),
        // qrtz snap (ADR 0159): Spectacle's frame math, history, shortcuts and wire, Foundation-only (tested on Linux);
        // Sources/QrtzSnap/mac is the Carbon hotkeys, AX mover and status item, macOS only.
        .target(name: "QrtzSnap", swiftSettings: [.swiftLanguageMode(.v5)]),
        .testTarget(name: "QrtzSnapTests", dependencies: ["QrtzSnap"], swiftSettings: [.swiftLanguageMode(.v5)]),
        .executableTarget(
            name: "qrtz-snap", dependencies: ["QrtzSnap", "QrtzGateProbe"], swiftSettings: [.swiftLanguageMode(.v5)]),
        .executableTarget(
            name: "qrtz-ctrl", dependencies: ["QrtzCtrl", "QrtzGateProbe"], swiftSettings: [.swiftLanguageMode(.v5)]),
    ]
)
