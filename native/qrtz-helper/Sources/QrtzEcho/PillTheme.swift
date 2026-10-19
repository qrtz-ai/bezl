import Foundation
#if canImport(Glibc)
import Glibc
#elseif canImport(Darwin)
import Darwin
#endif

// ADR 0211: the pill follows qrtz's own theme setting (`theme`: dark, light or system), not the macOS appearance.
// The host writes the setting as one word to a small file of its own (`<data>/pill/theme`, named by
// `QRTZ_PILL_THEME_FILE`) before it starts the helper and on every change; the native pill reads it when it is built
// and again whenever that folder changes (QrtzEchoMac `PillThemeWatch`). Nothing here is on the dictation path.

/// qrtz's theme setting, as the pill draws it. `system` follows macOS.
public enum PillTheme: String, Sendable, Equatable, CaseIterable {
    case dark, light, system

    /// The environment variable naming the file the host keeps the theme in.
    public static let fileEnv = "QRTZ_PILL_THEME_FILE"
    /// The file holds one word; anything longer is not a theme.
    static let maxBytes = 16

    /// One of the three words (surrounding whitespace allowed), nothing else.
    public static func parse(_ s: String) -> PillTheme? {
        PillTheme(rawValue: s.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    /// The theme in `path`; nil when the file is missing, unreadable, too long, not one of the three words, or not
    /// a regular file (a symlink is never followed and a FIFO never blocks: O_NOFOLLOW | O_NONBLOCK, then fstat).
    public static func read(path: String) -> PillTheme? {
        let fd = open(path, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
        guard fd >= 0 else { return nil }
        defer { close(fd) }
        var st = stat()
        guard fstat(fd, &st) == 0, (st.st_mode & S_IFMT) == S_IFREG else { return nil }
        let cap = maxBytes + 1
        var buf = [UInt8](repeating: 0, count: cap)
        var n = 0
        while n < cap {
            let r = buf.withUnsafeMutableBytes { sysRead(fd, $0.baseAddress! + n, cap - n) }
            if r < 0, errno == EINTR { continue }
            guard r >= 0 else { return nil }
            if r == 0 { break }
            n += r
        }
        guard n <= maxBytes, let s = String(bytes: buf[0..<n], encoding: .utf8) else { return nil }
        return parse(s)
    }

    /// The file the host named, if it did.
    public static func file(_ env: [String: String] = ProcessInfo.processInfo.environment) -> String? {
        env[fileEnv].flatMap { $0.isEmpty ? nil : $0 }
    }

    /// The host's theme at launch; nil (no file named, or none written yet): the pill follows macOS, as before.
    public static func fromEnvironment(_ env: [String: String] = ProcessInfo.processInfo.environment) -> PillTheme? {
        file(env).flatMap { read(path: $0) }
    }
}

/// read(2), outside `PillTheme` (whose `read(path:)` would shadow it).
private func sysRead(_ fd: Int32, _ p: UnsafeMutableRawPointer, _ n: Int) -> Int { read(fd, p, n) }
