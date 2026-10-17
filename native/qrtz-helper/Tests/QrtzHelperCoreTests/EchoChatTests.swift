import XCTest
@testable import QrtzHelperCore

final class EchoChatTests: XCTestCase {
    func testEchoGatePolicy() {
        let ctx = MeetingContext()
        let g = EchoGate(context: ctx)
        XCTAssertEqual(g.target(systemRms: 0.001, micRms: 0.2), 1)  // far end silent
        XCTAssertEqual(g.target(systemRms: 0.2, micRms: 0.05), 0.316, accuracy: 0.001)  // unknown, echo-like
        XCTAssertEqual(g.target(systemRms: 0.05, micRms: 0.2), 1)  // unknown, user louder (double talk)
        ctx.update(selfName: "Jared Grimes", activeSpeaker: "Grace Hopper")
        XCTAssertEqual(g.target(systemRms: 0.2, micRms: 0.2), 0.1)
        ctx.update(selfName: nil, activeSpeaker: "jared grimes")
        XCTAssertEqual(g.target(systemRms: 0.2, micRms: 0.01), 1)
    }

    func testMixerDucksEchoedMic() {
        let ctx = MeetingContext()
        ctx.update(selfName: "Me", activeSpeaker: "Them")
        let m = DSP.Mixer(echo: EchoGate(context: ctx))
        _ = m.pushSystem([Float](repeating: 0.3, count: 1000))
        _ = m.pushMic([Float](repeating: 0.3, count: 1000))  // first block ramps 1 → 0.1
        _ = m.pushSystem([Float](repeating: 0.3, count: 1000))
        let out = m.pushMic([Float](repeating: 0.3, count: 1000))
        XCTAssertEqual(out[500], 0.33, accuracy: 0.001)  // 0.3 + 0.3 * 0.1
    }

    func testSelfNameAndChatScrape() {
        let win = AXNode(role: "AXWindow", children: [
            AXNode(role: "AXList", desc: "Participants", children: [
                AXNode(role: "AXListItem", children: [text("Jared Grimes (You)")]),
                AXNode(role: "AXListItem", children: [text("Grace Hopper")]),
            ]),
            group("In-call messages", [
                group(nil, [text("Grace Hopper"), text("10:02 AM"), text("deck is in the doc")]),
                group(nil, [text("link: go/launch")]),
                group(nil, [text("From Ada Lovelace to Everyone:"), text("+1")]),
            ]),
        ])
        let s = AXScraper.scrape(platform: .googleMeet, window: win)
        XCTAssertEqual(s.selfName, "Jared Grimes")
        XCTAssertEqual(s.chat.map(\.author), ["Grace Hopper", "Grace Hopper", "Ada Lovelace"])
        XCTAssertEqual(s.chat.map(\.text), ["deck is in the doc", "link: go/launch", "+1"])
    }

    func testChatDiffPrimesThenEmitsNew() {
        let d = ScrapeDiffer()
        var s = MeetingScrape(chat: [("A", "old")])
        XCTAssertEqual(d.diff(s, atMs: 0), [])
        s.chat.append(("B", "new"))
        XCTAssertEqual(d.diff(s, atMs: 10), [.axChat(atMs: 10, author: "B", text: "new")])
        XCTAssertEqual(d.diff(s, atMs: 20), [])
    }
}
