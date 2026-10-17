//! Port of packages/core/src/helper/supervisor.test.ts with a fake child and paused tokio time.
use qrtz_helper_bridge::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Test-side ends of a fake child.
struct FakeEnd {
    out: mpsc::UnboundedSender<String>,
    stdin: mpsc::UnboundedReceiver<String>,
    exit: Option<oneshot::Sender<ExitInfo>>,
    killed: oneshot::Receiver<()>,
}
impl FakeEnd {
    fn line(&self, l: &str) {
        let _ = self.out.send(l.into());
    }
    fn exit(&mut self, code: i32) {
        if let Some(e) = self.exit.take() {
            let _ = e.send(ExitInfo {
                code: Some(code),
                signal: None,
            });
        }
    }
    fn was_killed(&mut self) -> bool {
        self.killed.try_recv().is_ok()
    }
}

#[derive(Default)]
struct FakeSpawn {
    ends: Mutex<Vec<FakeEnd>>,
    spawns: Mutex<u32>,
    fail: Mutex<bool>,
}
impl Spawn for FakeSpawn {
    fn spawn(&self) -> std::io::Result<ChildIo> {
        *self.spawns.lock().unwrap() += 1;
        if *self.fail.lock().unwrap() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "ENOENT"));
        }
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (exit_tx, exit_rx) = oneshot::channel();
        let (kill_tx, kill_rx) = oneshot::channel();
        self.ends.lock().unwrap().push(FakeEnd {
            out: out_tx,
            stdin: in_rx,
            exit: Some(exit_tx),
            killed: kill_rx,
        });
        Ok(ChildIo {
            lines: out_rx,
            stdin: in_tx,
            exit: exit_rx,
            kill: kill_tx,
        })
    }
}
impl FakeSpawn {
    fn spawns(&self) -> u32 {
        *self.spawns.lock().unwrap()
    }
    fn with_last<R>(&self, f: impl FnOnce(&mut FakeEnd) -> R) -> R {
        f(self.ends.lock().unwrap().last_mut().unwrap())
    }
}

