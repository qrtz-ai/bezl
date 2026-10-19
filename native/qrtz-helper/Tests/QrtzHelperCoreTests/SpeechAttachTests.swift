// N55 (ADR 0046 §1): the helper takes exactly the named write-only FIFOs for a speech session, over a real
// socketpair, writes PCM16 from the capture to them, and closes them when the capture ends.
#if canImport(Glibc)
import Glibc
#elseif canImport(Darwin)
import Darwin
#endif
import Foundation
import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

private func socketPair() -> (Int32, Int32) {
    var fds: [Int32] = [0, 0]
    #if os(Linux)
    let ty = Int32(SOCK_SEQPACKET.rawValue)
    #else
    let ty = SOCK_STREAM
    #endif
    XCTAssertEqual(socketpair(AF_UNIX, ty, 0, &fds), 0)
    return (fds[0], fds[1])
}

private func makePipe() -> (r: Int32, w: Int32) {
    var fds: [Int32] = [0, 0]
    XCTAssertEqual(pipe(&fds), 0)
    return (fds[0], fds[1])
}

/// The host side of one message (what the Rust `fdpass::send_fds` does).
private func sendFds(_ sock: Int32, _ payload: String, _ fds: [Int32]) {
    #if os(Linux)
    let align = MemoryLayout<Int>.size
    #else
    let align = MemoryLayout<UInt32>.size
    #endif
    func up(_ n: Int) -> Int { (n + align - 1) & ~(align - 1) }
    let hdr = up(MemoryLayout<cmsghdr>.size)
    let len = hdr + fds.count * 4
    var control = [UInt8](repeating: 0, count: up(len))
    var bytes = Array(payload.utf8)
    control.withUnsafeMutableBytes { cb in
        var c = cmsghdr()
        #if os(Linux)
        c.cmsg_len = len
        #else
        c.cmsg_len = socklen_t(len)
        #endif
        c.cmsg_level = SOL_SOCKET
        c.cmsg_type = Int32(SCM_RIGHTS)
        memcpy(cb.baseAddress!, &c, MemoryLayout<cmsghdr>.size)
        for (i, f) in fds.enumerated() {
            var v = f
            memcpy(cb.baseAddress!.advanced(by: hdr + i * 4), &v, 4)
        }
        bytes.withUnsafeMutableBytes { pb in
            var iov = iovec(iov_base: pb.baseAddress, iov_len: pb.count)
            withUnsafeMutablePointer(to: &iov) { iovp in
                var msg = msghdr()
                msg.msg_iov = iovp
                msg.msg_iovlen = 1
                msg.msg_control = cb.baseAddress
                #if os(Linux)
                msg.msg_controllen = cb.count
                #else
                msg.msg_controllen = socklen_t(cb.count)
                #endif
                XCTAssertEqual(sendmsg(sock, &msg, 0), payload.utf8.count)
            }
        }
    }
    for f in fds { close(f) }  // the host keeps no copy
}

private func readAll(_ fd: Int32) -> [UInt8] {
    var out: [UInt8] = []
    var buf = [UInt8](repeating: 0, count: 4096)
    while true {
        let n = read(fd, &buf, buf.count)
        if n <= 0 { break }
        out += buf[0..<n]
    }
    return out
}

final class SpeechAttachTests: XCTestCase {
    func testRejectsAnythingButTheNamedWriteOnlyFifos() {
        let (host, helper) = socketPair()
        defer { close(host); close(helper) }
        let rx = SocketFdReceiver(fd: helper)
        let sinks = SpeechSinks()
        let t = SpeechTarget.dictation(id: "d")

        let file = open("/dev/null", O_WRONLY)
        sendFds(host, "a", [file])
        XCTAssertEqual(sinks.accept(rx, sessionId: "a", target: t, channels: [.mic]), .notFifo)

        let p1 = makePipe()
        sendFds(host, "b", [p1.r])
        XCTAssertEqual(sinks.accept(rx, sessionId: "b", target: t, channels: [.mic]), .notWriteOnly)
        close(p1.w)

        let (p2, p3) = (makePipe(), makePipe())
        sendFds(host, "c", [p2.w, p3.w])
        XCTAssertEqual(sinks.accept(rx, sessionId: "c", target: t, channels: [.mic]), .count)
        // The rejected write ends were closed: their readers see EOF.
        XCTAssertEqual(readAll(p2.r).count, 0); close(p2.r)
        XCTAssertEqual(readAll(p3.r).count, 0); close(p3.r)

        let p4 = makePipe()
        sendFds(host, "zz", [p4.w])
        XCTAssertEqual(sinks.accept(rx, sessionId: "e", target: t, channels: [.mic]), .sessionMismatch)
        close(p4.r)

        XCTAssertEqual(sinks.accept(rx, sessionId: "f", target: t, channels: [.mic], timeoutMs: 50), .noFds)
        XCTAssertEqual(sinks.accept(nil, sessionId: "g", target: t, channels: [.mic]), .unsupported)
        XCTAssertFalse(sinks.has(t))
    }

