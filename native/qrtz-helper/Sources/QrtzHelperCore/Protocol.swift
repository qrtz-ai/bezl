import Foundation

/// Mirror of packages/core/src/helper/protocol.ts. Keep in sync; Fixtures/protocol/*.jsonl are the shared contract
/// (validated by zod in packages/core and round-tripped here in ProtocolGoldenTests).
public let helperProtocolVersion = 1

public enum OsPermission: String, CaseIterable, Sendable { case accessibility, screenRecording = "screen-recording", microphone }
/// Why the helper ended a dictation on its own.
/// Recording degradations a TCC revocation can cause (zod `RevocationDegradation`).
public enum RevocationDegradation: String, Sendable, CaseIterable {
    case axUnavailable = "ax-unavailable", audioUnavailable = "audio-unavailable", micOnly = "mic-only", noMic = "no-mic"
}
/// N126: why a channel has no speaker turns from `atMs` on (`backlog`: the diarizer fell too far behind and stood
/// down; `model-unavailable`: its model didn't load).
public enum DiarizationUnavailableReason: String, Sendable { case backlog, modelUnavailable = "model-unavailable" }
public enum DictationStopReason: String, Sendable { case permissionRevoked = "permission-revoked", micLost = "mic-lost" }
public enum PermissionState: String, Sendable { case granted, denied, notDetermined = "not-determined", unsupported }
public enum MeetingPlatform: String, Sendable { case zoom, googleMeet = "google-meet", teams, slackHuddle = "slack-huddle", webex, other }
public enum InsertMethod: String, Sendable { case axSelectedText = "ax-selected-text", axValue = "ax-value", paste, none }
public enum HotkeyAction: String, Sendable { case down, up }
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
    /// N35: zod `CAPTURE_DEGRADED_REASON_MAX` (UTF-16 units, like a JS string length).
    public static let captureDegradedReasonMax = 200

    /// Error text cut to `captureDegradedReasonMax` UTF-16 units on whole scalars, so the line always validates.
    static func capReason(_ r: String) -> String {
        guard r.utf16.count > captureDegradedReasonMax else { return r }
        var out = String.UnicodeScalarView()
        var n = 0
        for u in r.unicodeScalars {
            let w = u.utf16.count
            if n + w > captureDegradedReasonMax { break }
            out.append(u)
            n += w
        }
        return String(out)
    }

    case hello(protocol: Int, helperVersion: String, os: String, features: [String])
    case heartbeat(ts: Double)
    case pong(id: String)
    case permissionsStatus([OsPermission: PermissionState])
    case log(level: LogLevel, msg: String)
    /// id/activeId are set on dictation.busy (the ignored start, the dictation that owns the helper);
    /// audioPath/activeAudioPath on recording.busy (the ignored start's file, the running recording's file).
    case error(code: String, message: String, fatal: Bool, id: String? = nil, activeId: String? = nil, audioPath: String? = nil, activeAudioPath: String? = nil)
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
    /// Answer to recording.abort{audioPath}: no writer on that file remains (sent even when nothing recorded it).
    case recordingAborted(audioPath: String)
    // talk (dictation)
    case hotkey(HotkeyAction)
    case dictationStarted(id: String, preRollMs: Double)
    case dictationLevel(rms: Double)
    case dictationAsr(id: String, event: AsrEvent)
    case dictationAudio(id: String, seq: Int, pcm16: String)
    /// reason is set only when the helper ended the dictation itself (see DictationStopReason).
    case dictationStopped(id: String, text: String, durationMs: Double, reason: DictationStopReason? = nil)
    /// A TCC permission went granted → denied while capturing or dictating. `degraded` (N4): the arbiter's
    /// `degrade:*` effects, `[]` when not recording; nil only when parsing an older helper's line.
    /// N29: `source` has been under the silence floor since `sinceMs` (recording-relative), for `SilenceConfig.afterMs`.
    case captureSilent(source: CaptureSource, sinceMs: Double)
    /// N29: `source` is above the floor again after a `captureSilent`.
    case captureAudible(source: CaptureSource)
    /// ADR 0079: one speaker's turn on one raw channel (on-device diarization), recording-relative ms. `speaker` is
    /// that channel's slot 0...7 (the host names it `mic:N` / `system:N`); never merged across channels.
    case diarizationSegment(source: CaptureSource, speaker: Int, startMs: Double, endMs: Double)
    /// N126: `source` gets no more speaker turns this recording from `atMs` (recording-relative) on. Once per channel
    /// per recording, after the turns it could still send.
    case diarizationUnavailable(source: CaptureSource, atMs: Double, reason: DiarizationUnavailableReason)
    /// H8: capture source `from` failed to start; `to` is tried next (nil: none left).
    case captureFallback(from: String, to: String?, reason: String)
    /// N35: a source lost part of its input (today: its in-source mic) and records the rest. Sticky until a reconnect.
    case captureDegraded(degraded: [RevocationDegradation], reason: String)
    case permissionRevoked(permission: OsPermission, atMs: Double, degraded: [RevocationDegradation]? = nil)
    case insertResult(id: String, method: InsertMethod, ok: Bool, app: String?, error: String?)
    /// N55: the audio fds named by speech.attach were taken (see SpeechSinks).
    case speechAttached(sessionId: String)
    case speechAttachRejected(sessionId: String, reason: SpeechAttachReject)
    /// ADR 0075: answers to summarize / summarize.check.
    case summarizeResult(id: String, output: SummaryOutput)
    case summarizeError(id: String, failure: SummarizeFailure)
    case summarizeStatus(id: String, available: Bool, reason: SummarizeUnavailable?, contextTokens: Int?)
}

