// The qrtz speech module's audio (ADR 0046 §1, review N55). The host spawns the module and sends its audio pipe
// write ends here as one SCM_RIGHTS message on the speech socket (fd QRTZ_SPEECH_FD, payload = sessionId), then
// `speech.attach {sessionId, target, channels}` on stdin. The helper is not a deputy: it takes exactly the fds
// named, each a FIFO opened write-only, or closes everything and answers `speech.attach.rejected`. Accepted fds
// get contiguous PCM16LE 16 kHz mono from the target capture (silence while paused) and are closed when it ends.
#if canImport(Glibc)
import Glibc
#elseif canImport(Darwin)
import Darwin
#endif
import Foundation

public enum SpeechTarget: Equatable, Hashable, Sendable {
    case dictation(id: String)
    case recording(audioPath: String)
}

public enum SpeechAttachReject: String, Sendable, Equatable {
    case noFds = "no-fds", count, notFifo = "not-fifo", notWriteOnly = "not-write-only"
    case sessionMismatch = "session-mismatch", unsupported
}

/// Receives one fd message for a session. Tests inject a fake; the helper uses `SocketFdReceiver`.
public protocol FdReceiver: Sendable {
    /// Every fd returned is owned by the caller (closed on rejection by `SpeechSinks`).
    func receive(timeoutMs: Int32) -> (payload: [UInt8], fds: [Int32], truncated: Bool)?
}

/// N55 checks on one fd: a FIFO, opened write-only.
public func checkAudioFd(_ fd: Int32) -> SpeechAttachReject? {
    var st = stat()
    guard fstat(fd, &st) == 0, (st.st_mode & S_IFMT) == S_IFIFO else { return .notFifo }
    let fl = fcntl(fd, F_GETFL)
    guard fl >= 0, (fl & O_ACCMODE) == O_WRONLY else { return .notWriteOnly }
    return nil
}

/// The helper end of the speech socket.
public struct SocketFdReceiver: FdReceiver {
    public let fd: Int32
    public init(fd: Int32) { self.fd = fd }
    /// From `QRTZ_SPEECH_FD` (nil when the host did not pass one: older host, or a dev run).
    public static func fromEnvironment() -> SocketFdReceiver? {
        guard let v = ProcessInfo.processInfo.environment["QRTZ_SPEECH_FD"], let fd = Int32(v), fd > 2 else { return nil }
        return SocketFdReceiver(fd: fd)
    }

    public func receive(timeoutMs: Int32) -> (payload: [UInt8], fds: [Int32], truncated: Bool)? {
        var p = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
        guard poll(&p, 1, timeoutMs) > 0 else { return nil }
        let maxFds = 4
        #if os(Linux)
        let align = MemoryLayout<Int>.size
        #else
        let align = MemoryLayout<UInt32>.size
        #endif
        func up(_ n: Int) -> Int { (n + align - 1) & ~(align - 1) }
        let hdr = up(MemoryLayout<cmsghdr>.size)
        let space = hdr + up(maxFds * MemoryLayout<Int32>.size)
        var payload = [UInt8](repeating: 0, count: 512)
        var control = [UInt8](repeating: 0, count: space)
        var fds: [Int32] = []
        var truncated = false
        var got = -1
        payload.withUnsafeMutableBytes { pb in
            control.withUnsafeMutableBytes { cb in
                var iov = iovec(iov_base: pb.baseAddress, iov_len: pb.count)
                withUnsafeMutablePointer(to: &iov) { iovp in
                    var msg = msghdr()
                    msg.msg_iov = iovp
                    msg.msg_iovlen = 1
                    msg.msg_control = cb.baseAddress
                    #if os(Linux)
                    msg.msg_controllen = cb.count
                    let flags = Int32(MSG_CMSG_CLOEXEC) | Int32(MSG_DONTWAIT)
                    #else
                    msg.msg_controllen = socklen_t(cb.count)
                    let flags = Int32(MSG_DONTWAIT)
                    #endif
                    got = recvmsg(fd, &msg, flags)
                    guard got >= 0 else { return }
                    truncated = (msg.msg_flags & Int32(MSG_CTRUNC)) != 0 || (msg.msg_flags & Int32(MSG_TRUNC)) != 0
                    // Walk the control messages (CMSG_FIRSTHDR / CMSG_NXTHDR by hand).
                    var off = 0
                    let total = Int(msg.msg_controllen)
                    while off + MemoryLayout<cmsghdr>.size <= total {
                        let c = cb.baseAddress!.advanced(by: off).assumingMemoryBound(to: cmsghdr.self).pointee
                        let len = Int(c.cmsg_len)
                        if len < MemoryLayout<cmsghdr>.size { break }
                        if c.cmsg_level == SOL_SOCKET && c.cmsg_type == Int32(SCM_RIGHTS) {
                            let n = (len - hdr) / MemoryLayout<Int32>.size
                            for i in 0..<max(0, n) {
                                var v: Int32 = 0
                                memcpy(&v, cb.baseAddress!.advanced(by: off + hdr + i * 4), 4)
                                fds.append(v)
                            }
                        }
                        off += up(len)
                    }
                }
            }
        }
        guard got >= 0 else { return nil }
        for f in fds { _ = fcntl(f, F_SETFD, FD_CLOEXEC) }
        return (Array(payload[0..<got]), fds, truncated)
    }
}

