//! The speech socket between host and helper (ADR 0046 §1, N55): a `SOCK_SEQPACKET` pair; the host sends a
//! session's audio write ends as one SCM_RIGHTS message whose payload is the session id. The helper side
//! (Swift, and the Rust mock) accepts exactly the fds `speech.attach` names, each a FIFO opened write-only.
#![cfg(unix)]
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

/// The helper's end of the speech socket is inherited at this fd; `QRTZ_SPEECH_FD` names it.
pub const HELPER_SPEECH_FD: RawFd = 3;
pub const SPEECH_FD_ENV: &str = "QRTZ_SPEECH_FD";
/// At most this many fds are accepted per message (the protocol has two channels).
pub const MAX_FDS: usize = 4;

/// `(host, helper)`: both `CLOEXEC`; the host end is non-blocking (a stuck helper never blocks the supervisor).
pub fn socketpair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    #[cfg(target_os = "linux")]
    let ty = libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let ty = libc::SOCK_STREAM;
    // SAFETY: valid out array.
    if unsafe { libc::socketpair(libc::AF_UNIX, ty, 0, fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fresh fds owned by us.
    let (a, b) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&a, &b] {
        set_cloexec(fd.as_raw_fd())?;
    }
    set_nonblocking(a.as_raw_fd())?;
    Ok((a, b))
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: plain fcntl on an owned fd.
    unsafe {
        let f = libc::fcntl(fd, libc::F_GETFD);
        if f < 0 || libc::fcntl(fd, libc::F_SETFD, f | libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: plain fcntl on an owned fd.
    unsafe {
        let f = libc::fcntl(fd, libc::F_GETFL);
        if f < 0 || libc::fcntl(fd, libc::F_SETFL, f | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// One message: `payload` plus `fds` as SCM_RIGHTS.
pub fn send_fds(sock: &OwnedFd, payload: &[u8], fds: &[OwnedFd]) -> io::Result<()> {
    if fds.is_empty() || fds.len() > MAX_FDS || payload.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "1..=4 fds and a payload",
        ));
    }
    let raw: Vec<RawFd> = fds.iter().map(|f| f.as_raw_fd()).collect();
    let bytes = std::mem::size_of_val(raw.as_slice());
    // SAFETY: CMSG_SPACE is a pure size computation.
    let space = unsafe { libc::CMSG_SPACE(bytes as u32) } as usize;
    let mut cbuf = vec![0u64; space.div_ceil(8)];
    let mut iov = libc::iovec {
        iov_base: payload.as_ptr() as *mut _,
        iov_len: payload.len(),
    };
    // SAFETY: zeroed msghdr, then every pointer set to live buffers above.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(bytes as u32) as _;
        std::ptr::copy_nonoverlapping(raw.as_ptr(), libc::CMSG_DATA(c).cast::<RawFd>(), raw.len());
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_NOSIGNAL;
        #[cfg(not(target_os = "linux"))]
        let flags = 0;
        let n = libc::sendmsg(sock.as_raw_fd(), &msg, flags);
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n as usize != payload.len() {
            return Err(io::Error::other("short speech socket write"));
        }
    }
    Ok(())
}

/// What one `recvmsg` brought: the payload, every fd received (owned: dropping closes them), and whether the
/// kernel truncated the control data (more fds than [`MAX_FDS`]; the extras are already closed by the kernel).
pub struct Received {
    pub payload: Vec<u8>,
    pub fds: Vec<OwnedFd>,
    pub truncated: bool,
}

/// Wait up to `timeout` for one message (helper side).
pub fn recv_fds(sock: RawFd, timeout: Duration) -> io::Result<Option<Received>> {
    let mut pfd = libc::pollfd {
        fd: sock,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd.
    let r = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    if r == 0 {
        return Ok(None);
    }
    let mut buf = vec![0u8; 512];
    // SAFETY: size computation.
    let space = unsafe { libc::CMSG_SPACE((MAX_FDS * std::mem::size_of::<RawFd>()) as u32) } as usize;
    let mut cbuf = vec![0u64; space.div_ceil(8)];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    let mut out = Received {
        payload: vec![],
        fds: vec![],
        truncated: false,
    };
    // SAFETY: zeroed msghdr pointing at live buffers; received fds are taken into OwnedFd at once.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_CMSG_CLOEXEC | libc::MSG_DONTWAIT;
        #[cfg(not(target_os = "linux"))]
        let flags = libc::MSG_DONTWAIT;
        let n = libc::recvmsg(sock, &mut msg, flags);
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c).cast::<RawFd>();
                let len = (*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                for i in 0..len / std::mem::size_of::<RawFd>() {
                    let fd = std::ptr::read_unaligned(data.add(i));
                    out.fds.push(OwnedFd::from_raw_fd(fd));
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        out.truncated = msg.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0;
        buf.truncate(n as usize);
    }
    for f in &out.fds {
        let _ = set_cloexec(f.as_raw_fd());
    }
    out.payload = buf;
    Ok(Some(out))
}

/// N55: an audio fd must be a FIFO opened write-only. Returns the `speech.attach.rejected` reason otherwise.
pub fn check_audio_fd(fd: RawFd) -> Result<(), &'static str> {
    // SAFETY: fstat into a zeroed struct; fcntl F_GETFL.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 || (st.st_mode & libc::S_IFMT) != libc::S_IFIFO {
            return Err("not-fifo");
        }
        let fl = libc::fcntl(fd, libc::F_GETFL);
        if fl < 0 || (fl & libc::O_ACCMODE) != libc::O_WRONLY {
            return Err("not-write-only");
        }
    }
    Ok(())
}

/// The helper side of one `speech.attach` (N55): receive the message for `session_id`, require exactly
/// `channels` fds, each a write-only FIFO. On any failure every received fd is closed (dropped) and the
/// rejection reason is returned.
pub fn accept_attach(
    sock: RawFd,
    session_id: &str,
    channels: usize,
    timeout: Duration,
) -> Result<Vec<OwnedFd>, &'static str> {
    let got = match recv_fds(sock, timeout) {
        Ok(Some(r)) => r,
        Ok(None) | Err(_) => return Err("no-fds"),
    };
    if got.fds.is_empty() {
        return Err("no-fds");
    }
    if got.payload != session_id.as_bytes() {
        return Err("session-mismatch");
    }
    if got.truncated || got.fds.len() != channels {
        return Err("count");
    }
    for f in &got.fds {
        check_audio_fd(f.as_raw_fd())?;
    }
    Ok(got.fds)
}
