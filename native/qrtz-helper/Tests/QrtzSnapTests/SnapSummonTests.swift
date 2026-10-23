import XCTest
@testable import QrtzSnap

final class SnapSummonTests: XCTestCase {
    typealias F = SummonChordFilter
    let cmd: UInt64 = 0x10_0000
    var l: UInt64 { cmd | F.leftCmdDevice }
    var r: UInt64 { cmd | F.rightCmdDevice }
    var both: UInt64 { cmd | F.leftCmdDevice | F.rightCmdDevice }

    func flags(_ f: inout F, _ code: Int64, _ flags: UInt64, hid: Bool = true) -> Bool {
        f.handle(type: F.flagsChanged, keyCode: code, flags: flags, sourceState: hid ? F.hidSystemState : 0)
    }

    func testChordFiresOnceWhenItEnds() {
        var f = F()
        XCTAssertFalse(flags(&f, 55, l))
        XCTAssertFalse(flags(&f, 54, both), "nothing until the chord ends")
        XCTAssertTrue(flags(&f, 55, r), "first ⌘ let go: summon")
        XCTAssertFalse(flags(&f, 54, 0))
        // Right first works too.
        XCTAssertFalse(flags(&f, 54, r))
        XCTAssertFalse(flags(&f, 55, both))
        XCTAssertTrue(flags(&f, 54, l))
        XCTAssertFalse(flags(&f, 55, 0))
    }

    func testTappingOneCommandWhileTheOtherIsHeldToggles() {
        var f = F()
        _ = flags(&f, 55, l); _ = flags(&f, 54, both)
        XCTAssertTrue(flags(&f, 54, l))
        // Right ⌘ again while left is still held: a new chord, a second summon (back again).
        XCTAssertFalse(flags(&f, 54, both))
        XCTAssertTrue(flags(&f, 54, l))
        XCTAssertFalse(flags(&f, 55, 0), "letting go of the held ⌘ fires nothing")
        // A spoiled chord stays spoiled until both are up.
        _ = flags(&f, 55, l); _ = flags(&f, 54, both)
        _ = f.handle(type: F.keyDown, keyCode: 8, flags: both)
        XCTAssertFalse(flags(&f, 54, l))
        XCTAssertFalse(flags(&f, 54, both))
        XCTAssertFalse(flags(&f, 54, l))
    }

    func testSingleCommandNeverFires() {
        var f = F()
        XCTAssertFalse(flags(&f, 55, l)); XCTAssertFalse(flags(&f, 55, 0))
        XCTAssertFalse(flags(&f, 54, r)); XCTAssertFalse(flags(&f, 54, 0))
    }

    func testCommandKeyCombosDontFire() {
        var f = F()
        _ = flags(&f, 55, l); _ = flags(&f, 54, both)
        XCTAssertFalse(f.handle(type: F.keyDown, keyCode: 8, flags: both), "⌘C")
        XCTAssertFalse(flags(&f, 55, r)); XCTAssertFalse(flags(&f, 54, 0))
        // A key before the second ⌘.
        _ = flags(&f, 55, l)
        _ = f.handle(type: F.keyDown, keyCode: 9, flags: l)
        _ = flags(&f, 54, both)
        XCTAssertFalse(flags(&f, 55, r)); _ = flags(&f, 54, 0)
        // Another modifier (shift) joins.
        _ = flags(&f, 55, l); _ = flags(&f, 54, both)
        _ = flags(&f, 56, both | 0x2_0000)
        XCTAssertFalse(flags(&f, 55, r | 0x2_0000))
        _ = flags(&f, 56, r); _ = flags(&f, 54, 0)
        // Clean chord after all that still works.
        _ = flags(&f, 55, l); _ = flags(&f, 54, both)
        XCTAssertTrue(flags(&f, 55, r))
    }

    func testSyntheticEventsAreIgnored() {
        var f = F()
        XCTAssertFalse(flags(&f, 55, l, hid: false))
        XCTAssertFalse(flags(&f, 54, both, hid: false))
        XCTAssertFalse(flags(&f, 55, r, hid: false))
        XCTAssertFalse(f.armed)
    }

    func testResetForgetsAHeldChord() {
        var f = F()
        _ = flags(&f, 55, l); _ = flags(&f, 54, both)
        f.reset()
        XCTAssertFalse(flags(&f, 55, r))
    }

