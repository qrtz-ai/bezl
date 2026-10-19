// ADR 0225: the one secure-field rule, shared by ctrl's tree reader (CtrlTreeRules) and echo's text insertion
// (TextInserter / MacInsertion). Foundation-only, so it builds and tests on Linux.

/// An accessibility element is a password field when its role or its subrole is `AXSecureTextField` (a plain
/// `AXTextField` with the secure subrole counts too). Such a field is never read past its role and never typed into.
public enum SecureField {
    public static let roles: Set<String> = ["AXSecureTextField"]
    public static let subroles: Set<String> = ["AXSecureTextField"]

    public static func isSecure(role: String?, subrole: String?) -> Bool {
        role.map(roles.contains) == true || subrole.map(subroles.contains) == true
    }

    /// What echo's helper answers in `insert.result.error` when it refused to type into a secure field (or while
    /// secure event input is on). qrtz-talk matches it (`INSERT_REFUSED_SECURE`) and keeps nothing of the dictation.
    public static let insertRefused = "secure-field"

    /// The pill's notice for that refusal (qrtz-talk `MSG_SECURE_FIELD`; both tests pin the same words).
    public static let notice = "echo won't type into password fields"
}
