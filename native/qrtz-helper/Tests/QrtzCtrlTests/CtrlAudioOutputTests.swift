// ADR 0198: "switch the sound to my airpods", helper side. The act and its app-less measure parse from the fixture and
// reach the backend with no app and no content; `kind` must be one of the closed set's exact spellings and nothing else
// in the request is read; the pick takes the one visible output device of that kind, refuses none or several, and never
// sees a name; the Mac side sets only the default output device, reads only transport / streams / hidden, and spawns
// nothing.
import Foundation
import XCTest
@testable import QrtzCtrl
#if canImport(CoreAudio)
import CoreAudio
#endif

private final class AudioFake: CtrlBackend {
    var acted: [CtrlRoute] = []
    var measured: [(String, String)] = []
    func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { ([], nil) }
    func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw CtrlFailure("app.not_running") }
    func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { [] }
    func act(_ route: CtrlRoute, content: [String: String]) throws { acted.append(route) }
    func measure(check: String, app: String) -> Int? {
        measured.append((check, app))
        return check == CtrlAudioOutput.check ? CtrlAudioKind.bluetooth.code : nil
    }
    func restore(frontmostPid: Int32?) -> Bool { false }
    func laya(state: String, question: String, options: [String]) throws -> [Double] { [] }
    func frontmostPid() -> Int32? { 100 }
    func pid(of app: String) -> Int32? { nil }
}

private func json(_ s: String) -> [String: Any] { try! JSONSerialization.jsonObject(with: Data(s.utf8)) as! [String: Any] }

private let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

private func fixtureLine(_ match: (String) -> Bool) throws -> String {
    let raw = try String(contentsOf: root.appendingPathComponent("Fixtures/ctrl/protocol.jsonl"), encoding: .utf8)
    return try XCTUnwrap(raw.split(separator: "\n").map(String.init).first(where: match))
}

private typealias D = CtrlAudioOutput.Device

