#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Run 31: ctrl under ADR 0146 M2.4's split layout (`QRTZ_MODULE_HELPERS=split`), where qrtz-ctrl is a process of its
//! own. ctrl's wire (ADR 0110), the Laya warmer (ADR 0133) and `with_helper` all sit on `ModuleHelpers::ctrl`; these
//! check that a `ctrl.*` line reaches that process and only it, that its restarts and hellos leave echo and nano
//! alone, and (with the real Swift qrtz-ctrl, `QRTZ_CTRL_BIN`) that it answers ctrl and the warm-up over its own
//! stdio and leaves on the host's shutdown line instead of waiting out the grace for a kill.
mod e2e_support;
use e2e_support::*;
use qrtz_app::ctrl_helper::SupervisorWire;
use qrtz_app::module_helpers::{HelperLayout, ModuleHelpers};
use qrtz_ctrl::helper::HelperClient;
use qrtz_ctrl::laya::{LayaWarmer, ModelConsent};
use qrtz_helper_bridge::{HelperState, Supervisor};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

fn client(sup: &Supervisor, timeout: Duration) -> Arc<HelperClient> {
    Arc::new(HelperClient::new(Box::new(
        SupervisorWire::new(sup.clone(), tokio::runtime::Handle::current()).with_timeout(timeout),
    )))
}

async fn catalog(c: &Arc<HelperClient>) -> Result<(), String> {
    let c = c.clone();
    tokio::task::spawn_blocking(move || c.catalog().map(|_| ()))
        .await
        .unwrap()
}

fn drained(rx: &mut broadcast::Receiver<String>) -> Vec<String> {
    let mut out = vec![];
    while let Ok(l) = rx.try_recv() {
        out.push(l);
    }
    out
}

async fn all_running(m: &ModuleHelpers) {
    let procs = m.processes();
    wait("every module helper running", || {
        let all = procs
            .iter()
            .all(|(_, s)| s.status().state == HelperState::Running);
        async move { all.then_some(()) }
    })
    .await;
}

fn bye() -> String {
    qrtz_helper_bridge::encode_host_message(&json!({"type": "shutdown"})).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_lines_reach_ctrls_own_process_and_its_restarts_leave_echo_and_nano_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("data"), Opts::default().split_helpers()).await;
    a.helper_ready().await;
    let m = a.app.service.module_helpers().expect("split helpers").clone();
    all_running(&m).await;
    let (mut nano_l, mut echo_l, mut ctrl_l) = (
        m.nano.subscribe_lines(),
        m.echo.subscribe_lines(),
        m.ctrl.subscribe_lines(),
    );

    // On ctrl's process the mock (no ctrl backend) takes the request without refusing it; the run would ask after
    // the timeout, as with an older helper. Nothing is refused anywhere.
    let ctrl = client(&m.ctrl, Duration::from_millis(300));
    assert!(ctrl.is_up());
    assert_eq!(catalog(&ctrl).await.unwrap_err(), "helper.timeout");
    tokio::time::sleep(Duration::from_millis(100)).await;
    for l in drained(&mut nano_l)
        .iter()
        .chain(&drained(&mut echo_l))
        .chain(&drained(&mut ctrl_l))
    {
        assert!(!l.contains("is ctrl's"), "misrouted: {l}");
    }

    // The same request on nano's process is refused, so the check above can tell a misrouted line apart.
    let wrong = client(&m.nano, Duration::from_millis(300));
    assert_eq!(catalog(&wrong).await.unwrap_err(), "helper.timeout");
    let refused = wait("nano refuses ctrl", || {
        let hit = drained(&mut nano_l)
            .into_iter()
            .any(|l| l.contains("mock nano: ctrl.catalog is ctrl's"));
        async move { hit.then_some(()) }
    });
    tokio::time::timeout(Duration::from_secs(5), refused)
        .await
        .unwrap();

    // ctrl's process restarting (its own hello) touches neither echo's nor nano's process, the session or talk,
    // and the UI's helper status is running again once it's back.
    let (nano_r, echo_r) = (m.nano.status().restarts, m.echo.status().restarts);
    let before = a.session().await;
    let mut again = m.ctrl.subscribe_lines();
    m.ctrl.restart();
    let hello = wait("ctrl's new process says hello", || {
        let hit = drained(&mut again).into_iter().any(|l| l.contains("module:ctrl"));
        async move { hit.then_some(()) }
    });
    tokio::time::timeout(Duration::from_secs(10), hello)
        .await
        .unwrap();
    all_running(&m).await;
    assert!(ctrl.is_up());
    assert_eq!(m.nano.status().restarts, nano_r);
    assert_eq!(m.echo.status().restarts, echo_r);
    let s = a.session().await;
    assert_eq!(s["phase"], before["phase"], "{s}");
    assert_eq!(s["helper"], "simulated", "{s}");
    let st = a.settings("helper:status", Value::Null).await;
    assert_eq!(st["state"], "running", "{st}");

    // ctrl's helper down: ctrl's wire says so at once (runs stop at planned), echo's and nano's stay up.
    m.ctrl.stop();
    wait("ctrl stopped", || {
        let down = m.ctrl.status().state == HelperState::Stopped;
        async move { down.then_some(()) }
    })
    .await;
    assert!(!ctrl.is_up());
    let t0 = Instant::now();
    assert_eq!(catalog(&ctrl).await.unwrap_err(), "helper.not_running");
    assert!(t0.elapsed() < Duration::from_millis(250));
    assert_eq!(m.nano.status().state, HelperState::Running);
    assert_eq!(m.echo.status().state, HelperState::Running);

    m.shutdown(bye(), Duration::from_millis(1500)).await;
    a.quit().await;
}

