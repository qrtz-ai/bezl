#if os(macOS)
// ADR 0183: "put my mac to sleep", macOS side. The same request `pmset sleepnow` makes, in the helper's own process:
// `IOPMSleepSystem` on the root power domain's connection. No process is spawned, no Apple Event is sent, no key
// event; nothing is quit or closed (the system sends apps its own will-sleep notice) and any key, the mouse or the lid
// wakes it. The measure is `kern.sleeptime` (when the Mac last went to sleep), read with sysctlbyname in this process.
import Foundation
import IOKit
import IOKit.pwr_mgt

enum CtrlSystemSleepMac {
    /// `sleep.no_power_manager` when the connection can't be opened, `sleep.refused` when the request fails.
    static func sleepNow() throws {
        let pm = IOPMFindPowerManagement(kIOMainPortDefault)
        guard pm != 0 else { throw CtrlFailure("sleep.no_power_manager") }
        defer { IOServiceClose(pm) }
        guard IOPMSleepSystem(pm) == kIOReturnSuccess else { throw CtrlFailure("sleep.refused") }
    }

    /// Whole seconds since 1970 of the last sleep (0: not since boot); nil if the sysctl can't be read.
    static func lastSleep() -> Int? {
        var tv = timeval()
        var size = MemoryLayout<timeval>.size
        guard sysctlbyname(CtrlSystemSleep.sleepTimeName, &tv, &size, nil, 0) == 0 else { return nil }
        return Int(tv.tv_sec)
    }
}
#endif