    func testAcceptedFdsGetPcmThenEofWhenTheCaptureEnds() {
        let (host, helper) = socketPair()
        defer { close(host); close(helper) }
        let sinks = SpeechSinks()
        let t = SpeechTarget.recording(audioPath: "/tmp/r.wav")
        let mic = makePipe(), sys = makePipe()
        sendFds(host, "/tmp/r.wav", [mic.w, sys.w])
        XCTAssertNil(sinks.accept(SocketFdReceiver(fd: helper), sessionId: "/tmp/r.wav", target: t, channels: [.mic, .system]))
        sinks.write(t, .mic, [Float](repeating: 0.5, count: 320))
        sinks.write(t, .system, [Float](repeating: -0.5, count: 160))
        sinks.end(t)
        let m = readAll(mic.r), s = readAll(sys.r)
        XCTAssertEqual(m.count, 640)
        XCTAssertEqual(s.count, 320)
        XCTAssertEqual(Int16(bitPattern: UInt16(m[0]) | UInt16(m[1]) << 8), Int16(0.5 * Float(Int16.max)))
        close(mic.r); close(sys.r)
    }

    /// A part attached to the module is transcribed there only: the helper's own recogniser is not loaded or fed,
    /// no `asr` line and no asr.unavailable; the module's channels get the capture and EOF at stop.
    func testAnAttachedRecordingSkipsTheHelpersOwnRecogniser() async throws {
        let (host, helper) = socketPair()
        defer { close(host); close(helper) }
        let out = Box<[HelperMessage]>([])
        let asr = FakeASR()
        let c = HelperController(
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: [FakeAudio("sck")], transcribers: [asr], helperVersion: "t",
                             os: "linux", features: [], speechFds: SocketFdReceiver(fd: helper)),
            send: { m in out.mutate { $0.append(m) } }, now: { 0 })
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-sp-\(UUID()).wav").path
        let mic = makePipe(), sys = makePipe()
        sendFds(host, "s1", [mic.w, sys.w])
        await c.handle(.speechAttach(sessionId: "s1", target: .recording(audioPath: path), channels: [.mic, .system]))
        XCTAssertTrue(out.value.contains(.speechAttached(sessionId: "s1")))
        await c.handle(.recordingStart(audioPath: path))
        await c.onSamples([Float](repeating: 0.2, count: 16_000))
        await c.handle(.recordingStop())
        XCTAssertEqual(asr.text, "", "the helper's recogniser was not fed")
        XCTAssertFalse(out.value.contains { if case .asr = $0 { true } else { false } })
        XCTAssertFalse(out.value.contains { if case .error(let code, _, _, _, _, _, _, _) = $0 { code == "asr.unavailable" } else { false } })
        XCTAssertEqual(readAll(mic.r).count, 0, "EOF at stop (FakeAudio delivered no blocks)")
        XCTAssertEqual(readAll(sys.r).count, 0)
        close(mic.r); close(sys.r)
        try? FileManager.default.removeItem(atPath: path)
    }

    func testTheProtocolCarriesSpeechAttach() throws {
        let line = #"{"channels":["mic"],"sessionId":"d1","target":{"id":"d1","kind":"dictation"},"type":"speech.attach"}"#
        guard case .success(let m) = HostMessage.parse(line: line) else { return XCTFail("parse") }
        XCTAssertEqual(m, .speechAttach(sessionId: "d1", target: .dictation(id: "d1"), channels: [.mic]))
        if case .success = HostMessage.parse(line: #"{"channels":[],"sessionId":"d1","target":{"id":"d1","kind":"dictation"},"type":"speech.attach"}"#) {
            XCTFail("empty channels must be refused")
        }
    }
}
