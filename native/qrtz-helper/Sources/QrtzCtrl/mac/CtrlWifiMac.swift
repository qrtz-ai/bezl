#if os(macOS)
// ADR 0191: "turn off my wifi", macOS side. The same switch the menu bar's Wi-Fi item flips, in the helper's own
// process: CoreWLAN's `setPower` on the default Wi-Fi interface. No process is spawned (no networksetup), no Apple
// Event, no key event; no network is joined, left or scanned, and nothing that needs Location (a network's name, the
// networks around) is read. The measure is the interface's `powerOn`.
import CoreWLAN
import Foundation

enum CtrlWifiMac {
    /// `wifi.none` with no Wi-Fi interface; `wifi.needs_admin` / `wifi.refused` when the system refuses the change.
    static func setPower(_ on: Bool) throws {
        guard let wifi = CWWiFiClient.shared().interface() else { throw CtrlFailure("wifi.none") }
        do { try wifi.setPower(on) } catch { throw CtrlFailure(CtrlWifi.failure(code: (error as NSError).code)) }
    }

    /// The radio's power; nil with no Wi-Fi interface.
    static func powerOn() -> Bool? {
        CWWiFiClient.shared().interface()?.powerOn()
    }
}
#endif
