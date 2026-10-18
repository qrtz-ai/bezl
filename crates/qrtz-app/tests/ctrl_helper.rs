//! Slice 5b (ADR 0110): ctrl over the supervised helper process. A fake child behind the real `Supervisor` speaks
//! the echo/nano lines (hello, heartbeats) and answers `ctrl.*` lines with the in-memory `FakeHelper`, so a run goes
//! router → `CtrlService` → `HelperClient` → `SupervisorWire` → supervisor stdio → child and back, with the app's
//! own line pump (`start_helper`) running next to it.
use qrtz_app::ctrl_helper::{ctrl_line_id, SupervisorWire};
use qrtz_app::ports::*;
use qrtz_app::*;
use qrtz_ctrl::helper::{FakeHelper, FakeWorld, HelperClient, Wire};
use qrtz_ctrl::service::{CtrlService, SystemClock};
use qrtz_helper_bridge::{ChildIo, HelperState, Spawn, Supervisor, SupervisorOptions};
use qrtz_ipc::Router;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// A helper child: `answer_ctrl = false` is an older helper that ignores ctrl lines.
struct FakeChild {
    fake: Arc<FakeHelper>,
    answer_ctrl: bool,
}

impl Spawn for FakeChild {
    fn spawn(&self) -> std::io::Result<ChildIo> {
        let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<String>();
        let (_exit_tx, exit_rx) = oneshot::channel();
        let (kill_tx, _kill_rx) = oneshot::channel();
        let fake = self.fake.clone();
        let answer = self.answer_ctrl;
        let _ = out_tx.send(r#"{"type":"heartbeat"}"#.into());
        tokio::spawn(async move {
            let _keep = _exit_tx;
            while let Some(line) = in_rx.recv().await {
                if ctrl_line_id(&line).is_none() || !answer {
                    continue;
                }
                // Noise first: a heartbeat and another request's reply must not be taken for this one.
                let _ = out_tx.send(r#"{"type":"heartbeat"}"#.into());
                let _ = out_tx.send(r#"{"type":"ctrl.restore.result","id":"not-yours","ok":true}"#.into());
                let reply = fake.roundtrip(line.trim()).unwrap();
                let _ = out_tx.send(reply);
            }
        });
        Ok(ChildIo {
            lines: out_rx,
            stdin: in_tx,
            exit: exit_rx,
            kill: kill_tx,
            speech: None,
        })
    }
}

fn opts() -> SupervisorOptions {
    SupervisorOptions {
        heartbeat_timeout: Duration::ZERO,
        ..SupervisorOptions::default()
    }
}

async fn running(sup: &Supervisor) {
    let t0 = Instant::now();
    while sup.status().state != HelperState::Running {
        assert!(t0.elapsed() < Duration::from_secs(5), "helper never came up");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn service(dir: &std::path::Path, sup: Option<Supervisor>, wire: Option<SupervisorWire>) -> Arc<AppService> {
    let svc = AppService::new(AppDeps {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "darwin".into(),
            arch: "arm64".into(),
            shell: "tauri test".into(),
        },
        config: ConfigStore::open(dir.join("config.json")).into(),
        runtime: Arc::new(StubRuntime::default()),
        storage: Arc::new(StubStorage),
        talk: Arc::new(StubTalk),
        shell: Arc::new(NoShell),
        helper: sup,
        events: Events::new(),
    });
    let mut ctrl = CtrlService::open(dir.join("ctrl"), Box::new(SystemClock::default()));
    if let Some(w) = wire {
        ctrl = ctrl.with_helper(Arc::new(HelperClient::new(Box::new(w))));
    }
    svc.set_ctrl(Arc::new(ctrl));
    svc
}

async fn call(r: &Router<Shared>, ch: &str, payload: Value) -> Value {
    r.dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":"ctrl","payload":payload}))
        .await
}

async fn run_task(r: &Router<Shared>, text: &str) -> Value {
    let t = call(
        r,
        "ctrl:create-task",
        json!({"text": text, "schedule": {"kind": "once", "at": null}}),
    )
    .await;
    let id = t["data"]["id"].as_str().unwrap().to_string();
    let run = call(r, "ctrl:run-now", json!({"id": id})).await;
    assert_eq!(run["ok"], true, "{run}");
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_executes_through_the_supervised_helper_stdio_next_to_the_echo_nano_pump() {
    let d = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeHelper::new(FakeWorld::typical(4)));
    let sup = Supervisor::spawn(
        opts(),
        Arc::new(FakeChild {
            fake: fake.clone(),
            answer_ctrl: true,
        }),
    );
    let wire = SupervisorWire::new(sup.clone(), tokio::runtime::Handle::current());
    let svc = service(d.path(), Some(sup.clone()), Some(wire));
    svc.start_helper(); // the app's own pump sees the ctrl replies too and skips them
    running(&sup).await;
    let r = Router::new(Shared(svc));
    let run = run_task(&r, "close all my tabs").await;
    assert_eq!(run["data"]["status"], "done", "{run}");
    let v = call(&r, "ctrl:run", json!({"runId": run["data"]["id"]})).await;
    assert_eq!(v["data"]["state"]["violations"], 0, "{v}");
    let w = fake.world();
    assert_eq!(w.acted.len(), 1);
    assert_eq!(w.frontmost_pid, 100, "the user's app stayed frontmost");
    let safari = w.apps.iter().find(|a| a.bundle_id == "com.apple.Safari").unwrap();
    assert_eq!(safari.tabs, 0);
    // An irreversible task still holds before acting over the real stdio.
    let run = run_task(&r, "empty the trash").await;
    assert_eq!(run["data"]["status"], "needs-confirm", "{run}");
    assert_eq!(fake.world().acted.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn while_the_helper_is_down_runs_stop_at_planned_without_waiting() {
    let d = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeHelper::new(FakeWorld::typical(4)));
    // Never started: the supervisor is stopped, like a helper that's restarting.
    let sup = Supervisor::spawn(
        opts(),
        Arc::new(FakeChild {
            fake: fake.clone(),
            answer_ctrl: true,
        }),
    );
    let wire = SupervisorWire::new(sup.clone(), tokio::runtime::Handle::current());
    assert!(!wire.up());
    assert_eq!(
        wire.roundtrip(r#"{"type":"ctrl.catalog","id":"c0"}"#)
            .unwrap_err(),
        "helper.not_running"
    );
    let r = Router::new(Shared(service(d.path(), None, Some(wire))));
    let t0 = Instant::now();
    let run = run_task(&r, "close all my tabs").await;
    assert_eq!(run["data"]["status"], "planned", "{run}");
    assert!(t0.elapsed() < Duration::from_secs(2));
    assert!(fake.world().acted.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_helper_that_never_answers_ctrl_times_out_instead_of_hanging() {
    let fake = Arc::new(FakeHelper::new(FakeWorld::typical(4)));
    let sup = Supervisor::spawn(
        opts(),
        Arc::new(FakeChild {
            fake,
            answer_ctrl: false,
        }),
    );
    sup.start();
    running(&sup).await;
    let wire = SupervisorWire::new(sup.clone(), tokio::runtime::Handle::current())
        .with_timeout(Duration::from_millis(300));
    let client = HelperClient::new(Box::new(wire));
    let t0 = Instant::now();
    let e = tokio::task::spawn_blocking(move || client.catalog().map(|_| ()))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(e, "helper.timeout");
    assert!(t0.elapsed() < Duration::from_secs(2));
}

#[test]
fn only_ctrl_lines_are_ctrl_lines() {
    assert_eq!(
        ctrl_line_id(r#"{"type":"ctrl.act.result","id":"c3","ok":true,"err":null}"#).as_deref(),
        Some("c3")
    );
    assert_eq!(ctrl_line_id(r#"{"type":"heartbeat"}"#), None);
    // Transcript text that happens to mention a ctrl type is not a ctrl line.
    assert_eq!(
        ctrl_line_id(r#"{"type":"dictation.asr","text":"{\"type\":\"ctrl.act\"}"}"#),
        None
    );
    assert_eq!(ctrl_line_id("{nope"), None);
}

/// The real Swift helper binary behind the real supervisor (set `QRTZ_HELPER_BIN`; skipped otherwise). Its reader
/// thread routes ctrl lines to `CtrlStdio`; on Linux the backend answers `platform.unsupported` at once, on macOS
/// the catalog comes from LaunchServices with the default browser. Echo/nano lines keep flowing (hello first).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_real_helper_binary_answers_ctrl_lines_on_its_stdio() {
    let Some(bin) = std::env::var_os("QRTZ_HELPER_BIN") else {
        eprintln!("QRTZ_HELPER_BIN not set: skipped");
        return;
    };
    let sup = Supervisor::spawn(
        opts(),
        Arc::new(qrtz_helper_bridge::ProcessSpawn::helper(bin.into())),
    );
    let mut lines = sup.subscribe_lines();
    sup.start();
    running(&sup).await;
    let wire = SupervisorWire::new(sup.clone(), tokio::runtime::Handle::current());
    let client = HelperClient::new(Box::new(wire));
    let r = tokio::task::spawn_blocking(move || client.catalog())
        .await
        .unwrap();
    if cfg!(target_os = "macos") {
        let c = r.expect("catalog");
        assert!(!c.apps.is_empty());
    } else {
        assert_eq!(r.unwrap_err(), "platform.unsupported");
    }
    let mut saw_hello = false;
    while let Ok(l) = lines.try_recv() {
        saw_hello |= l.contains("\"hello\"");
    }
    assert!(saw_hello, "echo/nano lines still flow");
    sup.stop();
}