/// The accepted write ends, per target and channel. Thread-safe; writes never block (O_NONBLOCK, N77): a full
/// pipe drops the frame, a closed reader (EPIPE) closes that channel.
public final class SpeechSinks: @unchecked Sendable {
    private let lock = NSLock()
    private var sinks: [SpeechTarget: [(CaptureSource, Int32)]] = [:]
    public private(set) var droppedFrames = 0
    public init() {}

    /// N55: receive the fds for `sessionId` and keep them for `target`, or close all of them.
    public func accept(_ r: FdReceiver?, sessionId: String, target: SpeechTarget, channels: [CaptureSource],
                       timeoutMs: Int32 = 2000) -> SpeechAttachReject? {
        guard let r else { return .unsupported }
        guard let m = r.receive(timeoutMs: timeoutMs), !m.fds.isEmpty else { return .noFds }
        func reject(_ why: SpeechAttachReject) -> SpeechAttachReject { for f in m.fds { close(f) }; return why }
        guard m.payload == Array(sessionId.utf8) else { return reject(.sessionMismatch) }
        guard !m.truncated, m.fds.count == channels.count else { return reject(.count) }
        for f in m.fds { if let why = checkAudioFd(f) { return reject(why) } }
        for f in m.fds { _ = fcntl(f, F_SETFL, fcntl(f, F_GETFL) | O_NONBLOCK) }
        lock.lock()
        let old = sinks[target]
        sinks[target] = Array(zip(channels, m.fds))
        lock.unlock()
        for (_, f) in old ?? [] { close(f) }
        return nil
    }

    public func has(_ t: SpeechTarget) -> Bool { lock.lock(); defer { lock.unlock() }; return sinks[t] != nil }

    /// 16 kHz mono floats → PCM16LE on that channel of the target (no-op when none is attached).
    public func write(_ t: SpeechTarget, _ ch: CaptureSource, _ samples: [Float]) {
        guard !samples.isEmpty else { return }
        lock.lock()
        guard let fd = sinks[t]?.first(where: { $0.0 == ch })?.1 else { lock.unlock(); return }
        lock.unlock()
        var pcm = [UInt8](repeating: 0, count: samples.count * 2)
        for (i, s) in samples.enumerated() {
            let v = UInt16(bitPattern: Int16(max(-1, min(1, s)) * Float(Int16.max)))
            pcm[2 * i] = UInt8(v & 0xff); pcm[2 * i + 1] = UInt8(v >> 8)
        }
        let n = pcm.withUnsafeBytes { Glue.write(fd, $0.baseAddress, $0.count) }
        if n < 0 {
            if errno == EPIPE { closeChannel(t, ch) } else { lock.lock(); droppedFrames += 1; lock.unlock() }
        }
    }

    private func closeChannel(_ t: SpeechTarget, _ ch: CaptureSource) {
        lock.lock()
        let fd = sinks[t]?.first(where: { $0.0 == ch })?.1
        sinks[t]?.removeAll { $0.0 == ch }
        if sinks[t]?.isEmpty == true { sinks[t] = nil }
        lock.unlock()
        if let fd { close(fd) }
    }

    /// The capture ended: close its fds (the module sees EOF).
    public func end(_ t: SpeechTarget) {
        lock.lock()
        let fds = sinks.removeValue(forKey: t) ?? []
        lock.unlock()
        for (_, f) in fds { close(f) }
    }

    public func endAll() {
        lock.lock()
        let all = sinks.values.flatMap { $0 }
        sinks.removeAll()
        lock.unlock()
        for (_, f) in all { close(f) }
    }
}

/// `write(2)` without the name clash with the method above.
enum Glue {
    static func write(_ fd: Int32, _ p: UnsafeRawPointer?, _ n: Int) -> Int {
        #if canImport(Glibc)
        return Glibc.write(fd, p, n)
        #else
        return Darwin.write(fd, p, n)
        #endif
    }
}
