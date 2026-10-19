#if os(macOS)
// ADR 0194: "keep my mac awake for 2 hours", macOS side. What `caffeinate -t` asks for, in the helper's own process
// (nothing spawned): one IOKit power assertion with a kernel timeout whose action releases it, so it never outlives the
// task's duration and ends with this process. No Apple Event, no key event, no "user activity" declared (that would
// wake the display), nothing on screen; no other app's assertions are listed or read. The measure is this one
// assertion's level.
import Foundation
import IOKit
import IOKit.pwr_mgt

enum CtrlKeepAwakeMac {
    /// `awake.refused` when IOKit won't make the assertion; `awake.no_timeout` (and released at once) when its timeout
    /// can't be set: never held without an end.
    static func hold(minutes: Int, display: Bool) throws -> IOPMAssertionID {
        guard let seconds = CtrlKeepAwake.seconds(minutes: minutes) else { throw CtrlFailure("awake.bad_minutes") }
        var id = IOPMAssertionID(0)
        guard IOPMAssertionCreateWithName(CtrlKeepAwake.type(display: display) as CFString,
                                          IOPMAssertionLevel(kIOPMAssertionLevelOn),
                                          CtrlKeepAwake.name as CFString, &id) == kIOReturnSuccess
        else { throw CtrlFailure("awake.refused") }
        let timed = IOPMAssertionSetProperty(id, CtrlKeepAwake.timeoutActionKey as CFString,
                                             CtrlKeepAwake.timeoutActionRelease as CFString) == kIOReturnSuccess
            && IOPMAssertionSetProperty(id, CtrlKeepAwake.timeoutKey as CFString, NSNumber(value: seconds)) == kIOReturnSuccess
        guard timed else {
            _ = IOPMAssertionRelease(id)
            throw CtrlFailure("awake.no_timeout")
        }
        return id
    }

    static func release(_ id: IOPMAssertionID) {
        _ = IOPMAssertionRelease(id)
    }

    /// IOKit's record of this one assertion; nil once it's released or timed out.
    static func properties(_ id: IOPMAssertionID) -> [String: Any]? {
        guard let d = IOPMAssertionCopyProperties(id)?.takeRetainedValue() else { return nil }
        return (d as NSDictionary) as? [String: Any]
    }
}
#endif
