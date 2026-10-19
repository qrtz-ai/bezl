// ADR 0176: "eject my disks". Foundation only, so the rules build and test on Linux; the Mac backend lists the mounted
// volumes (`FileManager.mountedVolumeURLs`, the mount table's resource values only, nothing on a volume read) and
// unmounts each with `NSWorkspace.unmountAndEjectDevice(at:)`, never forced.
import Foundation

public enum CtrlVolumes {
    /// The app-less measure `ctrl.measure {check: ejectable_volumes}`: how many volumes an eject would take.
    public static let check = "ejectable_volumes"

    /// What the helper reads about one mounted volume (resource values, never its contents).
    public struct Info: Equatable, Sendable {
        public var ejectable: Bool
        public var removable: Bool
        public var local: Bool
        public var root: Bool
        public init(ejectable: Bool, removable: Bool, local: Bool, root: Bool) {
            self.ejectable = ejectable; self.removable = removable; self.local = local; self.root = root
        }
    }

    /// USB / Thunderbolt disks, SD cards, optical discs and disk images: ejectable or removable, local, and never the
    /// root file system. Not an internal partition (neither flag), a network share (not local) or the startup disk.
    /// A value the system doesn't give counts against ejecting (unknown root = root).
    public static func ejectable(_ v: Info) -> Bool { (v.ejectable || v.removable) && v.local && !v.root }

    /// The volumes one act ejects. A singular task ("eject the usb drive") takes exactly one; with any other count the
    /// helper refuses before unmounting anything, so a disk plugged in after the host's read is never taken.
    public static func plan<T>(_ volumes: [T], single: Bool) throws -> [T] {
        if single && volumes.count != 1 { throw CtrlFailure("volumes.not_one") }
        return volumes
    }
}
