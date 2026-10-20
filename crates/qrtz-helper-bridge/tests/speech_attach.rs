#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! N55 (ADR 0046 §1): the helper is not a deputy. Over the real speech socket to the Rust mock helper (which
//! checks like the Swift helper): exactly the named count, FIFOs only, write-only only, the right session; the
//! accepted write ends get PCM while the capture runs and are closed (EOF) when it ends.
#![cfg(target_os = "linux")]
use qrtz_helper_bridge::{encode_host_message, HelperState, ProcessSpawn, Supervisor, SupervisorOptions};
use serde_json::{json, Value};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

async fn helper() -> Supervisor {
    let s = Supervisor::spawn(
        SupervisorOptions::default(),
        Arc::new(ProcessSpawn::helper(PathBuf::from(env!(
            "CARGO_BIN_EXE_qrtz-mock-helper"
        )))),
    );
    s.start();
    let mut w = s.watch_status();
    timeout(Duration::from_secs(10), async {
        while w.borrow_and_update().state != HelperState::Running {
            w.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    s
}

fn pipe() -> (OwnedFd, OwnedFd) {
    let mut fds = [0; 2];
    // SAFETY: valid out array.
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    // SAFETY: fresh fds.
    unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
}

fn nonblocking(fd: &OwnedFd) {
    // SAFETY: fcntl on an owned fd.
    unsafe {
        let f = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, f | libc::O_NONBLOCK);
    }
}

fn line(sid: &str, target: Value, channels: &[&str]) -> String {
    encode_host_message(
        &json!({"type": "speech.attach", "sessionId": sid, "target": target, "channels": channels}),
    )
    .unwrap()
}

async fn answer(rx: &mut tokio::sync::broadcast::Receiver<String>, sid: &str) -> Value {
    timeout(Duration::from_secs(5), async {
        loop {
            let l = rx.recv().await.unwrap();
            let v: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
            if v["type"].as_str().is_some_and(|t| t.starts_with("speech.")) && v["sessionId"] == sid {
                return v;
            }
        }
    })
    .await
    .expect("no speech.attach answer")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n55_the_helper_refuses_anything_but_the_named_write_only_fifos() {
    let h = helper().await;
    let mut rx = h.subscribe_lines();
    let d = |id: &str| json!({"kind": "dictation", "id": id});

    // A regular file.
    let f: OwnedFd = tempfile::tempfile().unwrap().into();
    h.send_fds(line("a", d("a"), &["mic"]), b"a".to_vec(), vec![f])
        .await
        .unwrap();
    assert_eq!(answer(&mut rx, "a").await["reason"], "not-fifo");

    // A read end.
    let (r, _w) = pipe();
    h.send_fds(line("b", d("b"), &["mic"]), b"b".to_vec(), vec![r])
        .await
        .unwrap();
    assert_eq!(answer(&mut rx, "b").await["reason"], "not-write-only");

    // A socket.
    let (s1, _s2) = std::os::unix::net::UnixStream::pair().unwrap();
    h.send_fds(line("c", d("c"), &["mic"]), b"c".to_vec(), vec![s1.into()])
        .await
        .unwrap();
    assert_eq!(answer(&mut rx, "c").await["reason"], "not-fifo");

    // An extra fd (two for one channel).
    let ((_r1, w1), (_r2, w2)) = (pipe(), pipe());
    h.send_fds(line("e", d("e"), &["mic"]), b"e".to_vec(), vec![w1, w2])
        .await
        .unwrap();
    assert_eq!(answer(&mut rx, "e").await["reason"], "count");

    // The fds were sent for another session.
    let (_r3, w3) = pipe();
    h.send_fds(line("f", d("f"), &["mic"]), b"zz".to_vec(), vec![w3])
        .await
        .unwrap();
    assert_eq!(answer(&mut rx, "f").await["reason"], "session-mismatch");

    // A rejected write end was closed by the helper: our read end sees EOF once our own write end is gone.
    let (r4, w4) = pipe();
    h.send_fds(line("g", d("g"), &["mic", "system"]), b"g".to_vec(), vec![w4])
        .await
        .unwrap();
    assert_eq!(answer(&mut rx, "g").await["reason"], "count");
    let mut file = std::fs::File::from(r4);
    let mut buf = [0u8; 16];
    assert_eq!(
        file.read(&mut buf).unwrap(),
        0,
        "EOF: no copy of the write end survives"
    );
    h.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n55_accepted_fds_get_pcm_while_the_dictation_runs_then_eof() {
    let h = helper().await;
    let mut rx = h.subscribe_lines();
    let (r, w) = pipe();
    nonblocking(&w);
    h.send_fds(
        line("d1", json!({"kind": "dictation", "id": "d1"}), &["mic"]),
        b"d1".to_vec(),
        vec![w],
    )
    .await
    .unwrap();
    assert_eq!(answer(&mut rx, "d1").await["type"], "speech.attached");
    // Nothing is written before the dictation starts.
    let reader = tokio::task::spawn_blocking(move || {
        let mut f = std::fs::File::from(r);
        let mut all = vec![];
        f.read_to_end(&mut all).unwrap();
        all
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        h.send(encode_host_message(&json!({"type": "dictation.start", "id": "d1"})).unwrap())
            .await
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        h.send(encode_host_message(&json!({"type": "dictation.stop", "id": "d1"})).unwrap())
            .await
    );
    let pcm = timeout(Duration::from_secs(5), reader)
        .await
        .expect("EOF after stop")
        .unwrap();
    assert!(
        pcm.len() >= 640 * 10,
        "~500 ms of 16 kHz PCM16, got {} bytes",
        pcm.len()
    );
    assert_eq!(pcm.len() % 2, 0);
    h.stop();
}
