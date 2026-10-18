import Foundation
import XCTest
@testable import QrtzGateProbe

/// ADR 0146 M1.3: the module apps' `--qrtz-gate-probe` mode, in the stand-in probe's line format.
final class GateProbeTests: XCTestCase {
    func testNoFlagRunsTheAppNormally() {
        XCTAssertEqual(GateProbeRequest.parse(["qrtz-ctrl", "--stdio"]), .absent)
        XCTAssertEqual(GateProbeRequest.parse([]), .absent)
    }

    func testEveryGateCommandParses() {
        for c in GateProbeCommand.allCases {
            XCTAssertEqual(GateProbeRequest.parse(["app", gateProbeFlag, c.rawValue]), .probe(GateProbeRequest(command: c)))
        }
        XCTAssertEqual(GateProbeCommand.allCases.map(\.rawValue), ["ax", "mic", "socket", "ae"])
    }

    func testOptions() {
        XCTAssertEqual(
            GateProbeRequest.parse(["app", gateProbeFlag, "mic", "--open", "--ask", "--out", "/tmp/x"]),
            .probe(GateProbeRequest(command: .mic, ask: true, openMic: true, out: "/tmp/x")))
        XCTAssertEqual(
            GateProbeRequest.parse([gateProbeFlag, "socket", "4242"]),
            .probe(GateProbeRequest(command: .socket, port: 4242)))
        XCTAssertEqual(GateProbeRequest.parse([gateProbeFlag, "ax", "--ask"]), .probe(GateProbeRequest(command: .ax, ask: true)))
    }

    /// A bad probe request never falls through to a normal run (that would start dictation or ctrl from a gate).
    func testBadRequestsAreInvalidNotAbsent() {
        for argv in [
            [gateProbeFlag], [gateProbeFlag, "xpc"], [gateProbeFlag, "info"], [gateProbeFlag, "socket", "0"],
            [gateProbeFlag, "socket", "70000"], [gateProbeFlag, "ax", "--out"], [gateProbeFlag, "ax", "4242"],
            [gateProbeFlag, "ae", "--stdio"],
        ] {
            guard case .invalid = GateProbeRequest.parse(argv) else { return XCTFail("\(argv) not invalid") }
        }
    }

    struct Fake: GateProbeBackend {
        var trusted = true
        func axTrusted(ask: Bool) -> Bool { trusted && !ask }
        func mic(ask: Bool, open: Bool) -> [(String, String)] { [("status", open ? "authorized" : "denied")] }
        func automateSystemEvents(ask: Bool) -> [(String, String)] { [("permission", "0"), ("sent", "ok")] }
    }

    func line(_ argv: [String], module: String = "echo", backend: GateProbeBackend = Fake()) -> String {
        guard case .probe(let r) = GateProbeRequest.parse(argv) else { XCTFail("no probe"); return "" }
        return GateProbe.line(r.command.rawValue, GateProbe.fields(r, module: module, bundleId: "ai.qrtz.\(module)", backend: backend))
    }

    /// The gate script reads `probe <cmd> key=value ...` (its field() splits on spaces), like the stand-in probe.
    func testLinesMatchTheStandInProbeFormat() {
        XCTAssertEqual(line([gateProbeFlag, "ax"]), "probe ax bundle=ai.qrtz.echo module=echo trusted=true")
        XCTAssertEqual(line([gateProbeFlag, "ax", "--ask"], module: "ctrl"), "probe ax bundle=ai.qrtz.ctrl module=ctrl trusted=false")
        XCTAssertEqual(line([gateProbeFlag, "mic", "--open"], module: "nano"), "probe mic bundle=ai.qrtz.nano module=nano status=authorized")
        XCTAssertEqual(line([gateProbeFlag, "ae"], module: "ctrl"), "probe ae bundle=ai.qrtz.ctrl module=ctrl permission=0 sent=ok")
        XCTAssertEqual(GateProbe.line("x", [("error", "unknown probe: a b")]), "probe x error=unknown_probe:_a_b")
    }

    /// Off macOS the Apple checks say so instead of pretending to be granted.
    func testUnsupportedBackend() {
        let u = UnsupportedGateProbeBackend()
        XCTAssertEqual(line([gateProbeFlag, "ax"], backend: u), "probe ax bundle=ai.qrtz.echo module=echo trusted=false")
        XCTAssertEqual(line([gateProbeFlag, "mic"], backend: u), "probe mic bundle=ai.qrtz.echo module=echo status=unsupported")
        XCTAssertEqual(line([gateProbeFlag, "ae"], backend: u), "probe ae bundle=ai.qrtz.echo module=echo permission=unsupported sent=no")
    }

    /// The socket check is real on Linux too: a closed loopback port answers ECONNREFUSED (it reached the stack).
    func testSocketReachesLoopback() {
        let l = line([gateProbeFlag, "socket", "9"])
        XCTAssertTrue(l.hasPrefix("probe socket bundle=ai.qrtz.echo module=echo tcp="), l)
        XCTAssertTrue(l.contains("tcp=ECONNREFUSED"), l)
        XCTAssertTrue(l.contains(" udp="), l)
    }

    func testDefaultOutIsInTheHome() {
        XCTAssertEqual(GateProbe.defaultOut(home: "/Users/x/Library/Containers/ai.qrtz.echo/Data"),
                       "/Users/x/Library/Containers/ai.qrtz.echo/Data/qrtz-gate-probe.out")
    }
}
