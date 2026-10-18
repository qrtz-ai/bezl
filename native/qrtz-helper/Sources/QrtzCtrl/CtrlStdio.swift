// ctrl on the helper's stdio (slice 5b, ADR 0110). The helper's reader thread hands every line whose `type` starts
// with `ctrl.` to `CtrlStdio` instead of the echo/nano consumer, so ctrl requests never queue behind a dictation or
// a recording stop and echo/nano never see ctrl lines. Requests run one at a time on ctrl's own serial queue (an AX
// call or an Apple Event can take a second); each reply is one line through the helper's stdout writer.
//
// Foundation-only. Off macOS the backend is `UnsupportedCtrlBackend`: every request answers `platform.unsupported`
// at once, so a Linux build of the helper never leaves the host waiting.
import Foundation

public final class CtrlStdio: @unchecked Sendable {
    let server: CtrlServer
    let queue = DispatchQueue(label: "ai.qrtz.helper.ctrl")
    let write: @Sendable (String) -> Void

    public init(backend: CtrlBackend, write: @escaping @Sendable (String) -> Void) {
        server = CtrlServer(backend: backend)
        self.write = write
    }

    /// True for a ctrl request line. Cheap: a substring test first, then the parse.
    public static func isCtrlLine(_ line: String) -> Bool {
        guard line.contains("\"ctrl.") else { return false }
        guard let data = line.data(using: .utf8),
              let o = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let type = o["type"] as? String else { return false }
        return type.hasPrefix("ctrl.")
    }

    /// The host's goodbye (`{"type":"shutdown"}`, ADR 0146 M2.4's joint quit). qrtz-ctrl, which speaks only ctrl
    /// lines, finishes its queued requests and leaves on it instead of waiting out the grace for a kill.
    public static func isShutdownLine(_ line: String) -> Bool {
        guard line.contains("\"shutdown\"") else { return false }
        guard let data = line.data(using: .utf8),
              let o = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return false }
        return o["type"] as? String == "shutdown"
    }

    /// qrtz-ctrl's first line (run 31). The host's supervisor counts a helper as running from its first line, and
    /// ctrl's wire and warmer only talk to a running one, so a qrtz-ctrl that waited for a request to say anything
    /// would leave every run at `planned` and the app's helper status short of running. The host skips the line
    /// (it isn't a ctrl reply); `module:ctrl` says which app this is, like the mock helper's role hello.
    public static func helloLine(version: String) -> String {
        let o: [String: Any] = [
            "type": "hello", "protocol": 1, "helperVersion": version, "features": ["module:ctrl"],
        ]
        guard let d = try? JSONSerialization.data(withJSONObject: o, options: [.sortedKeys]),
              let s = String(data: d, encoding: .utf8) else { return #"{"type":"hello"}"# }
        return s
    }

    /// Queue one request; its reply is written when the backend is done.
    public func submit(_ line: String) {
        queue.async { [server, write] in write(server.handle(line: line)) }
    }

    /// Wait for the queued requests (tests, shutdown).
    public func drain() { queue.sync {} }
}

/// The backend off macOS (Linux builds and tests of the helper).
public final class UnsupportedCtrlBackend: CtrlBackend {
    public init() {}
    static let no = CtrlFailure("platform.unsupported")
    public func catalog() throws -> (apps: [CtrlCatalogApp], defaultBrowser: String?) { throw Self.no }
    public func preview(app: String, what: String) throws -> [CtrlPreviewItem] { throw Self.no }
    public func probe() throws -> (env: [String: Any], ui: [String: Any]) { throw Self.no }
    public func snapshot(app: String, scope: String?) throws -> [CtrlAXNode] { throw Self.no }
    public func act(_ route: CtrlRoute, content: [String: String]) throws { throw Self.no }
    public func measure(check: String, app: String) -> Int? { nil }
    public func restore(frontmostPid: Int32?) -> Bool { false }
    public func laya(state: String, question: String, options: [String]) throws -> [Double] { throw Self.no }
    public func warmLaya(model: String) throws -> CtrlLayaWarmer.State { throw Self.no }
    public func frontmostPid() -> Int32? { nil }
    public func pid(of app: String) -> Int32? { nil }
}

/// The backend this platform has.
public func makeCtrlBackend() -> CtrlBackend {
    #if os(macOS)
    return MacCtrlBackend()
    #else
    return UnsupportedCtrlBackend()
    #endif
}
