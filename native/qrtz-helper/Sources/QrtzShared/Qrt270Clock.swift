import Foundation

/// QRT-270 diag (temporary, not for main): ms since this process started, for the `qrt270 t+` launch-timing lines.
private let qrt270Start = ProcessInfo.processInfo.systemUptime
public func qrt270Ms() -> Int { Int((ProcessInfo.processInfo.systemUptime - qrt270Start) * 1000) }
