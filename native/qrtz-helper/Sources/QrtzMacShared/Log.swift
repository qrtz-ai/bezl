#if os(macOS)
import Foundation

/// The app's stderr tag ("qrtz-helper", "qrtz-echo", "qrtz-nano"); set once by the executable before anything logs.
nonisolated(unsafe) public var logTag = "qrtz-helper"

/// A diagnostic line on stderr (stdout carries protocol lines only).
public func logErr(_ s: String) { FileHandle.standardError.write(Data("[\(logTag)] \(s)\n".utf8)) }

/// "macOS 27.0.1", for the hello line.
public func macOSVersionString() -> String {
    let v = ProcessInfo.processInfo.operatingSystemVersion
    return "macOS \(v.majorVersion).\(v.minorVersion).\(v.patchVersion)"
}
#endif