/// Host → helper.
public enum HostMessage: Equatable, Sendable {
    case ping(id: String)
    case permissionsCheck
    case permissionsRequest(OsPermission)
    case shutdown
    case recordingStart(audioPath: String)
    /// audioPath (optional): only stop the recording writing this file; a stop for any other path is ignored.
    case recordingStop(audioPath: String? = nil)
    /// N2: tear down the recording writing this file without a flush (scoped; dictation untouched). Out of band.
    case recordingAbort(audioPath: String)
    /// Privacy pause: audio is replaced by silence (timeline stays aligned) and nothing reaches the recogniser.
    case recordingPause
    case recordingResume
    /// consumeFn: nil = not sent (an older host): the helper's default, on.
    case dictationConfigure(warmMic: Bool, preRollMs: Int, streamAudio: Bool, pushKey: String, consumeFn: Bool? = nil)
    case dictationStart(id: String)
    case dictationStop(id: String)
    case dictationCancel(id: String)
    case insertText(id: String, text: String, restoreClipboard: Bool)
    /// ADR 0046 §1: the speech module's audio write ends wait on the speech socket (see SpeechSinks).
    case speechAttach(sessionId: String, target: SpeechTarget, channels: [CaptureSource])
    /// ADR 0075: summarize a host-built, fenced prompt with the on-device model (see SummarizeService).
    case summarize(id: String, stage: SummarizeStage, prompt: String)
    case summarizeCancel(id: String)
    /// ADR 0079 / N121: the host just installed the speech models: load each once (one at a time, then unload) so the
    /// OS compiles and caches them now, not at the first recording. Ignored while a recording runs.
    case modelsWarm
    case summarizeCheck(id: String)
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
        case .error(let code, let message, let fatal, let id, let activeId, let audioPath, let activeAudioPath):
            var o: [String: JSONValue] = ["code": s(code), "message": s(message), "fatal": .bool(fatal)]
            if let id { o["id"] = s(id) }
            if let activeId { o["activeId"] = s(activeId) }
            if let audioPath { o["audioPath"] = s(audioPath) }
            if let activeAudioPath { o["activeAudioPath"] = s(activeAudioPath) }
            return obj("error", o)
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
        case .recordingAborted(let p): return obj("recording.aborted", ["audioPath": s(p)])
        case .hotkey(let a): return obj("hotkey", ["action": s(a.rawValue)])
        case .dictationStarted(let id, let pre): return obj("dictation.started", ["id": s(id), "preRollMs": n(pre)])
        case .dictationLevel(let rms): return obj("dictation.level", ["rms": n(min(1, max(0, rms)))])
        case .dictationAsr(let id, let e): return obj("dictation.asr", ["id": s(id), "event": e.json])
        case .dictationAudio(let id, let seq, let pcm): return obj("dictation.audio", ["id": s(id), "seq": n(Double(seq)), "pcm16": s(pcm)])
        case .dictationStopped(let id, let text, let d, let reason):
            var o: [String: JSONValue] = ["id": s(id), "text": s(text), "durationMs": n(d)]
            if let reason { o["reason"] = s(reason.rawValue) }
            return obj("dictation.stopped", o)
        case .captureSilent(let src, let since): return obj("capture.silent", ["source": s(src.rawValue), "sinceMs": n(since)])
        case .captureAudible(let src): return obj("capture.audible", ["source": s(src.rawValue)])
        case .diarizationSegment(let src, let spk, let a, let b):
            return obj("diarization.segment", ["source": s(src.rawValue), "speaker": n(Double(spk)), "startMs": n(a), "endMs": n(b)])
        case .diarizationUnavailable(let src, let at, let why):
            return obj("diarization.unavailable", ["source": s(src.rawValue), "atMs": n(at), "reason": s(why.rawValue)])
        case .captureFallback(let from, let to, let reason):
            return obj("capture.fallback", ["from": s(from), "to": to.map(s) ?? .null, "reason": s(reason)])
        case .captureDegraded(let degraded, let reason):
            return obj("capture.degraded", ["degraded": .array(degraded.map { s($0.rawValue) }), "reason": s(Self.capReason(reason))])
        case .permissionRevoked(let p, let at, let degraded):
            var o: [String: JSONValue] = ["permission": s(p.rawValue), "atMs": n(at)]
            if let degraded { o["degraded"] = .array(degraded.map { s($0.rawValue) }) }
            return obj("permission.revoked", o)
        case .insertResult(let id, let m, let ok, let app, let err):
            var o: [String: JSONValue] = ["id": s(id), "method": s(m.rawValue), "ok": .bool(ok)]
            if let app { o["app"] = s(app) }
            if let err { o["error"] = s(err) }
            return obj("insert.result", o)
        case .speechAttached(let sid): return obj("speech.attached", ["sessionId": s(sid)])
        case .speechAttachRejected(let sid, let r): return obj("speech.attach.rejected", ["sessionId": s(sid), "reason": s(r.rawValue)])
        case .summarizeResult(let id, let out):
            let items: [JSONValue] = out.actionItems.map { a in
                var o: [String: JSONValue] = ["text": s(a.text)]
                if let owner = a.owner { o["owner"] = s(owner) }
                return .object(o)
            }
            let qa: [JSONValue] = out.qa.map { q in
                var o: [String: JSONValue] = ["question": s(q.question), "answer": s(q.answer)]
                if let by = q.askedBy { o["askedBy"] = s(by) }
                return .object(o)
            }
            return obj("summarize.result", ["id": s(id), "summary": s(out.summary), "actionItems": .array(items), "qa": .array(qa)])
        case .summarizeError(let id, let f):
            var o: [String: JSONValue] = ["id": s(id), "code": s(f.code.rawValue), "message": s(SummaryOutput.clean(f.message, SummarizeLimits.messageMax))]
            if let r = f.reason { o["reason"] = s(r.rawValue) }
            return obj("summarize.error", o)
        case .summarizeStatus(let id, let available, let reason, let ctx):
            var o: [String: JSONValue] = ["id": s(id), "available": .bool(available)]
            if let reason { o["reason"] = s(reason.rawValue) }
            if let ctx { o["contextTokens"] = n(Double(ctx)) }
            return obj("summarize.status", o)
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
        case "error":
            self = .error(code: try f.str("code"), message: try f.str("message"), fatal: f.o["fatal"]?.bool ?? false,
                          id: try f.optStr("id"), activeId: try f.optStr("activeId"),
                          audioPath: try f.optStr("audioPath"), activeAudioPath: try f.optStr("activeAudioPath"))
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
        case "recording.aborted": self = .recordingAborted(audioPath: try f.str("audioPath"))
        case "hotkey": self = .hotkey(try f.enumValue("action", HotkeyAction.self))
        case "dictation.started": self = .dictationStarted(id: try f.str("id"), preRollMs: try f.num("preRollMs"))
        case "dictation.level":
            let r = try f.num("rms")
            guard (0...1).contains(r) else { throw ProtocolError("rms out of range") }
            self = .dictationLevel(rms: r)
        case "dictation.asr":
            guard let e = f.o["event"] else { throw ProtocolError("missing event") }
            self = .dictationAsr(id: try f.str("id"), event: try AsrEvent(json: e))
        case "dictation.audio": self = .dictationAudio(id: try f.str("id"), seq: Int(try f.num("seq")), pcm16: try f.str("pcm16"))
        case "dictation.stopped":
            self = .dictationStopped(id: try f.str("id"), text: try f.str("text"), durationMs: try f.num("durationMs"),
                                     reason: try f.optStr("reason").map { r in
                                         guard let v = DictationStopReason(rawValue: r) else { throw ProtocolError("bad reason \(r)") }
                                         return v
                                     })
        case "capture.silent":
            let since = try f.num("sinceMs")
            guard since >= 0 else { throw ProtocolError("negative sinceMs") }
            self = .captureSilent(source: try f.enumValue("source", CaptureSource.self), sinceMs: since)
        case "capture.audible": self = .captureAudible(source: try f.enumValue("source", CaptureSource.self))
        case "diarization.segment":
            let spk = try f.num("speaker"), a = try f.num("startMs"), b = try f.num("endMs")
            guard spk >= 0, spk <= 7, spk == spk.rounded(), a >= 0, b > a else { throw ProtocolError("bad diarization.segment") }
            self = .diarizationSegment(source: try f.enumValue("source", CaptureSource.self), speaker: Int(spk), startMs: a, endMs: b)
        case "diarization.unavailable":
            let at = try f.num("atMs")
            guard at >= 0 else { throw ProtocolError("negative atMs") }
            self = .diarizationUnavailable(source: try f.enumValue("source", CaptureSource.self), atMs: at,
                                           reason: try f.enumValue("reason", DiarizationUnavailableReason.self))
        case "capture.fallback":
            guard let to = f.o["to"] else { throw ProtocolError("missing to") }
            guard to == .null || !(to.string ?? "").isEmpty else { throw ProtocolError("bad to") }
            let from = try f.str("from")
            guard !from.isEmpty else { throw ProtocolError("empty from") }
            self = .captureFallback(from: from, to: to.string, reason: try f.str("reason"))
        case "capture.degraded":
            guard let a = f.o["degraded"]?.array, !a.isEmpty else { throw ProtocolError("bad degraded") }
            let degraded = try a.map { x in
                guard let r = x.string.flatMap(RevocationDegradation.init(rawValue:)) else { throw ProtocolError("bad degraded") }
                return r
            }
            let reason = try f.str("reason")
            guard reason.utf16.count <= Self.captureDegradedReasonMax else { throw ProtocolError("reason too long") }
            self = .captureDegraded(degraded: degraded, reason: reason)
        case "permission.revoked":
            var degraded: [RevocationDegradation]?
            if let v = f.o["degraded"], v != .null {
                guard let a = v.array else { throw ProtocolError("bad array degraded") }
                degraded = try a.map { x in
                    guard let r = x.string.flatMap(RevocationDegradation.init(rawValue:)) else { throw ProtocolError("bad degraded") }
                    return r
                }
            }
            self = .permissionRevoked(permission: try f.enumValue("permission", OsPermission.self), atMs: try f.num("atMs"), degraded: degraded)
        case "insert.result":
            guard let ok = f.o["ok"]?.bool else { throw ProtocolError("missing ok") }
            self = .insertResult(id: try f.str("id"), method: try f.enumValue("method", InsertMethod.self), ok: ok, app: try f.optStr("app"), error: try f.optStr("error"))
        case "speech.attached": self = .speechAttached(sessionId: try f.str("sessionId"))
        case "speech.attach.rejected":
            self = .speechAttachRejected(sessionId: try f.str("sessionId"), reason: try f.enumValue("reason", SpeechAttachReject.self))
        case "summarize.result":
            func arr(_ k: String) throws -> [Fields] {
                guard let a = f.o[k]?.array else { throw ProtocolError("missing array \(k)") }
                return try a.map { try Fields($0) }
            }
            let out = SummaryOutput(
                summary: try f.str("summary"),
                actionItems: try arr("actionItems").map { SummaryActionItem(text: try $0.str("text"), owner: try $0.optStr("owner")) },
                qa: try arr("qa").map { SummaryQa(question: try $0.str("question"), answer: try $0.str("answer"), askedBy: try $0.optStr("askedBy")) })
            guard out == out.capped else { throw ProtocolError("summarize.result out of bounds") }
            self = .summarizeResult(id: try f.str("id"), output: out)
        case "summarize.error":
            let reason = try f.optStr("reason").map { r -> SummarizeUnavailable in
                guard let v = SummarizeUnavailable(rawValue: r) else { throw ProtocolError("bad reason") }
                return v
            }
            self = .summarizeError(id: try f.str("id"), failure: SummarizeFailure(try f.enumValue("code", SummarizeErrorCode.self), reason: reason, try f.str("message")))
        case "summarize.status":
            guard let a = f.o["available"]?.bool else { throw ProtocolError("missing available") }
            let reason = try f.optStr("reason").map { r -> SummarizeUnavailable in
                guard let v = SummarizeUnavailable(rawValue: r) else { throw ProtocolError("bad reason") }
                return v
            }
            self = .summarizeStatus(id: try f.str("id"), available: a, reason: reason, contextTokens: try f.optNum("contextTokens").map { Int($0) })
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
        case .recordingStop(let p):
            return p.map { .object(["type": s("recording.stop"), "audioPath": s($0)]) } ?? .object(["type": s("recording.stop")])
        case .recordingAbort(let p): return .object(["type": s("recording.abort"), "audioPath": s(p)])
        case .recordingPause: return .object(["type": s("recording.pause")])
        case .recordingResume: return .object(["type": s("recording.resume")])
        case .modelsWarm: return .object(["type": s("models.warm")])
        case .dictationConfigure(let warm, let pre, let stream, let key, let consume):
            var o: [String: JSONValue] = ["type": s("dictation.configure"), "warmMic": .bool(warm), "preRollMs": n(Double(pre)), "streamAudio": .bool(stream), "pushKey": s(key)]
            if let consume { o["consumeFn"] = .bool(consume) }
            return .object(o)
        case .dictationStart(let id): return .object(["type": s("dictation.start"), "id": s(id)])
        case .dictationStop(let id): return .object(["type": s("dictation.stop"), "id": s(id)])
        case .dictationCancel(let id): return .object(["type": s("dictation.cancel"), "id": s(id)])
        case .insertText(let id, let text, let restore):
            return .object(["type": s("insert.text"), "id": s(id), "text": s(text), "restoreClipboard": .bool(restore)])
        case .speechAttach(let sid, let target, let channels):
            let t: JSONValue
            switch target {
            case .dictation(let id): t = .object(["kind": s("dictation"), "id": s(id)])
            case .recording(let p): t = .object(["kind": s("recording"), "audioPath": s(p)])
            }
            return .object(["type": s("speech.attach"), "sessionId": s(sid), "target": t, "channels": .array(channels.map { s($0.rawValue) })])
        case .summarize(let id, let stage, let prompt):
            return .object(["type": s("summarize"), "id": s(id), "stage": s(stage.rawValue), "prompt": s(prompt)])
        case .summarizeCancel(let id): return .object(["type": s("summarize.cancel"), "id": s(id)])
        case .summarizeCheck(let id): return .object(["type": s("summarize.check"), "id": s(id)])
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
        case "recording.stop": self = .recordingStop(audioPath: try f.optStr("audioPath"))
        case "recording.abort": self = .recordingAbort(audioPath: try f.str("audioPath"))
        case "recording.pause": self = .recordingPause
        case "recording.resume": self = .recordingResume
        case "models.warm": self = .modelsWarm
        case "dictation.configure":
            guard let warm = f.o["warmMic"]?.bool, let stream = f.o["streamAudio"]?.bool else { throw ProtocolError("missing bool") }
            if let c = f.o["consumeFn"], c.bool == nil { throw ProtocolError("consumeFn must be a bool") }
            self = .dictationConfigure(warmMic: warm, preRollMs: Int(try f.num("preRollMs")), streamAudio: stream, pushKey: try f.str("pushKey"),
                                       consumeFn: f.o["consumeFn"]?.bool)
        case "dictation.start": self = .dictationStart(id: try f.str("id"))
        case "dictation.stop": self = .dictationStop(id: try f.str("id"))
        case "dictation.cancel": self = .dictationCancel(id: try f.str("id"))
        case "insert.text":
            guard let r = f.o["restoreClipboard"]?.bool else { throw ProtocolError("missing restoreClipboard") }
            self = .insertText(id: try f.str("id"), text: try f.str("text"), restoreClipboard: r)
        case "speech.attach":
            let t = try Fields(f.o["target"] ?? .null)
            let target: SpeechTarget
            switch try t.str("kind") {
            case "dictation": target = .dictation(id: try t.str("id"))
            case "recording": target = .recording(audioPath: try t.str("audioPath"))
            case let k: throw ProtocolError("bad speech target \(k)")
            }
            let chans = try f.strs("channels").map { c -> CaptureSource in
                guard let v = CaptureSource(rawValue: c) else { throw ProtocolError("bad channel") }
                return v
            }
            guard (1...2).contains(chans.count) else { throw ProtocolError("bad channels") }
            self = .speechAttach(sessionId: try f.str("sessionId"), target: target, channels: chans)
        case "summarize":
            let id = try f.str("id"), prompt = try f.str("prompt")
            guard (1...64).contains(id.count) else { throw ProtocolError("bad id") }
            guard !prompt.isEmpty, prompt.utf16.count <= SummarizeLimits.promptMax else { throw ProtocolError("bad prompt") }
            self = .summarize(id: id, stage: try f.enumValue("stage", SummarizeStage.self), prompt: prompt)
        case "summarize.cancel": self = .summarizeCancel(id: try f.str("id"))
        case "summarize.check": self = .summarizeCheck(id: try f.str("id"))
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
