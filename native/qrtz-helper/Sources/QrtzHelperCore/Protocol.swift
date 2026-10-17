import Foundation

/// Mirror of packages/core/src/helper/protocol.ts. Keep in sync; Fixtures/protocol/*.jsonl are the shared contract
/// (validated by zod in packages/core and round-tripped here in ProtocolGoldenTests).
public let helperProtocolVersion = 1

public enum OsPermission: String, CaseIterable, Sendable { case accessibility, screenRecording = "screen-recording", microphone }
public enum PermissionState: String, Sendable { case granted, denied, notDetermined = "not-determined", unsupported }
public enum MeetingPlatform: String, Sendable { case zoom, googleMeet = "google-meet", teams, slackHuddle = "slack-huddle", webex, other }
public enum LogLevel: String, Sendable { case debug, info, warn, error }

public enum AsrEvent: Equatable, Sendable {
    case partial(utteranceId: String, startMs: Double, text: String)
    case final(utteranceId: String, startMs: Double, endMs: Double, text: String, confidence: Double?)
}

public struct ProtocolError: Error, Equatable, CustomStringConvertible {
    public let description: String
    public init(_ d: String) { description = d }
}

/// Helper → host.
public enum HelperMessage: Equatable, Sendable {
    case hello(protocol: Int, helperVersion: String, os: String, features: [String])
    case heartbeat(ts: Double)
    case pong(id: String)
    case permissionsStatus([OsPermission: PermissionState])
    case log(level: LogLevel, msg: String)
    case error(code: String, message: String, fatal: Bool)
    case meetingDetected(key: String, title: String, platform: MeetingPlatform, app: String)
    case meetingEnded(key: String)
    case axParticipants(names: [String])
    case axActiveSpeaker(atMs: Double, name: String)
    case axCaption(atMs: Double, speaker: String?, text: String)
    case axChat(atMs: Double, author: String, text: String)
    case audioLevel(rms: Double)
    case asr(AsrEvent)
    case recordingStarted(audioPath: String)
    case recordingStopped(audioPath: String, durationMs: Double)
}

/// Host → helper.
public enum HostMessage: Equatable, Sendable {
    case ping(id: String)
    case permissionsCheck
    case permissionsRequest(OsPermission)
    case shutdown
    case recordingStart(audioPath: String)
    case recordingStop
    /// Privacy pause: audio is replaced by silence (timeline stays aligned) and nothing reaches the recogniser.
    case recordingPause
    case recordingResume
}

// MARK: - Encoding helpers

private func s(_ v: String) -> JSONValue { .string(v) }
private func n(_ v: Double) -> JSONValue { .number(v) }

private struct Fields {
    let o: [String: JSONValue]
    init(_ v: JSONValue) throws {
        guard let o = v.object else { throw ProtocolError("expected object") }
        self.o = o
    }
    func str(_ k: String) throws -> String {
        guard let v = o[k]?.string else { throw ProtocolError("missing string \(k)") }
        return v
    }
    func optStr(_ k: String) throws -> String? {
        guard let v = o[k], v != .null else { return nil }
        guard let s = v.string else { throw ProtocolError("bad string \(k)") }
        return s
    }
    func num(_ k: String) throws -> Double {
        guard let v = o[k]?.number, v.isFinite else { throw ProtocolError("missing number \(k)") }
        return v
    }
    func optNum(_ k: String) throws -> Double? {
        guard let v = o[k], v != .null else { return nil }
        guard let d = v.number else { throw ProtocolError("bad number \(k)") }
        return d
    }
    func strs(_ k: String) throws -> [String] {
        guard let a = o[k]?.array else { throw ProtocolError("missing array \(k)") }
        return try a.map { guard let s = $0.string else { throw ProtocolError("bad item in \(k)") }; return s }
    }
    func enumValue<E: RawRepresentable>(_ k: String, _: E.Type) throws -> E where E.RawValue == String {
        guard let e = E(rawValue: try str(k)) else { throw ProtocolError("bad enum \(k)") }
        return e
    }
}

