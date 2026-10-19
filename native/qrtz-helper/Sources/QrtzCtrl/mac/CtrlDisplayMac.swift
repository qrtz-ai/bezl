#if os(macOS)
// ADR 0180: "turn off my display", macOS side. The same registry write `pmset displaysleepnow` makes, in the helper's
// own process: `IORequestIdle = true` on the display wrangler. No process is spawned, no Apple Event is sent, no key
// event; the Mac stays awake and nothing is locked (any key or the mouse wakes the display). The measure is
// `CGDisplayIsAsleep` of the main display.
import CoreGraphics
import Foundation
import IOKit

enum CtrlDisplayMac {
    /// `display.no_wrangler` when the registry has no display wrangler, `display.refused` when the write fails.
    static func sleepNow() throws {
        let wrangler = IORegistryEntryFromPath(kIOMainPortDefault, CtrlDisplay.wranglerPath)
        guard wrangler != 0 else { throw CtrlFailure("display.no_wrangler") }
        defer { IOObjectRelease(wrangler) }
        let r = IORegistryEntrySetCFProperty(wrangler, CtrlDisplay.requestIdle as CFString, kCFBooleanTrue)
        guard r == KERN_SUCCESS else { throw CtrlFailure("display.refused") }
    }

    static func asleep() -> Bool {
        CGDisplayIsAsleep(CGMainDisplayID()) != 0
    }
}
#endif
