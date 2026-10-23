import XCTest
@testable import QrtzEcho

final class FieldTextTests: XCTestCase {
    func testEmptySlateWithHintIsEmpty() {
        let v = "\u{FEFF}\nGuide Devin while it works, or press  to queue"
        XCTAssertEqual((v as NSString).length, 48)
        XCTAssertEqual(FieldText.normalize(value: v, location: 1, length: 0), FieldText(value: "", location: 0, length: 0))
    }
    func testBareBOMIsEmpty() {
        XCTAssertEqual(FieldText.normalize(value: "\u{FEFF}", location: 1, length: 0), FieldText(value: "", location: 0, length: 0))
    }
    func testNormalTextUnchanged() {
        XCTAssertEqual(FieldText.normalize(value: "hello world", location: 5, length: 3), FieldText(value: "hello world", location: 5, length: 3))
    }
    func testBOMMidStringStrippedAndOffsetsShift() {
        // "ab\u{FEFF}cd\u{FEFF}ef", caret selecting "d\u{FEFF}e" (3..<6 in the cleaned frame → 2..<4)
        let r = FieldText.normalize(value: "ab\u{FEFF}cd\u{FEFF}ef", location: 4, length: 3)
        XCTAssertEqual(r, FieldText(value: "abcdef", location: 3, length: 2))
    }
    func testCaretAtZeroBeforeRealTextKept() {
        XCTAssertEqual(FieldText.normalize(value: "hello", location: 0, length: 0), FieldText(value: "hello", location: 0, length: 0))
    }
}
