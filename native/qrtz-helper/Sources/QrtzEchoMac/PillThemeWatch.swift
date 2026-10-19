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
public final class PillThemeWatch: @unchecked Sendable {
    private let source: DispatchSourceFileSystemObject
    private var last: PillTheme?

    public init?(path: String, initial: PillTheme?, onChange: @escaping (PillTheme) -> Void) {
        let dir = (path as NSString).deletingLastPathComponent
        let fd = open(dir, O_EVTONLY)
        guard fd >= 0 else { return nil }
        last = initial
        source = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .rename, .delete, .link],
                                                           queue: .main)
        source.setEventHandler { [weak self] in
            guard let self, let t = PillTheme.read(path: path), t != self.last else { return }
            self.last = t
            onChange(t)
        }
        source.setCancelHandler { close(fd) }
        source.resume()
    }

    deinit { source.cancel() }
}
#endif