/// The real Swift qrtz-ctrl (set `QRTZ_CTRL_BIN`, e.g. a Linux `swift build`'s; skipped otherwise) as ctrl's helper
/// next to the mock in echo's and nano's roles.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_real_qrtz_ctrl_answers_ctrl_and_the_warm_up_and_leaves_on_shutdown() {
    let Some(bin) = std::env::var_os("QRTZ_CTRL_BIN") else {
        eprintln!("QRTZ_CTRL_BIN not set: skipped");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let o = Opts {
        helper_layout: HelperLayout::Split {
            programs: BTreeMap::from([("ctrl".to_owned(), PathBuf::from(bin))]),
        },
        ..Opts::default()
    };
    let a = App::launch(&tmp.path().join("data"), o).await;
    a.helper_ready().await;
    let m = a.app.service.module_helpers().expect("split helpers").clone();
    all_running(&m).await;

    // It is up because it said hello at once: the supervisor counts a helper as running from its first line, so a
    // silent qrtz-ctrl left every run at `planned`, the warmer idle and `helper:status` short of running.
    let st = a.settings("helper:status", Value::Null).await;
    assert_eq!(st["state"], "running", "{st}");
    let ctrl = client(&m.ctrl, Duration::from_secs(5));
    assert!(ctrl.is_up());
    let r = catalog(&ctrl).await;
    if cfg!(target_os = "macos") {
        r.expect("catalog from qrtz-ctrl");
    } else {
        assert_eq!(r.unwrap_err(), "platform.unsupported");
    }

    // ADR 0133's warmer on ctrl's process: up, settled, consent + a model on disk → one warm-up asked and answered.
    let model = tmp.path().join("laya.mlmodelc");
    std::fs::create_dir_all(&model).unwrap();
    let m2 = model.clone();
    let warmer = LayaWarmer::new(
        Box::new(move || ModelConsent {
            consented: true,
            installed: Some(m2.clone()),
        }),
        ctrl.clone(),
        true,
    );
    let r = tokio::task::spawn_blocking(move || {
        let first = warmer.tick();
        (first, warmer.tick())
    })
    .await
    .unwrap();
    let first = r.0.expect("a warm-up asked");
    if !cfg!(target_os = "macos") {
        assert_eq!(first.unwrap_err(), "platform.unsupported");
    }
    assert!(r.1.is_none(), "warmed once");

    // The host's goodbye: every process leaves well inside the grace (qrtz-ctrl used to ignore it and be killed).
    let t0 = Instant::now();
    m.shutdown(bye(), Duration::from_millis(3000)).await;
    let took = t0.elapsed();
    assert!(took < Duration::from_millis(1500), "shutdown took {took:?}");
    for (_, s) in m.processes() {
        assert_eq!(s.status().state, HelperState::Stopped);
    }
    a.quit().await;
}
