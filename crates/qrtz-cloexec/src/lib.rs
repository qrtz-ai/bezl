//! Close-on-exec fds without an inheritance window (review N98, §12.19).
//!
//! Linux creates pipes and sockets atomically close-on-exec (`pipe2(O_CLOEXEC)`, `SOCK_CLOEXEC`). macOS has
//! neither, so a fd is created first and marked `FD_CLOEXEC` second; a child spawned by another thread in between
//! would inherit it (a speech module pipe end leaking into the helper, say). Like Go's `syscall.ForkLock`, a
//! process-wide `RwLock` closes the gap: every non-atomic create-then-mark runs under the **read** lock (creates
//! never wait on each other), and every host child is spawned under the **write** lock via [`spawn`], so no fork
//! or `posix_spawn` can land between the two calls.
//!
//! Scope: this covers every child qrtz itself starts (the helper supervisor, the speech module). Children started
//! by third-party code that does not take [`spawn`] are not covered; on Linux nothing needs covering.
#![cfg(unix)]
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

static SPAWN_LOCK: RwLock<()> = RwLock::new(());

fn read_guard() -> RwLockReadGuard<'static, ()> {
    SPAWN_LOCK.read().unwrap_or_else(|e| e.into_inner())
}

fn write_guard() -> RwLockWriteGuard<'static, ()> {
    SPAWN_LOCK.write().unwrap_or_else(|e| e.into_inner())
}

/// Run `f` (which forks / `posix_spawn`s a child, e.g. `Command::spawn`) with no non-atomic fd creation in flight.
/// Keep `f` to the spawn itself: never wait on the child or `.await` inside it.
pub fn spawn<T>(f: impl FnOnce() -> T) -> T {
    let _g = write_guard();
    f()
}

/// Run a caller's own create-then-`FD_CLOEXEC` sequence (e.g. after `recvmsg` on macOS) inside the window guard.
pub fn creating<T>(f: impl FnOnce() -> T) -> T {
    let _g = read_guard();
    f()
}

/// Mark `fd` close-on-exec.
pub fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: plain fcntl on a caller-owned fd.
    unsafe {
        let f = libc::fcntl(fd, libc::F_GETFD);
        if f < 0 || libc::fcntl(fd, libc::F_SETFD, f | libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// A close-on-exec pipe `(read, write)`.
pub fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
    {
        // SAFETY: fds is valid for two ints.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh fds we own.
        Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "freebsd")))]
    {
        let _g = read_guard();
        // SAFETY: fds is valid for two ints.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh fds we own (dropping closes them on the error path).
        let pair = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        set_cloexec(fds[0])?;
        set_cloexec(fds[1])?;
        Ok(pair)
    }
}

/// A close-on-exec `AF_UNIX` socket pair of type `ty` (`SOCK_STREAM`, `SOCK_SEQPACKET`, ...).
pub fn socketpair(ty: libc::c_int) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
    {
        // SAFETY: valid out array.
        if unsafe { libc::socketpair(libc::AF_UNIX, ty | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh fds we own.
        Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "freebsd")))]
    {
        let _g = read_guard();
        // SAFETY: valid out array.
        if unsafe { libc::socketpair(libc::AF_UNIX, ty, 0, fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh fds we own.
        let pair = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        set_cloexec(fds[0])?;
        set_cloexec(fds[1])?;
        Ok(pair)
    }
}