fn opts() -> SupervisorOptions {
    SupervisorOptions {
        name: "test".into(),
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(1000),
        max_restarts: 3,
        window: Duration::from_secs(10),
        heartbeat_timeout: Duration::from_millis(500),
    }
}
fn sup(fake: &Arc<FakeSpawn>) -> Supervisor {
    // random() = 0.5 → no jitter, delays exactly base·2^n.
    Supervisor::spawn_with_random(opts(), fake.clone(), Arc::new(|| 0.5))
}
async fn tick(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

#[test]
fn backoff_matches_ts() {
    assert_eq!(backoff_delay(0, 500, 30_000, 0.5), 500);
    assert_eq!(backoff_delay(3, 500, 30_000, 0.5), 4000);
    assert_eq!(backoff_delay(10, 500, 30_000, 0.5), 30_000);
    assert_eq!(backoff_delay(0, 500, 30_000, 0.0), 400);
    assert_eq!(backoff_delay(0, 500, 30_000, 1.0), 600);
}

#[tokio::test(start_paused = true)]
async fn starting_until_first_line_then_running_and_forwards_lines() {
    let fake = Arc::new(FakeSpawn::default());
    let s = sup(&fake);
    let mut lines = s.subscribe_lines();
    s.start();
    tick(1).await;
    assert_eq!(s.status().state, HelperState::Starting);
    assert!(!s.send("x\n".into()).await, "no sends before running");
    fake.with_last(|e| e.line(r#"{"type":"hello"}"#));
    tick(1).await;
    assert_eq!(s.status().state, HelperState::Running);
    assert_eq!(lines.recv().await.unwrap(), r#"{"type":"hello"}"#);
    assert!(s.send("{\"type\":\"ping\",\"id\":\"1\"}\n".into()).await);
    assert_eq!(
        fake.with_last(|e| e.stdin.try_recv().unwrap()),
        "{\"type\":\"ping\",\"id\":\"1\"}\n"
    );
}

#[tokio::test(start_paused = true)]
async fn restarts_with_exponential_backoff_and_resets_after_success() {
    let fake = Arc::new(FakeSpawn::default());
    let s = sup(&fake);
    s.start();
    tick(1).await;
    fake.with_last(|e| e.exit(1));
    tick(1).await;
    let st = s.status();
    assert_eq!(st.state, HelperState::Backoff);
    assert_eq!(st.restarts, 1);
    assert_eq!(st.last_error.as_deref(), Some("exited code=1 signal=null"));
    tick(98).await;
    assert_eq!(fake.spawns(), 1);
    tick(3).await;
    assert_eq!(fake.spawns(), 2, "relaunched after 100 ms");
    fake.with_last(|e| e.exit(1));
    tick(150).await;
    assert_eq!(fake.spawns(), 2, "second delay is 200 ms");
    tick(60).await;
    assert_eq!(fake.spawns(), 3);
    // a line resets the attempt counter → next delay is base again
    fake.with_last(|e| e.line("{}"));
    tick(1).await;
    fake.with_last(|e| e.exit(1));
    tick(102).await;
    assert_eq!(fake.spawns(), 4);
}

#[tokio::test(start_paused = true)]
async fn circuit_breaker_fails_after_max_restarts_in_window() {
    let fake = Arc::new(FakeSpawn::default());
    let s = sup(&fake);
    s.start();
    for _ in 0..4 {
        tick(1).await;
        fake.with_last(|e| e.exit(2));
        tick(2000).await;
    }
    let st = s.status();
    assert_eq!(st.state, HelperState::Failed);
    assert_eq!(fake.spawns(), 4);
    // start() again resets the circuit
    s.start();
    tick(1).await;
    assert_eq!(s.status().state, HelperState::Starting);
    assert_eq!(fake.spawns(), 5);
}

#[tokio::test(start_paused = true)]
async fn heartbeat_timeout_kills_and_restarts() {
    let fake = Arc::new(FakeSpawn::default());
    let s = sup(&fake);
    s.start();
    tick(1).await;
    fake.with_last(|e| e.line("{}"));
    tick(400).await;
    fake.with_last(|e| e.line("{}")); // keeps it alive
    tick(400).await;
    assert_eq!(s.status().state, HelperState::Running);
    tick(200).await;
    assert!(fake.with_last(|e| e.was_killed()));
    let st = s.status();
    assert_eq!(st.state, HelperState::Backoff);
    assert_eq!(st.last_error.as_deref(), Some("heartbeat timeout"));
    tick(150).await;
    assert_eq!(fake.spawns(), 2);
}

#[tokio::test(start_paused = true)]
async fn stale_exit_after_stop_is_ignored_and_stop_kills() {
    let fake = Arc::new(FakeSpawn::default());
    let s = sup(&fake);
    s.start();
    tick(1).await;
    s.stop();
    tick(1).await;
    assert!(fake.with_last(|e| e.was_killed()));
    fake.with_last(|e| e.exit(0));
    tick(5000).await;
    assert_eq!(s.status().state, HelperState::Stopped);
    assert_eq!(fake.spawns(), 1);
}

#[tokio::test(start_paused = true)]
async fn spawn_errors_back_off_like_exits() {
    let fake = Arc::new(FakeSpawn::default());
    *fake.fail.lock().unwrap() = true;
    let s = sup(&fake);
    s.start();
    tick(1).await;
    let st = s.status();
    assert_eq!(st.state, HelperState::Backoff);
    assert!(st.last_error.unwrap().contains("ENOENT"));
    *fake.fail.lock().unwrap() = false;
    tick(101).await;
    assert_eq!(s.status().state, HelperState::Starting);
}

#[tokio::test(start_paused = true)]
async fn status_serializes_like_ts() {
    let fake = Arc::new(FakeSpawn::default());
    let s = sup(&fake);
    assert_eq!(
        serde_json::to_value(s.status()).unwrap(),
        serde_json::json!({"state": "stopped", "restarts": 0})
    );
}

#[tokio::test(start_paused = true)]
async fn status_changes_are_observable() {
    let fake = Arc::new(FakeSpawn::default());
    let s = sup(&fake);
    let seen = Arc::new(Mutex::new(vec![]));
    let mut w = s.watch_status();
    let seen2 = seen.clone();
    tokio::spawn(async move {
        while w.changed().await.is_ok() {
            seen2.lock().unwrap().push(w.borrow().state);
        }
    });
    s.start();
    tick(1).await;
    fake.with_last(|e| e.line("{}"));
    tick(1).await;
    assert_eq!(
        *seen.lock().unwrap(),
        vec![HelperState::Starting, HelperState::Running]
    );
}
