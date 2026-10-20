//! A helper started suspended, checked, then let go (Bozniak's 902bcab review, design call (a)).
//!
//! Checking a file (`codesign -R <path>`) and then executing that path leaves a window in which the file can be
//! swapped. On macOS the launch closes it: `posix_spawn` with Apple's `POSIX_SPAWN_START_SUSPENDED` maps the
//! executable and stops the new process before its first user-space instruction; the caller's check then runs
//! against the *running* code (the kernel's view of what was mapped, by pid), and only then is the child sent
//! `SIGCONT`. A child that fails the check is `SIGKILL`ed and reaped without ever running. The pid can't be reused
//! meanwhile: the child is ours and unreaped until we wait on it.
//!
//! [`release_if_verified`] is the decision (resume or kill), over [`Suspended`] so it is tested on any platform;
//! the macOS launcher is [`ProcessSpawn::spawn_verified`].
use crate::supervisor::ChildIo;
use std::io;

/// A child that exists but hasn't run yet.
pub trait Suspended: Sized {
    fn pid(&self) -> i32;
    /// Let it run (`SIGCONT`) and hand its pipes to the supervisor.
    fn resume(self) -> io::Result<ChildIo>;
    /// `SIGKILL` it and reap it.
    fn kill(self);
}

/// `verify(pid)` passes: the child runs. It fails: the child is killed and the launch is an error naming why.
pub fn release_if_verified<S: Suspended>(
    child: S,
    verify: impl FnOnce(i32) -> Result<(), String>,
) -> io::Result<ChildIo> {
    let pid = child.pid();
    match verify(pid) {
        Ok(()) => child.resume(),
        Err(why) => {
            child.kill();
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("the started code failed its check and was killed before it ran: {why}"),
            ))
        }
    }
}

#[cfg(target_os = "macos")]
mod posix {
    use super::Suspended;
    use crate::process::{child_io, signal_number_name, ProcessSpawn};
    use crate::supervisor::{ChildIo, ExitInfo};
    use std::collections::BTreeMap;
    use std::ffi::{CString, OsString};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::sync::{Arc, Mutex};

    /// The child's pid and whether it has been reaped: a kill after the reap would hit whatever reused the pid.
    struct Reaper {
        pid: libc::pid_t,
        reaped: Mutex<bool>,
    }

    impl Reaper {
        fn kill(&self) {
            let g = self.reaped.lock().unwrap_or_else(|p| p.into_inner());
            if !*g {
                // SAFETY: plain kill(2) on our own unreaped child.
                unsafe { libc::kill(self.pid, libc::SIGKILL) };
            }
        }

