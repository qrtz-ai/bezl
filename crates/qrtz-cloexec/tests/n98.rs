//! N98: while one thread creates pipes and socket pairs, a second thread keeps spawning `ls /dev/fd` through
//! `qrtz_cloexec::spawn`; no child may ever see one of those fds. `ls` itself holds exactly one extra fd (its
//! open of /dev/fd), so a child's listing is compared with a baseline taken before any pipe exists.
use std::os::fd::{AsRawFd, OwnedFd};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn child_fds() -> Vec<i32> {
    let child = qrtz_cloexec::spawn(|| {
        Command::new("ls")
            .arg("/dev/fd/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    })
    .expect("spawn ls");
    let out = child.wait_with_output().expect("ls output");
    let mut v: Vec<i32> = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|s| s.trim_end_matches('/').parse().ok())
        .filter(|&fd| fd > 2)
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn no_pipe_fd_reaches_a_concurrently_spawned_child() {
    let baseline = child_fds().len();
    let stop = Arc::new(AtomicBool::new(false));
    let spawner = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut runs = 0u32;
            while !stop.load(Ordering::Relaxed) {
                let fds = child_fds();
                if fds.len() != baseline {
                    seen.push(fds);
                }
                runs += 1;
            }
            (runs, seen)
        })
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut created = 0u64;
    while Instant::now() < deadline {
        let mut keep: Vec<OwnedFd> = Vec::with_capacity(64);
        for _ in 0..16 {
            let (r, w) = qrtz_cloexec::pipe().expect("pipe");
            let (a, b) = qrtz_cloexec::socketpair(libc::SOCK_STREAM).expect("socketpair");
            assert!(r.as_raw_fd() > 2 && w.as_raw_fd() > 2);
            keep.extend([r, w, a, b]);
            created += 4;
        }
        drop(keep);
    }
    stop.store(true, Ordering::Relaxed);
    let (runs, seen) = spawner.join().unwrap();
    assert!(runs > 0 && created > 0, "the race must actually be exercised");
    assert!(
        seen.is_empty(),
        "children inherited host fds (baseline {baseline}): {seen:?}"
    );
}

#[test]
fn fds_are_close_on_exec() {
    let (r, w) = qrtz_cloexec::pipe().unwrap();
    let (a, b) = qrtz_cloexec::socketpair(libc::SOCK_STREAM).unwrap();
    for fd in [&r, &w, &a, &b] {
        // SAFETY: fcntl on an owned fd.
        let f = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
        assert!(f >= 0 && f & libc::FD_CLOEXEC != 0);
    }
}
