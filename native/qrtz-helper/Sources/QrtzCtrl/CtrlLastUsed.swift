// ADR 0130: the per-app last-used signal behind "quit apps I haven't used today". The macOS backend feeds it
// NSWorkspace activation notifications (bundle id + time) from the moment the helper starts; nothing is read from
// the screen and nothing is written to disk. `since` is when the watch began: the host's cold-start rule treats an
// app with no record as unused only when the watch covers the whole day. Foundation-only, so it tests on Linux.
import Foundation

public final class CtrlLastUsed: @unchecked Sendable {
    private var last: [String: UInt64] = [:]
    private let lock = NSLock()
    /// When the watch began (UTC ms).
    public let sinceMs: UInt64
    /// Bounded: one entry per app the user activates, oldest dropped past this.
    static let maxApps = 512

    public init(sinceMs: UInt64) { self.sinceMs = sinceMs }

    public func record(_ bundleId: String, atMs: UInt64) {
        lock.lock(); defer { lock.unlock() }
        last[bundleId] = max(last[bundleId] ?? 0, atMs)
        if last.count > Self.maxApps, let oldest = last.min(by: { $0.value < $1.value })?.key {
            last.removeValue(forKey: oldest)
        }
    }

    public func lastActive(_ bundleId: String) -> UInt64? {
        lock.lock(); defer { lock.unlock() }
        return last[bundleId]
    }

    /// Local midnight of the day containing `nowMs`, in `calendar`'s zone (the Mac's own by default).
    public static func dayStartMs(nowMs: UInt64, calendar: Calendar = .current) -> UInt64 {
        let now = Date(timeIntervalSince1970: Double(nowMs) / 1000)
        return UInt64(max(0, calendar.startOfDay(for: now).timeIntervalSince1970 * 1000))
    }

    public static func nowMs() -> UInt64 { UInt64(max(0, Date().timeIntervalSince1970 * 1000)) }
}