extension AsrEvent {
    public var json: JSONValue {
        switch self {
        case .partial(let id, let start, let text):
            return .object(["kind": s("partial"), "utteranceId": s(id), "startMs": n(start), "text": s(text)])
        case .final(let id, let start, let end, let text, let conf):
            var o: [String: JSONValue] = ["kind": s("final"), "utteranceId": s(id), "startMs": n(start), "endMs": n(end), "text": s(text)]
            if let conf { o["confidence"] = n(conf) }
            return .object(o)
        }
    }

    public init(json: JSONValue) throws {
        let f = try Fields(json)
        switch try f.str("kind") {
        case "partial": self = .partial(utteranceId: try f.str("utteranceId"), startMs: try f.num("startMs"), text: try f.str("text"))
        case "final":
            self = .final(utteranceId: try f.str("utteranceId"), startMs: try f.num("startMs"), endMs: try f.num("endMs"), text: try f.str("text"), confidence: try f.optNum("confidence"))
        default: throw ProtocolError("bad asr kind")
        }
    }
}

extension HelperMessage {
    public var json: JSONValue {
        func obj(_ type: String, _ rest: [String: JSONValue] = [:]) -> JSONValue {
            var o = rest
            o["type"] = s(type)
            return .object(o)
        }
        switch self {
        case .hello(let p, let v, let os, let features):
            return obj("hello", ["protocol": n(Double(p)), "helperVersion": s(v), "os": s(os), "features": .array(features.map(s))])
        case .heartbeat(let ts): return obj("heartbeat", ["ts": n(ts)])
        case .pong(let id): return obj("pong", ["id": s(id)])
        case .permissionsStatus(let st):
            // zod's record over an enum key is exhaustive, so always send every permission.
            var o: [String: JSONValue] = [:]
            for p in OsPermission.allCases { o[p.rawValue] = s((st[p] ?? .notDetermined).rawValue) }
            return obj("permissions.status", ["status": .object(o)])
        case .log(let level, let msg): return obj("log", ["level": s(level.rawValue), "msg": s(msg)])
        case .error(let code, let message, let fatal): return obj("error", ["code": s(code), "message": s(message), "fatal": .bool(fatal)])
        case .meetingDetected(let key, let title, let platform, let app):
            return obj("meeting.detected", ["key": s(key), "title": s(title), "platform": s(platform.rawValue), "app": s(app)])
        case .meetingEnded(let key): return obj("meeting.ended", ["key": s(key)])
        case .axParticipants(let names): return obj("ax.participants", ["names": .array(names.map(s))])
        case .axActiveSpeaker(let at, let name): return obj("ax.activeSpeaker", ["atMs": n(at), "name": s(name)])
        case .axCaption(let at, let speaker, let text):
            var o: [String: JSONValue] = ["atMs": n(at), "text": s(text)]
            if let speaker { o["speaker"] = s(speaker) }
            return obj("ax.caption", o)
        case .axChat(let at, let author, let text): return obj("ax.chat", ["atMs": n(at), "author": s(author), "text": s(text)])
        case .audioLevel(let rms): return obj("audio.level", ["rms": n(min(1, max(0, rms)))])
        case .asr(let e): return obj("asr", ["event": e.json])
        case .recordingStarted(let p): return obj("recording.started", ["audioPath": s(p)])
        case .recordingStopped(let p, let d): return obj("recording.stopped", ["audioPath": s(p), "durationMs": n(d)])
        }
    }

    /// One wire line (no trailing newline).
    public var line: String { json.serialized() }