    func testStepSummonsThenGoesBack() {
        XCTAssertEqual(SnapSummon.step(frontmost: "com.apple.Safari", chosen: "com.openai.chat", previous: nil),
                       .summon(previous: "com.apple.Safari"))
        XCTAssertEqual(SnapSummon.step(frontmost: "com.openai.chat", chosen: "com.openai.chat", previous: "com.apple.Safari"),
                       .dismiss(back: "com.apple.Safari"))
        XCTAssertEqual(SnapSummon.step(frontmost: nil, chosen: "com.openai.chat", previous: "x"), .summon(previous: nil))
    }

    func testConfigureCarriesSummonAndStatusReportsIt() {
        let c = SnapConfig(json: ["summon": ["enabled": true, "bundleId": " com.openai.chat ", "echoKey": "right-command"]])
        XCTAssertEqual(c.summon, SnapSummonConfig(enabled: true, bundleId: "com.openai.chat", echoKey: "right-command"))
        XCTAssertTrue(c.summon.clashesWithEcho)
        XCTAssertEqual(SnapConfig(json: [:]).summon, SnapSummonConfig())

        let line = SnapLines.status(axTrusted: true, registered: [:], refused: [:],
                                    summon: SnapSummonStatus(inputMonitoring: true, active: true,
                                                             apps: [SnapApp(bundleId: "a.b", name: "AB")]))
        let o = (try! JSONSerialization.jsonObject(with: Data(line.utf8))) as! [String: Any]
        XCTAssertEqual(o["inputMonitoring"] as? Bool, true)
        XCTAssertEqual(o["summonActive"] as? Bool, true)
        XCTAssertEqual(o["apps"] as? [[String: String]], [["bundleId": "a.b", "name": "AB"]])
        let off = (try! JSONSerialization.jsonObject(with: Data(SnapLines.status(axTrusted: true, registered: [:], refused: [:]).utf8))) as! [String: Any]
        XCTAssertNil(off["apps"])
    }

    func testServerAsksForAccessOnlyWhenSummonTurnsOn() {
        final class B: SnapBackend {
            var axTrusted = true
            var asks: [Bool] = []
            func register(_ s: [SnapAction: SnapShortcut]) -> [SnapAction: String] { [:] }
            func perform(_ a: SnapAction) -> SnapOutcome { .failed("") }
            func setDisabledApps(_ apps: Set<String>) {}
            func setStatusItem(_ shown: Bool) {}
            func setSummon(_ c: SnapSummonConfig, requestAccess: Bool) { asks.append(requestAccess) }
        }
        let b = B()
        let s = SnapServer(backend: b, write: { _ in })
        s.configure(SnapConfig())
        var on = SnapConfig(); on.summon.enabled = true
        s.configure(on)
        s.configure(on)
        XCTAssertEqual(b.asks, [false, true, false])
    }

    func testOnlyInstalledAppsAreAllowed() {
        let apps = [SnapApp(bundleId: "com.openai.chat", name: "ChatGPT")]
        XCTAssertTrue(SnapSummon.allowed("com.openai.chat", installed: apps))
        XCTAssertFalse(SnapSummon.allowed("com.evil.payload", installed: apps))
        XCTAssertFalse(SnapSummon.allowed("", installed: apps))
        XCTAssertFalse(SnapSummon.allowed("com.openai.chat", installed: []))
    }

    /// Echo's talk key is right ⌘: right ⌘ first (dictation starts in Echo), then left ⌘. The chord must not
    /// summon, in either order, and a reset keeps it blocked.
    func testClashBlocksTheChordInEitherOrder() {
        var f = F(blocked: true)
        XCTAssertFalse(flags(&f, 54, r))
        XCTAssertFalse(flags(&f, 55, both))
        XCTAssertFalse(flags(&f, 54, l))
        XCTAssertFalse(flags(&f, 55, 0))
        XCTAssertFalse(flags(&f, 55, l)); XCTAssertFalse(flags(&f, 54, both)); XCTAssertFalse(flags(&f, 55, r))
        _ = flags(&f, 54, 0)
        f.reset()
        XCTAssertTrue(f.blocked)
        // Unblocked, the same right-then-left order fires.
        var g = F()
        _ = flags(&g, 54, r); _ = flags(&g, 55, both)
        XCTAssertTrue(flags(&g, 54, l))
    }
}
