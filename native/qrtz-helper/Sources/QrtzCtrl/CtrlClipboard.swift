// ADR 0168: "clear my clipboard". Foundation only, so the verify rule builds and tests on Linux; the Mac backend
// calls `NSPasteboard.general.clearContents()` and keeps the change count it returns.
import Foundation

public enum CtrlClipboard {
    /// The app-less measure `ctrl.measure {check: clipboard_cleared}`.
    public static let check = "clipboard_cleared"

    /// 1 when the pasteboard's change count is still the one ctrl's last clear returned (nothing copied since), 0
    /// when it moved or ctrl never cleared it. Only the counter is compared: nothing on the clipboard (no item, no
    /// type) is ever read, so the check can't see what the user copied, and can't trigger the paste privacy prompt.
    public static func cleared(changeCount: Int, lastClear: Int?) -> Int {
        guard let last = lastClear else { return 0 }
        return changeCount == last ? 1 : 0
    }
}