        /// Block until the child exits, then reap it (under the lock, so no kill races the reap).
        fn wait(&self) -> ExitInfo {
            let ewait = || ExitInfo {
                code: None,
                signal: Some("EWAIT".into()),
            };
            loop {
                // SAFETY: zeroed siginfo_t is a valid out buffer; WNOWAIT leaves the child to the waitpid below.
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                let r = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        self.pid as libc::id_t,
                        &mut info,
                        libc::WEXITED | libc::WNOWAIT,
                    )
                };
                if r == 0 {
                    break;
                }
                if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    break;
                }
            }
            let mut g = self.reaped.lock().unwrap_or_else(|p| p.into_inner());
            let mut st: libc::c_int = 0;
            loop {
                // SAFETY: waitpid on our own child.
                let r = unsafe { libc::waitpid(self.pid, &mut st, 0) };
                if r == self.pid {
                    break;
                }
                if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    *g = true;
                    return ewait();
                }
            }
            *g = true;
            if libc::WIFEXITED(st) {
                ExitInfo {
                    code: Some(libc::WEXITSTATUS(st)),
                    signal: None,
                }
            } else if libc::WIFSIGNALED(st) {
                ExitInfo {
                    code: None,
                    signal: Some(signal_number_name(libc::WTERMSIG(st))),
                }
            } else {
                ewait()
            }
        }
    }

    /// Kills the child if the task waiting on it is dropped (the runtime shutting down), like `kill_on_drop`.
    struct KillOnDrop(Arc<Reaper>);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            self.0.kill();
        }
    }

    pub struct PosixSuspended {
        reaper: Option<Arc<Reaper>>,
        stdin: OwnedFd,
        stdout: OwnedFd,
        stderr: OwnedFd,
        speech: Option<OwnedFd>,
    }

    impl Suspended for PosixSuspended {
        fn pid(&self) -> i32 {
            self.reaper.as_ref().map_or(0, |r| r.pid)
        }

        fn resume(mut self) -> io::Result<ChildIo> {
            let reaper = self
                .reaper
                .take()
                .ok_or_else(|| io::Error::other("suspended helper was already resumed"))?;
            let fds = (
                self.stdout.try_clone(),
                self.stdin.try_clone(),
                self.stderr.try_clone(),
            );
            let (stdout, stdin, stderr) = match fds {
                (Ok(o), Ok(i), Ok(e)) => (o, i, e),
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                    self.reaper = Some(reaper);
                    return Err(e);
                }
            };
            let pipes = (|| {
                Ok::<_, io::Error>((
                    tokio::net::unix::pipe::Receiver::from_owned_fd(stdout)?,
                    tokio::net::unix::pipe::Sender::from_owned_fd(stdin)?,
                    tokio::net::unix::pipe::Receiver::from_owned_fd(stderr)?,
                ))
            })();
            let (stdout, stdin, stderr) = match pipes {
                Ok(p) => p,
                Err(e) => {
                    self.reaper = Some(reaper);
                    return Err(e);
                }
            };
            // SAFETY: SIGCONT to our own unreaped, suspended child.
            if unsafe { libc::kill(reaper.pid, libc::SIGCONT) } != 0 {
                let e = io::Error::last_os_error();
                self.reaper = Some(reaper);
                return Err(e);
            }
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            let r = reaper.clone();
            std::thread::Builder::new()
                .name(format!("qrtz-wait-{}", reaper.pid))
                .spawn(move || {
                    let _ = done_tx.send(r.wait());
                })?;
            let speech = self.speech.take();
            Ok(child_io(
                stdout,
                stdin,
                stderr,
                speech,
                move |kill_rx| async move {
                    let guard = KillOnDrop(reaper);
                    let mut done_rx = done_rx;
                    let info = tokio::select! {
                        r = &mut done_rx => r,
                        _ = kill_rx => {
                            guard.0.kill();
                            done_rx.await
                        }
                    };
                    drop(guard);
                    info.unwrap_or(ExitInfo {
                        code: None,
                        signal: Some("EWAIT".into()),
                    })
                },
            ))
        }

        fn kill(mut self) {
            if let Some(r) = self.reaper.take() {
                r.kill();
                r.wait();
            }
        }
    }

    impl Drop for PosixSuspended {
        /// Never leave a suspended child behind: one not resumed is killed and reaped.
        fn drop(&mut self) {
            if let Some(r) = self.reaper.take() {
                r.kill();
                r.wait();
            }
        }
    }

    fn cstr(b: &[u8]) -> io::Result<CString> {
        CString::new(b)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL byte in a spawn argument"))
    }

    /// A close-on-exec copy of `fd` numbered at least 64, so no `dup2` onto 0-3 can clobber another source.
    fn high(fd: RawFd) -> io::Result<OwnedFd> {
        // SAFETY: fcntl on a caller-owned fd; the new fd is ours.
        let n = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 64) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(n) })
    }

    fn check(r: libc::c_int) -> io::Result<()> {
        if r == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(r))
        }
    }

    /// `posix_spawn` `sp` with `POSIX_SPAWN_START_SUSPENDED`: piped stdin/stdout/stderr, the speech socket's helper
    /// end at fd 3, nothing else inherited (`POSIX_SPAWN_CLOEXEC_DEFAULT`), SIGPIPE back to default and an empty
    /// signal mask (what std's `Command` gives a child), the parent's environment plus `sp.env`.
    pub fn spawn_suspended(sp: &ProcessSpawn) -> io::Result<PosixSuspended> {
        let program = cstr(sp.program.as_os_str().as_bytes())?;
        let mut argv = vec![program.clone()];
        for a in &sp.args {
            argv.push(cstr(a.as_bytes())?);
        }
        let (speech_host, speech_helper) = crate::fdpass::socketpair()?;
        let mut env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        for (k, v) in &sp.env {
            env.insert(k.into(), v.into());
        }
        env.insert(
            crate::fdpass::SPEECH_FD_ENV.into(),
            crate::fdpass::HELPER_SPEECH_FD.to_string().into(),
        );
        let mut envp = vec![];
        for (k, v) in env {
            let mut kv = k.into_vec();
            kv.push(b'=');
            kv.extend(v.into_vec());
            envp.push(cstr(&kv)?);
        }
        let (in_r, in_w) = qrtz_cloexec::pipe()?;
        let (out_r, out_w) = qrtz_cloexec::pipe()?;
        let (err_r, err_w) = qrtz_cloexec::pipe()?;
        let child_side = [
            (high(in_r.as_raw_fd())?, 0),
            (high(out_w.as_raw_fd())?, 1),
            (high(err_w.as_raw_fd())?, 2),
            (high(speech_helper.as_raw_fd())?, crate::fdpass::HELPER_SPEECH_FD),
        ];
        drop((in_r, out_w, err_w, speech_helper));

        let mut argv_p: Vec<*mut libc::c_char> = argv.iter().map(|c| c.as_ptr() as *mut _).collect();
        argv_p.push(std::ptr::null_mut());
        let mut envp_p: Vec<*mut libc::c_char> = envp.iter().map(|c| c.as_ptr() as *mut _).collect();
        envp_p.push(std::ptr::null_mut());

        // SAFETY: the attr and file-action objects are initialised before use and destroyed on every path; every
        // pointer handed to posix_spawn outlives the call.
        unsafe {
            let mut fa: libc::posix_spawn_file_actions_t = std::mem::zeroed();
            check(libc::posix_spawn_file_actions_init(&mut fa))?;
            let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
            if let Err(e) = check(libc::posix_spawnattr_init(&mut attr)) {
                libc::posix_spawn_file_actions_destroy(&mut fa);
                return Err(e);
            }
            let result: io::Result<libc::pid_t> = (|| {
                for (src, dst) in &child_side {
                    check(libc::posix_spawn_file_actions_adddup2(
                        &mut fa,
                        src.as_raw_fd(),
                        *dst,
                    ))?;
                }
                let flags = libc::POSIX_SPAWN_START_SUSPENDED
                    | libc::POSIX_SPAWN_CLOEXEC_DEFAULT
                    | libc::POSIX_SPAWN_SETSIGMASK
                    | libc::POSIX_SPAWN_SETSIGDEF;
                check(libc::posix_spawnattr_setflags(&mut attr, flags as libc::c_short))?;
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                check(libc::posix_spawnattr_setsigmask(&mut attr, &set))?;
                libc::sigaddset(&mut set, libc::SIGPIPE);
                check(libc::posix_spawnattr_setsigdefault(&mut attr, &set))?;
                let mut pid: libc::pid_t = 0;
                // N98: no non-atomic close-on-exec create in flight while the child is made.
                check(qrtz_cloexec::spawn(|| {
                    libc::posix_spawn(
                        &mut pid,
                        program.as_ptr(),
                        &fa,
                        &attr,
                        argv_p.as_ptr(),
                        envp_p.as_ptr(),
                    )
                }))?;
                Ok(pid)
            })();
            libc::posix_spawnattr_destroy(&mut attr);
            libc::posix_spawn_file_actions_destroy(&mut fa);
            let pid = result?;
            drop(child_side);
            Ok(PosixSuspended {
                reaper: Some(Arc::new(Reaper {
                    pid,
                    reaped: Mutex::new(false),
                })),
                stdin: in_w,
                stdout: out_r,
                stderr: err_r,
                speech: Some(speech_host),
            })
        }
    }
}

