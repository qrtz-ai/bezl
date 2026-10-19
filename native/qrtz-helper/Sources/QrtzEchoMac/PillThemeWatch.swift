#if os(macOS)
import AppKit
import Foundation
import QrtzEcho

// ADR 0211: the pill's appearance comes from qrtz's theme setting, set explicitly on its panel.

public extension PillTheme {
    /// The panel's appearance: dark or light whatever macOS uses; nil (system) inherits macOS's.
    var appearance: NSAppearance? {
        switch self {
        case .dark: NSAppearance(named: .darkAqua)
        case .light: NSAppearance(named: .aqua)
        case .system: nil
        }
    }
}

/// Watches the folder of the host's theme file (the host replaces the file by a rename, so the folder is what
/// changes) and calls `onChange` on the main queue with each new theme. A file that doesn't parse changes nothing.
/// If the folder itself is deleted or renamed, the watch re-arms on the folder at that path (retrying a few times
/// while it is missing).
public final class PillThemeWatch: @unchecked Sendable {
    private var source: DispatchSourceFileSystemObject?
    private var last: PillTheme?
    private let path: String
    private let dir: String
    private let onChange: (PillTheme) -> Void

    public init?(path: String, initial: PillTheme?, onChange: @escaping (PillTheme) -> Void) {
        self.path = path
        dir = (path as NSString).deletingLastPathComponent
        self.onChange = onChange
        last = initial
        guard arm() else { return nil }
    }

    /// Open the folder and watch it; false when it can't be opened.
    @discardableResult
    private func arm() -> Bool {
        let fd = open(dir, O_EVTONLY)
        guard fd >= 0 else { return false }
        let src = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .rename, .delete, .link],
                                                            queue: .main)
        src.setEventHandler { [weak self, weak src] in
            guard let self, let src else { return }
            if !src.data.intersection([.delete, .rename]).isEmpty { self.rearm(tries: 10) }
            self.reread()
        }
        src.setCancelHandler { close(fd) }
        source = src
        src.resume()
        return true
    }

    private func reread() {
        guard let t = PillTheme.read(path: path), t != last else { return }
        last = t
        onChange(t)
    }

    /// The watched folder went away: drop the old watch and reopen the path, retrying every 0.5 s while missing.
    private func rearm(tries: Int) {
        source?.cancel()
        source = nil
        if arm() { reread(); return }
        guard tries > 0 else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in self?.rearm(tries: tries - 1) }
    }

    deinit { source?.cancel() }
}
#endif
