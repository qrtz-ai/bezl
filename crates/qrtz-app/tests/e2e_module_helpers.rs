#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0146 M2.4: `QRTZ_MODULE_HELPERS=split` runs one helper per module. Here each is the mock helper told its
//! module (`QRTZ_MODULE`); it refuses another module's commands, so a recording or a dictation only completes when
//! the host sent it to the right process.
mod e2e_support;
use e2e_support::*;
use serde_json::Value;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_helpers_route_recordings_to_nano_and_dictation_to_echo() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let a = App::launch(&data, Opts::default().split_helpers()).await;
    a.helper_ready().await;
    let m = a.app.service.module_helpers().expect("split helpers").clone();
    let procs = m.processes();
    assert_eq!(procs.len(), 3);
    wait("every module helper running", || {
        let all = procs
            .iter()
            .all(|(_, s)| s.status().state == qrtz_helper_bridge::HelperState::Running);
        async move { all.then_some(()) }
    })
    .await;
    let st = a.settings("helper:status", Value::Null).await;
    assert_eq!(st["state"], "running", "{st}");

    // Nano's process records (a misrouted recording.start would never be acked).
    let (id, _) = record_until(&a, None, |_| true).await;
    assert!(!id.is_empty());

    // Echo's process dictates.
    a.talk("talk:start", Value::Null).await;
    a.until_talk("heard the mock", |v| talk_text(v).contains("hello"))
        .await;
    a.talk("talk:stop", Value::Null).await;
    a.until_talk("idle", |v| v["phase"] == "idle").await;
    assert_eq!(a.talk_history().await.len(), 1);

    // Echo's helper restarting leaves nano's alone (its hello reaches talk only).
    let nano_restarts = m.nano.status().restarts;
    let before = a.session().await["phase"].clone();
    m.echo.restart();
    wait("echo back", || {
        let up = m.echo.status().state == qrtz_helper_bridge::HelperState::Running;
        async move { up.then_some(()) }
    })
    .await;
    assert_eq!(m.nano.status().restarts, nano_restarts);
    let s = a.session().await;
    assert_eq!(s["phase"], before, "{s}");
    assert_eq!(s["helper"], "simulated", "{s}");
    a.quit().await;
}

/// The default stays the monolith: no module helpers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_layout_is_the_monolith() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("data"), Opts::default()).await;
    a.helper_ready().await;
    assert!(a.app.service.module_helpers().is_none());
    a.quit().await;
}
