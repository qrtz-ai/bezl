// ctrl rung 1 (ADR 0100): plain file operations. Downloads cleanup needs no UI, no Accessibility and no Apple
// Events, only the Files and Folders grant, which the user gives in the foreground. Moves never overwrite; "delete"
// is a move to the Trash, so every file action has an inverse.
import Foundation

public enum CtrlFiles {
    public enum Failure: Error, Equatable {
        case noDirectory
        case exists
        case io(String)
    }

    /// Move `src` into directory `dir` under its own name. Returns the new URL.
    @discardableResult
    public static func moveInto(_ src: URL, _ dir: URL) throws -> URL {
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: dir.path, isDirectory: &isDir), isDir.boolValue else { throw Failure.noDirectory }
        let dst = dir.appendingPathComponent(src.lastPathComponent)
        if (try? FileManager.default.attributesOfItem(atPath: dst.path)) != nil { throw Failure.exists }
        do { try FileManager.default.moveItem(at: src, to: dst) } catch { throw Failure.io(String(describing: type(of: error))) }
        return dst
    }

    /// Move `src` to the Trash: the user's (macOS `FileManager.trashItem`), or `into` when given (tests, Linux).
    @discardableResult
    public static func trash(_ src: URL, into: URL? = nil) throws -> URL {
        if let into {
            try FileManager.default.createDirectory(at: into, withIntermediateDirectories: true)
            return try moveInto(src, into)
        }
        #if os(macOS)
        var out: NSURL?
        do { try FileManager.default.trashItem(at: src, resultingItemURL: &out) } catch { throw Failure.io("trash") }
        return (out as URL?) ?? src
        #else
        throw Failure.io("no system trash")
        #endif
    }
}
