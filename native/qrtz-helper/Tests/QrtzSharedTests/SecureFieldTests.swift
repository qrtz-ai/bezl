import XCTest
@testable import QrtzShared

/// ADR 0224: the one secure-field rule (ctrl's tree reader and echo's insertion both use it).
final class SecureFieldTests: XCTestCase {
    func testRoleOrSubroleMarksAPasswordField() {
        XCTAssertTrue(SecureField.isSecure(role: "AXSecureTextField", subrole: nil))
        XCTAssertTrue(SecureField.isSecure(role: "AXTextField", subrole: "AXSecureTextField"), "a text field with the secure subrole")
        XCTAssertTrue(SecureField.isSecure(role: nil, subrole: "AXSecureTextField"))
        XCTAssertFalse(SecureField.isSecure(role: "AXTextField", subrole: nil))
        XCTAssertFalse(SecureField.isSecure(role: "AXTextArea", subrole: "AXSearchField"))
        XCTAssertFalse(SecureField.isSecure(role: nil, subrole: nil))
        XCTAssertFalse(SecureField.isSecure(role: "axsecuretextfield", subrole: nil), "AX roles are exact")
    }

    func testTheRefusalCodeMatchesTheHost() {
        XCTAssertEqual(SecureField.insertRefused, "secure-field")  // qrtz-talk INSERT_REFUSED_SECURE
    }
}
