// ADR 0166: "open my downloads", the plain-data half (Foundation-only, tested on Linux). The folder is a closed set
// (Rust `UserFolder`, snake_case), and the helper resolves it from the system's own location
// (`FileManager.urls(for:in:)`), never from a path the host, a model or the task sent. Nothing here reads the folder:
// no listing, no stat, no `displayName` (that can look inside for `.localized`, and Desktop / Documents / Downloads
// contents are TCC-protected). The Mac backend opens the URL in the running Finder with `activates = false` and checks
// the Finder's AX windows only when AX is already trusted.
import Foundation

/// The folders ctrl can show. Mirrors Rust `UserFolder`; anything else is a bad request.
public enum CtrlFolder: String, CaseIterable, Sendable {
    case downloads, desktop, documents
}

public enum CtrlFolders {
    /// The one app that shows a folder.
    public static let finder = "com.apple.finder"
    /// `ctrl.measure {check: folder_open, folder}` reaches the backend as `folder_open.<folder>` for the Finder, so
    /// `CtrlBackend.measure(check:app:)` keeps its shape.
    public static let measurePrefix = "folder_open."

    /// Exactly one of the enum's spellings (no trimming, no case folding, no paths).
    public static func folder(_ raw: String) -> CtrlFolder? { CtrlFolder(rawValue: raw) }

    /// The folder a `folder_open.<folder>` check names, or nil.
    public static func measured(_ check: String) -> CtrlFolder? {
        guard check.hasPrefix(measurePrefix) else { return nil }
        return folder(String(check.dropFirst(measurePrefix.count)))
    }

    static func directory(_ f: CtrlFolder) -> FileManager.SearchPathDirectory {
        switch f {
        case .downloads: return .downloadsDirectory
        case .desktop: return .desktopDirectory
        case .documents: return .documentDirectory
        }
    }

    /// The folder's location in the user's domain as the system names it (no file access), or nil when it names none.
    public static func url(_ f: CtrlFolder, fileManager: FileManager = .default) -> URL? {
        guard let u = fileManager.urls(for: directory(f), in: .userDomainMask).first, u.isFileURL else { return nil }
        return u.standardizedFileURL
    }

    /// Whether the Finder shows `folder` (at `path`), from its windows' AX `document` URLs and titles. 0 only when it
    /// has no windows at all; 1 when a window's document is that folder or its title is the folder's own name; nil
    /// otherwise (a localized title, or the folder in a tab behind the front one, can't be told from "not shown").
    public static func shown(windows: Int, documents: [String], titles: [String], path: String) -> Int? {
        if windows == 0 { return 0 }
        let target = URL(fileURLWithPath: path).standardizedFileURL.path
        if documents.contains(where: { URL(string: $0).map { $0.isFileURL && $0.standardizedFileURL.path == target } ?? false }) {
            return 1
        }
        let name = URL(fileURLWithPath: target).lastPathComponent
        return !name.isEmpty && titles.contains(name) ? 1 : nil
    }
}