#[cfg(target_os = "macos")]
pub use posix::PosixSuspended;

impl crate::process::ProcessSpawn {
    /// macOS: start this program suspended, run `verify(pid)` against the running code, then let it run (or kill
    /// it and fail). See the module doc.
    #[cfg(target_os = "macos")]
    pub fn spawn_verified(&self, verify: impl FnOnce(i32) -> Result<(), String>) -> io::Result<ChildIo> {
        if crate::process::is_dev_stub(&self.program) {
            return Err(io::Error::other(format!(
                "{} is the dev stub, not the real binary",
                self.program.display()
            )));
        }
        release_if_verified(posix::spawn_suspended(self)?, verify)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Log(Mutex<Vec<&'static str>>);

    struct Fake(Arc<Log>);
    impl Suspended for Fake {
        fn pid(&self) -> i32 {
            4242
        }
        fn resume(self) -> io::Result<ChildIo> {
            self.0 .0.lock().unwrap().push("SIGCONT");
            Err(io::Error::other("resumed (a fake has no pipes)"))
        }
        fn kill(self) {
            self.0 .0.lock().unwrap().push("SIGKILL");
        }
    }

    #[test]
    fn a_child_that_passes_its_check_is_resumed_and_one_that_fails_is_killed_unrun() {
        let log = Arc::new(Log::default());
        let mut seen = None;
        let r = release_if_verified(Fake(log.clone()), |pid| {
            seen = Some(pid);
            Ok(())
        });
        assert_eq!(seen, Some(4242), "the check sees the suspended child's pid");
        assert!(r.err().unwrap().to_string().contains("resumed"));
        assert_eq!(*log.0.lock().unwrap(), vec!["SIGCONT"]);

        let log = Arc::new(Log::default());
        let err = release_if_verified(Fake(log.clone()), |_| Err("not ai.qrtz.snap".into()))
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("not ai.qrtz.snap"), "{err}");
        assert_eq!(*log.0.lock().unwrap(), vec!["SIGKILL"], "killed, never resumed");
    }
}
