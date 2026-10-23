import XCTest
@testable import QrtzEcho

final class FieldTextTests: XCTestCase {
    func testSlateEmptyFirstLineWithRealTextIsNotEmptyAndNotOverwritten() {
        let r = FieldText.classify(value: "\u{FEFF}\nreal text", placeholder: "")
        XCTAssertFalse(r.isEmpty)
        XCTAssertFalse(r.canOverwriteValue)
    }
    func testEmptySlateWithHintUsesNonOverwritePath() {
        let v = "\u{FEFF}\nGuide Devin while it works, or press  to queue"
        XCTAssertEqual((v as NSString).length, 48)
        let r = FieldText.classify(value: v, placeholder: "")
        XCTAssertFalse(r.canOverwriteValue)
    }
    func testBareBOMIsEmpty() {
        XCTAssertEqual(FieldText.classify(value: "\u{FEFF}", placeholder: nil), FieldText(canOverwriteValue: false, isEmpty: true))
    }
    func testNormalTextUnchanged() {
        XCTAssertEqual(FieldText.classify(value: "hello world", placeholder: nil), FieldText(canOverwriteValue: true, isEmpty: false))
        XCTAssertEqual(FieldText.classify(value: "", placeholder: nil), FieldText(canOverwriteValue: true, isEmpty: true))
    }
    func testPlaceholderMatchIsEmpty() {
        XCTAssertEqual(FieldText.classify(value: "\u{FEFF}\nAsk anything", placeholder: "Ask anything"),
                       FieldText(canOverwriteValue: false, isEmpty: true))
        XCTAssertEqual(FieldText.classify(value: "Ask anything", placeholder: "Ask anything"),
                       FieldText(canOverwriteValue: true, isEmpty: true))
    }
}