    public init(json: JSONValue) throws {
        let f = try Fields(json)
        switch try f.str("type") {
        case "hello": self = .hello(protocol: Int(try f.num("protocol")), helperVersion: try f.str("helperVersion"), os: try f.str("os"), features: try f.strs("features"))
        case "heartbeat": self = .heartbeat(ts: try f.num("ts"))
        case "pong": self = .pong(id: try f.str("id"))
        case "permissions.status":
            guard let o = f.o["status"]?.object else { throw ProtocolError("missing status") }
            var st: [OsPermission: PermissionState] = [:]
            for p in OsPermission.allCases {
                guard let raw = o[p.rawValue]?.string, let v = PermissionState(rawValue: raw) else { throw ProtocolError("missing permission \(p.rawValue)") }
                st[p] = v
            }
            self = .permissionsStatus(st)
        case "log": self = .log(level: try f.enumValue("level", LogLevel.self), msg: try f.str("msg"))
        case "error": self = .error(code: try f.str("code"), message: try f.str("message"), fatal: f.o["fatal"]?.bool ?? false)
        case "meeting.detected": self = .meetingDetected(key: try f.str("key"), title: try f.str("title"), platform: try f.enumValue("platform", MeetingPlatform.self), app: try f.str("app"))
        case "meeting.ended": self = .meetingEnded(key: try f.str("key"))
        case "ax.participants": self = .axParticipants(names: try f.strs("names"))
        case "ax.activeSpeaker": self = .axActiveSpeaker(atMs: try f.num("atMs"), name: try f.str("name"))
        case "ax.caption": self = .axCaption(atMs: try f.num("atMs"), speaker: try f.optStr("speaker"), text: try f.str("text"))
        case "ax.chat": self = .axChat(atMs: try f.num("atMs"), author: try f.str("author"), text: try f.str("text"))
        case "audio.level":
            let r = try f.num("rms")
            guard (0...1).contains(r) else { throw ProtocolError("rms out of range") }
            self = .audioLevel(rms: r)
        case "asr":
            guard let e = f.o["event"] else { throw ProtocolError("missing event") }
            self = .asr(try AsrEvent(json: e))
        case "recording.started": self = .recordingStarted(audioPath: try f.str("audioPath"))
        case "recording.stopped": self = .recordingStopped(audioPath: try f.str("audioPath"), durationMs: try f.num("durationMs"))
        case let t: throw ProtocolError("unknown helper message type \(t)")
        }
    }
}

extension HostMessage {
    public var json: JSONValue {
        switch self {
        case .ping(let id): return .object(["type": s("ping"), "id": s(id)])
        case .permissionsCheck: return .object(["type": s("permissions.check")])
        case .permissionsRequest(let p): return .object(["type": s("permissions.request"), "permission": s(p.rawValue)])
        case .shutdown: return .object(["type": s("shutdown")])
        case .recordingStart(let p): return .object(["type": s("recording.start"), "audioPath": s(p)])
        case .recordingStop: return .object(["type": s("recording.stop")])
        case .recordingPause: return .object(["type": s("recording.pause")])
        case .recordingResume: return .object(["type": s("recording.resume")])
        }
    }

    public init(json: JSONValue) throws {
        let f = try Fields(json)
        switch try f.str("type") {
        case "ping": self = .ping(id: try f.str("id"))
        case "permissions.check": self = .permissionsCheck
        case "permissions.request": self = .permissionsRequest(try f.enumValue("permission", OsPermission.self))
        case "shutdown": self = .shutdown
        case "recording.start": self = .recordingStart(audioPath: try f.str("audioPath"))
        case "recording.stop": self = .recordingStop
        case "recording.pause": self = .recordingPause
        case "recording.resume": self = .recordingResume
        case let t: throw ProtocolError("unknown host message type \(t)")
        }
    }

    /// Parse one stdin line. Never throws; returns nil on malformed or unknown input (host may be newer).
    public static func parse(line: String) -> Result<HostMessage, ProtocolError> {
        let t = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return .failure(ProtocolError("empty")) }
        do { return .success(try HostMessage(json: try JSONValue.parse(t))) }
        catch let e as ProtocolError { return .failure(e) }
        catch { return .failure(ProtocolError("invalid json")) }
    }
}

/// Splits a byte stream into lines, buffering partial lines across chunks (mirrors createLineSplitter in TS).
public struct LineSplitter {
    private var buf = Data()
    public let maxLine: Int
    public init(maxLine: Int = 1_000_000) { self.maxLine = maxLine }

    public mutating func feed(_ chunk: Data) -> [String] {
        buf.append(chunk)
        var out: [String] = []
        while let i = buf.firstIndex(of: 0x0A) {
            out.append(String(decoding: buf[buf.startIndex..<i], as: UTF8.self))
            buf = Data(buf[buf.index(after: i)...])
        }
        if buf.count > maxLine { buf.removeAll() }
        return out
    }
}
