// ADR 0187: the minimize probe's rules (Foundation, so on Linux), the policy that refuses the write both ways until a
// clean probe, and static checks that only the opt-in probe ever writes `AXMinimized` and the helper never calls it.
import Foundation
import XCTest
@testable import QrtzCtrl

private let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
private let repo = root.deletingLastPathComponent().deletingLastPathComponent()

private func outcome(_ violations: [GuardField] = [], inconclusive: [GuardField] = [], userInput: Bool = false) -> GuardOutcome {
    var o = GuardOutcome()
    o.violations = violations
    o.inconclusive = inconclusive
    o.userInput = userInput
    return o
}

private let probe: Int32 = 400, user: Int32 = 100

private func round(minimize: GuardOutcome = outcome(), restore: GuardOutcome = outcome(), minimized: Bool = true,
                   restored: Bool = true, frontBefore: Int32? = user, frontAfter: Int32? = user) -> CtrlMinimizeProbe.Round {
    CtrlMinimizeProbe.Round(minimize: minimize, restore: restore, minimizedAfterWrite: minimized, restoredAfterWrite: restored,
                            frontOwnerBefore: frontBefore, frontOwnerAfterRestore: frontAfter, probePid: probe)
}

final class CtrlMinimizeProbeTests: XCTestCase {
    func testARoundThatChangesNothingIsClean() {
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round()), .clean)
        // Focus unread (nil both times) is fine: frontmost and the front window are what a raise would change.
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(restore: outcome(inconclusive: [.focusedElement]))), .clean)
    }

    func testTheRestoredWindowOverTheUsersIsRaisedAndActivationBeatsEverything() {
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(restore: outcome([.keyWindow]), frontAfter: probe)), .raised)
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(frontAfter: probe)), .raised, "front owner alone is enough")
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(restore: outcome([.keyWindow, .frontmostApp]), frontAfter: probe)), .activated)
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(minimize: outcome([.frontmostApp]))), .activated)
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(restore: outcome([.focusedElement]))), .focusMoved)
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(minimize: outcome([.cursor]))), .focusMoved)
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(minimize: outcome([.keyWindow]))), .frontWindowChanged)
    }

    func testUserInputUnreadableFieldsAndAFrontProbeWindowAreNoVerdict() {
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(restore: outcome(userInput: true))), .inconclusive)
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(minimize: outcome(inconclusive: [.frontmostApp]))), .inconclusive)
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(restore: outcome(inconclusive: [.keyWindow]))), .inconclusive)
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(frontAfter: nil)), .inconclusive)
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(frontBefore: probe, frontAfter: probe)), .inconclusive,
                       "the probe window was in front already: a raise couldn't show")
    }

    func testAWriteThatDidntTakeIsNoEffect() {
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(minimized: false)), .noEffect)
        XCTAssertEqual(CtrlMinimizeProbe.verdict(round(restored: false)), .noEffect)
    }

    func testOnlyEnoughCleanRoundsLiftTheHold() {
        XCTAssertEqual(CtrlMinimizeProbe.roundsNeeded, 3)
        XCTAssertTrue(CtrlMinimizeProbe.liftsHold([.clean, .clean, .clean]))
        XCTAssertFalse(CtrlMinimizeProbe.liftsHold([.clean, .clean]))
        XCTAssertFalse(CtrlMinimizeProbe.liftsHold([.clean, .clean, .inconclusive]))
        XCTAssertFalse(CtrlMinimizeProbe.liftsHold([.clean, .raised, .clean, .clean]))
        XCTAssertEqual(CtrlMinimizeProbe.report([.clean, .raised], os: "Version 27.0"),
                       "[ctrl-minimize] os=Version 27.0 rounds=2 verdicts=clean,raised lifts_hold=false")
    }

    func testThePolicyRefusesTheWriteBothWaysUntilAnAmendment() {
        XCTAssertEqual(CtrlMinimizeProbe.attribute, "AXMinimized")
        XCTAssertTrue(CtrlPolicy.forbiddenAXSetAttributes.contains("AXMinimized"))
        for role in ["AXWindow", "AXStandardWindow"] {
            XCTAssertThrowsError(try CtrlPolicy.checkAXSet("AXMinimized", role: role)) { e in
                XCTAssertEqual(e as? CtrlPolicy.Refusal, .forbiddenAttribute("AXMinimized"))
            }
        }
    }

    /// Only the opt-in probe writes the attribute; nothing the helper runs (the server, the backend, the actions) calls
    /// the probe, and the read-only Mac check only reads counts.
    func testOnlyTheOptInProbeEverWritesAXMinimized() throws {
        let fm = FileManager.default
        let dir = root.appendingPathComponent("Sources/QrtzCtrl")
        let files = (fm.enumerator(at: dir, includingPropertiesForKeys: nil)?.compactMap { $0 as? URL } ?? []).filter { $0.pathExtension == "swift" }
        XCTAssertFalse(files.isEmpty)
        var writers: [String] = [], callers: [String] = []
        for f in files {
            let src = try String(contentsOf: f, encoding: .utf8)
            let name = f.lastPathComponent
            if src.contains("AXUIElementSetAttributeValue"), src.contains("CtrlMinimizeProbe.attribute") || src.contains("kAXMinimizedAttribute as CFString") {
                writers.append(name)
            }
            if src.contains("CtrlMinimizeProbeMac.rounds") { callers.append(name) }
        }
        XCTAssertEqual(writers, ["CtrlMinimizeProbeMac.swift"])
        XCTAssertEqual(callers, [], "the helper never runs the active probe")
        let probeSrc = try String(contentsOf: dir.appendingPathComponent("mac/CtrlMinimizeProbeMac.swift"), encoding: .utf8)
        XCTAssertFalse(probeSrc.contains("Process("), "nothing spawned")
        XCTAssertFalse(probeSrc.contains("activate("), "nothing activated")
        let check = try String(contentsOf: repo.appendingPathComponent("scripts/ctrl/mac-check/main.swift"), encoding: .utf8)
        XCTAssertTrue(check.contains("CtrlMinimizeProbeMac.minimizedCount"))
        XCTAssertFalse(check.contains("CtrlMinimizeProbeMac.rounds"), "the Mac check stays read-only")
    }
}
