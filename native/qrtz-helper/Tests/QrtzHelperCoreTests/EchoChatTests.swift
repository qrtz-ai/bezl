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

final class FakeAEC: MicEnhancer, @unchecked Sendable {
    let name = "fake-aec"
    var failAfter: Int
    var calls = 0
    var held: [Float] = []
    init(failAfter: Int = .max) { self.failAfter = failAfter }
    func prepare() async throws {}
    func reset() async { held = [] }
    /// Removes the echo perfectly but delays output by 4 samples, like a real streaming AEC.
    func enhance(mic: [Float], reference: [Float]) async throws -> [Float] {
        calls += 1
        if calls > failAfter { throw ProtocolError("model error") }
        held += zip(mic, reference).map { $0 - $1 }
        let n = max(0, held.count - 4)
        defer { held.removeFirst(n) }
        return Array(held[0..<n])
    }
}

final class MicPipelineTests: XCTestCase {
    func testAecRemovesEchoAndKeepsAlignment() async {
        let p = MicPipeline(gate: EchoGate(context: MeetingContext()), enhancer: FakeAEC())
        await p.start()
        XCTAssertEqual(p.mode, "aec:fake-aec")
        // mic = echo of system (0.2) + user voice (0.1)
        var total = 0
        for _ in 0..<3 {
            let out = await p.process(CaptureBlock(system: [Float](repeating: 0.2, count: 10), mic: [Float](repeating: 0.3, count: 10)))
            for v in out { XCTAssertEqual(v, 0.3, accuracy: 0.0001) }  // system 0.2 + clean voice 0.1, no doubled echo
            total += out.count
        }
        XCTAssertEqual(total + p.finish().count, 30)  // nothing lost
    }

    func testAecFailureFallsBackToGate() async {
        let logs = Box<[String]>([])
        let p = MicPipeline(gate: EchoGate(context: MeetingContext()), enhancer: FakeAEC(failAfter: 1))
        p.onLog = { _, m in logs.mutate { $0.append(m) } }
        await p.start()
        _ = await p.process(CaptureBlock(system: [Float](repeating: 0.2, count: 10), mic: [Float](repeating: 0.3, count: 10)))
        let out = await p.process(CaptureBlock(system: [Float](repeating: 0.2, count: 10), mic: [Float](repeating: 0.3, count: 10)))
        XCTAssertEqual(p.mode, "gate")
        XCTAssertEqual(out.count, 4 + 10)  // the 4 held system samples are released, then the gated block
        XCTAssertTrue(logs.value.contains { $0.contains("falling back to echo gate") })
    }

    func testNoMicPassesSystemThrough() async {
        let p = MicPipeline(gate: EchoGate(context: MeetingContext()), enhancer: nil)
        await p.start()
        let out = await p.process(CaptureBlock(system: [0.1, 0.2], mic: nil))
        XCTAssertEqual(out, [0.1, 0.2])
    }

    func testAligner() {
        let a = DSP.Aligner(maxSkewFrames: 4)
        XCTAssertEqual(a.pushSystem([1, 1]), [])
        XCTAssertEqual(a.pushMic([2]), [CaptureBlock(system: [1], mic: [2])])
        XCTAssertEqual(a.pushSystem([1, 1, 1, 1]), [CaptureBlock(system: [1, 1, 1, 1, 1], mic: nil)])
    }
}
