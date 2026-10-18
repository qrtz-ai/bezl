import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

func text(_ s: String) -> AXNode { AXNode(role: "AXStaticText", value: s) }
func group(_ desc: String? = nil, _ c: [AXNode]) -> AXNode { AXNode(role: "AXGroup", desc: desc, children: c) }

final class AXScrapeTests: XCTestCase {
    func testCleanName() {
        XCTAssertEqual(AXScraper.cleanName("Jared Grimes (Host, me)"), "Jared Grimes")
        XCTAssertEqual(AXScraper.cleanName("Ada Lovelace, muted"), "Ada Lovelace")
        XCTAssertEqual(AXScraper.cleanName("Grace Hopper is speaking"), "Grace Hopper")
    }

    /// Shape modelled on Google Meet (Chrome): People panel list, speaking tile description, captions region.
    func testMeetShape() {
        let win = AXNode(role: "AXWindow", title: "Meet - Weekly Product Sync", children: [
            AXNode(role: "AXWebArea", children: [
                group(nil, [AXNode(role: "AXButton", desc: "Grace Hopper is speaking")]),
                AXNode(role: "AXList", desc: "Participants", children: [
                    AXNode(role: "AXListItem", children: [text("Jared Grimes (You)"), text("Meeting host")]),
                    AXNode(role: "AXListItem", children: [text("Grace Hopper")]),
                    AXNode(role: "AXListItem", children: [text("Ada Lovelace")]),
                ]),
                group("Captions", [
                    group(nil, [text("Grace Hopper"), text("Shipping Friday.")]),
                    group(nil, [text("Ada Lovelace"), text("I'll take the release notes")]),
                ]),
            ]),
        ])
        let s = AXScraper.scrape(platform: .googleMeet, window: win)
        XCTAssertEqual(s.participants, ["Jared Grimes", "Grace Hopper", "Ada Lovelace"])
        XCTAssertEqual(s.activeSpeaker, "Grace Hopper")
        XCTAssertEqual(s.captions.map(\.speaker), ["Grace Hopper", "Ada Lovelace"])
    }

    /// Shape modelled on Zoom.app: participants outline, "Name: text" caption lines.
    func testZoomShape() {
        let win = AXNode(role: "AXWindow", title: "Zoom Meeting", children: [
            AXNode(role: "AXOutline", title: "Participants (2)", children: [
                AXNode(role: "AXRow", subrole: "AXOutlineRow", children: [text("Jared Grimes (Host, me)")]),
                AXNode(role: "AXRow", subrole: "AXOutlineRow", children: [text("Ada Lovelace")]),
            ]),
            group("Live Transcript", [text("Ada Lovelace: hello there"), text("and welcome")]),
        ])
        let s = AXScraper.scrape(platform: .zoom, window: win)
        XCTAssertEqual(s.participants, ["Jared Grimes", "Ada Lovelace"])
        XCTAssertNil(s.activeSpeaker)
        XCTAssertEqual(s.captions.count, 2)
        XCTAssertEqual(s.captions[1].speaker, "Ada Lovelace")
    }

    func testDifferEmitsOnlyChangesAndSettledCaptions() {
        let d = ScrapeDiffer(captionSettleMs: 1000)
        var s = MeetingScrape(participants: ["A"], activeSpeaker: "A", captions: [("A", "hel")])
        XCTAssertEqual(d.diff(s, atMs: 0), [.axParticipants(names: ["A"]), .axActiveSpeaker(atMs: 0, name: "A")])
        s.captions = [("A", "hello.")]
        XCTAssertEqual(d.diff(s, atMs: 200), [])
        XCTAssertEqual(d.diff(s, atMs: 1300), [.axCaption(atMs: 1300, speaker: "A", text: "hello.")])
        s.captions = [("A", "hello."), ("B", "hi")]
        s.activeSpeaker = "B"
        XCTAssertEqual(d.diff(s, atMs: 1400), [.axActiveSpeaker(atMs: 1400, name: "B")])
    }
}