final class CtrlAudioOutputTests: XCTestCase {
    func testSetAudioOutputReachesTheBackendWithNoAppAndNoContent() throws {
        let line = try fixtureLine { $0.contains(#""op":"set_audio_output""#) }
        guard case let .act(_, _, route, content)? = CtrlRequest.parse(line) else { return XCTFail("set_audio_output") }
        XCTAssertEqual(route, .setAudioOutput(kind: .bluetooth))
        XCTAssertTrue(content.isEmpty)
        XCTAssertNil(CtrlServer.appOf(route))
        XCTAssertNoThrow(try CtrlServer.recheck(route, frontmostPid: 100, targetPid: nil))
        let fake = AudioFake()
        XCTAssertEqual(json(CtrlServer(backend: fake).handle(line: line))["ok"] as? Bool, true)
        XCTAssertEqual(fake.acted, [.setAudioOutput(kind: .bluetooth)])
    }

    func testItsMeasureHasNoAppAndReachesTheBackend() throws {
        let line = try fixtureLine { $0.contains(#""check":"audio_output""#) }
        guard case let .measure(_, check, app)? = CtrlRequest.parse(line) else { return XCTFail("audio_output") }
        XCTAssertEqual(check, CtrlAudioOutput.check)
        XCTAssertEqual(app, "")
        XCTAssertTrue(CtrlRoute.applessChecks.contains(CtrlAudioOutput.check))
        let fake = AudioFake()
        let r = json(CtrlServer(backend: fake).handle(line: line))
        XCTAssertEqual(r["value"] as? Int, 20, "\(r)")
        XCTAssertEqual(fake.measured.map(\.0), [CtrlAudioOutput.check])
    }

    func testKindIsAClosedSpellingAndNothingElseIsRead() {
        func parse(_ route: String) -> CtrlRoute? {
            let line = #"{"type":"ctrl.act","id":"x","actionId":"a","route":"# + route + #","content":{}}"#
            guard case let .act(_, _, r, _)? = CtrlRequest.parse(line) else { return nil }
            return r
        }
        for k in CtrlAudioKind.allCases {
            XCTAssertEqual(parse(#"{"route":"workspace","op":"set_audio_output","kind":""# + k.rawValue + #""}"#),
                           .setAudioOutput(kind: k))
        }
        XCTAssertEqual(CtrlAudioKind.allCases.map(\.rawValue), ["built_in", "bluetooth", "usb", "display"])
        XCTAssertEqual(CtrlAudioKind.allCases.map(\.code), [10, 20, 30, 40], "Rust AudioOutput::code")
        // A device name or id riding along is never read: the route is still only the kind.
        XCTAssertEqual(parse(#"{"route":"workspace","op":"set_audio_output","kind":"usb","device":"jared's airpods","id":73}"#),
                       .setAudioOutput(kind: .usb))
        for bad in [#"{"route":"workspace","op":"set_audio_output","kind":"airplay"}"#,
                    #"{"route":"workspace","op":"set_audio_output","kind":"Bluetooth"}"#,
                    #"{"route":"workspace","op":"set_audio_output","kind":"builtin"}"#,
                    #"{"route":"workspace","op":"set_audio_output","kind":20}"#,
                    #"{"route":"workspace","op":"set_audio_output","device":"airpods"}"#,
                    #"{"route":"workspace","op":"set_audio_output"}"#,
                    #"{"route":"files","op":"set_audio_output","kind":"usb"}"#] {
            if let r = parse(bad) {
                if case .setAudioOutput = r { XCTFail(bad) }
            }
        }
    }

    func testThePickTakesTheOneVisibleOutputOfThatKind() throws {
        let speakers = D(id: 70, transport: CtrlAudioOutput.transportBuiltIn, outputs: true, hidden: false)
        let mic = D(id: 71, transport: CtrlAudioOutput.transportBuiltIn, outputs: false, hidden: false)
        let airpods = D(id: 80, transport: CtrlAudioOutput.transportBluetooth, outputs: true, hidden: false)
        let airpodsMic = D(id: 81, transport: CtrlAudioOutput.transportBluetooth, outputs: false, hidden: false)
        let le = D(id: 82, transport: CtrlAudioOutput.transportBluetoothLE, outputs: true, hidden: false)
        let usb = D(id: 90, transport: CtrlAudioOutput.transportUSB, outputs: true, hidden: false)
        let hdmi = D(id: 100, transport: CtrlAudioOutput.transportHDMI, outputs: true, hidden: false)
        let dp = D(id: 101, transport: CtrlAudioOutput.transportDisplayPort, outputs: true, hidden: false)
        let hiddenUSB = D(id: 91, transport: CtrlAudioOutput.transportUSB, outputs: true, hidden: true)
        let aggregate = D(id: 110, transport: 0x6772_7570, outputs: true, hidden: false)  // 'grup'
        let airplay = D(id: 111, transport: 0x6169_7270, outputs: true, hidden: false)  // 'airp'
        let all = [mic, speakers, airpodsMic, airpods, usb, hiddenUSB, hdmi, aggregate, airplay]
        XCTAssertEqual(try CtrlAudioOutput.pick(all, kind: .builtIn), 70)
        XCTAssertEqual(try CtrlAudioOutput.pick(all, kind: .bluetooth), 80, "the input-only half of the AirPods is skipped")
        XCTAssertEqual(try CtrlAudioOutput.pick(all, kind: .usb), 90, "a hidden device is never picked")
        XCTAssertEqual(try CtrlAudioOutput.pick(all, kind: .display), 100)
        XCTAssertEqual(try CtrlAudioOutput.pick([speakers, dp], kind: .display), 101)
        XCTAssertEqual(try CtrlAudioOutput.pick([speakers, le], kind: .bluetooth), 82, "Bluetooth LE is Bluetooth")
        XCTAssertThrowsError(try CtrlAudioOutput.pick([speakers, usb, aggregate, airplay], kind: .bluetooth)) {
            XCTAssertEqual($0 as? CtrlFailure, CtrlFailure("audio_output.none"))
        }
        XCTAssertThrowsError(try CtrlAudioOutput.pick([speakers, airpods, le], kind: .bluetooth)) {
            XCTAssertEqual($0 as? CtrlFailure, CtrlFailure("audio_output.not_one"), "two headsets: never the first")
        }
        XCTAssertThrowsError(try CtrlAudioOutput.pick([speakers, hdmi, dp], kind: .display)) {
            XCTAssertEqual($0 as? CtrlFailure, CtrlFailure("audio_output.not_one"))
        }
        XCTAssertThrowsError(try CtrlAudioOutput.pick([], kind: .builtIn))
    }

    func testTheMeasureIsTheKindsCodeAndNeverMakesUpAValue() {
        XCTAssertEqual(CtrlAudioOutput.value(transport: CtrlAudioOutput.transportBuiltIn), 10)
        XCTAssertEqual(CtrlAudioOutput.value(transport: CtrlAudioOutput.transportBluetooth), 20)
        XCTAssertEqual(CtrlAudioOutput.value(transport: CtrlAudioOutput.transportBluetoothLE), 20)
        XCTAssertEqual(CtrlAudioOutput.value(transport: CtrlAudioOutput.transportUSB), 30)
        XCTAssertEqual(CtrlAudioOutput.value(transport: CtrlAudioOutput.transportHDMI), 40)
        XCTAssertEqual(CtrlAudioOutput.value(transport: CtrlAudioOutput.transportDisplayPort), 40)
        XCTAssertEqual(CtrlAudioOutput.value(transport: 0x6169_7270), 0, "AirPlay is another kind")
        XCTAssertEqual(CtrlAudioOutput.value(transport: 0), 0)
        XCTAssertNil(CtrlAudioOutput.value(transport: nil))
    }

    #if canImport(CoreAudio)
    /// On a Mac: the four-char codes here are the SDK's.
    func testTheTransportCodesAreTheSDKs() {
        XCTAssertEqual(CtrlAudioOutput.transportBuiltIn, kAudioDeviceTransportTypeBuiltIn)
        XCTAssertEqual(CtrlAudioOutput.transportBluetooth, kAudioDeviceTransportTypeBluetooth)
        XCTAssertEqual(CtrlAudioOutput.transportBluetoothLE, kAudioDeviceTransportTypeBluetoothLE)
        XCTAssertEqual(CtrlAudioOutput.transportUSB, kAudioDeviceTransportTypeUSB)
        XCTAssertEqual(CtrlAudioOutput.transportHDMI, kAudioDeviceTransportTypeHDMI)
        XCTAssertEqual(CtrlAudioOutput.transportDisplayPort, kAudioDeviceTransportTypeDisplayPort)
    }
    #endif

    /// The Mac side sets one property (the default output device) and reads only transport, streams and hidden; never
    /// a name or UID, never the input or system (alert) device, never a volume or mute; it spawns nothing.
    func testTheMacSideSetsOnlyTheDefaultOutputAndSpawnsNothing() throws {
        let src = try String(contentsOf: root.appendingPathComponent("Sources/QrtzCtrl/mac/CtrlAudioOutputMac.swift"), encoding: .utf8)
        let code = src.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }.joined(separator: "\n")
        XCTAssertEqual(code.components(separatedBy: "AudioObjectSetPropertyData(").count - 1, 1)
        for banned in ["Process(", "posix_spawn", "NSTask", "system(", "NSAppleScript", "/bin/", "/usr/",
                       "kAudioObjectPropertyName", "kAudioDevicePropertyDeviceUID", "kAudioDevicePropertyModelUID",
                       "kAudioHardwarePropertyDefaultInputDevice", "kAudioHardwarePropertyDefaultSystemOutputDevice",
                       "VolumeScalar", "kAudioDevicePropertyMute", "AudioDeviceStart", "AudioDeviceCreateIOProcID",
                       "CATapDescription", "AVAudio", "kAudioObjectPropertyScopeInput"] {
            XCTAssertFalse(code.contains(banned), banned)
        }
        // Nothing else in QrtzCtrl talks to Core Audio.
        let dir = root.appendingPathComponent("Sources/QrtzCtrl")
        let files = try XCTUnwrap(FileManager.default.enumerator(atPath: dir.path)).compactMap { $0 as? String }
            .filter { $0.hasSuffix(".swift") }
        for f in files where !f.hasSuffix("CtrlAudioOutputMac.swift") {
            let s = try String(contentsOf: dir.appendingPathComponent(f), encoding: .utf8)
            XCTAssertFalse(s.contains("import CoreAudio") || s.contains("AudioObjectSetPropertyData"), f)
        }
    }
}
