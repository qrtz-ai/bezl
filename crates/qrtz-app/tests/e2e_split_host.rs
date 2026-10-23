//! ADR 0150 N174 and ctrl loop run 31 on the split host (`QRTZ_MODULE_HELPERS=split`, each extension helper the
//! mock helper told its module): echo's helper gets its own warm, qrtz-ctrl only runs while ctrl is on and never
//! holds echo and nano back, and quit reaches all three.
mod e2e_support;
use e2e_support::*;
use qrtz_helper_bridge::HelperState;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::sync::broadcast;

fn drained(rx: &mut broadcast::Receiver<String>) -> Vec<String> {
    let mut out = vec![];
    while let Ok(l) = rx.try_recv() {
        out.push(l);
    }
    out
}

async fn state_is(s: &qrtz_helper_bridge::Supervisor, what: &str, want: HelperState) {
    wait(what, || {
        let hit = s.status().state == want;
        async move { hit.then_some(()) }
    })
    .await;
}

/// Lines from `rx` until one contains `needle` (within the deadline).
async fn saw(rx: &mut broadcast::Receiver<String>, what: &str, needle: &str) {
    let mut seen = vec![];
    wait(what, || {
        seen.extend(drained(rx));
        let hit = seen.iter().any(|l| l.contains(needle));
        async move { hit.then_some(()) }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_finished_download_warms_echo_too_and_a_missed_warm_waits_for_echos_hello() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(&tmp.path().join("data"), Opts::default().split_helpers()).await;
    a.helper_ready().await;
    let m = a.app.service.module_helpers().unwrap().clone();
    let installed = a.app.setup.speech.installed_hook().expect("warm after download");
    let (mut nano, mut echo) = (m.nano.subscribe_lines(), m.echo.subscribe_lines());
    installed();
    saw(&mut nano, "nano warms", "models warmed: mock nano").await;
    saw(&mut echo, "echo warms", "models warmed: mock echo").await;

    // Echo's app down at the end of the download: its warm goes out on its next hello.
    m.echo.stop();
    state_is(&m.echo, "echo stopped", HelperState::Stopped).await;
    installed();
    saw(&mut nano, "nano warms again", "models warmed: mock nano").await;
    m.echo.start();
    saw(
        &mut echo,
        "echo's missed warm on hello",
        "models warmed: mock echo",
    )
    .await;
    a.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qrtz_ctrl_runs_only_while_ctrl_is_on_and_quit_reaches_every_process() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("config.json"),
        r#"{"modules": {"ctrl": {"enabled": false}}, "app": {"experimentalApps": true}}"#,
    )
    .unwrap();
    let a = App::launch(&data, Opts::default().split_helpers()).await;
    a.helper_ready().await;
    let m = a.app.service.module_helpers().unwrap().clone();
    state_is(&m.nano, "nano running", HelperState::Running).await;
    state_is(&m.echo, "echo running", HelperState::Running).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        m.ctrl.status().state,
        HelperState::Stopped,
        "ctrl is off: no qrtz-ctrl"
    );
    assert_eq!(a.settings("helper:status", Value::Null).await["state"], "running");

    a.settings(
        "config:set",
        json!({"patch": {"modules": {"ctrl": {"enabled": true}}}}),
    )
    .await;
    state_is(&m.ctrl, "ctrl on: qrtz-ctrl starts", HelperState::Running).await;
    a.settings(
        "config:set",
        json!({"patch": {"modules": {"ctrl": {"enabled": false}}}}),
    )
    .await;
    state_is(&m.ctrl, "ctrl off: qrtz-ctrl stops", HelperState::Stopped).await;
    a.settings(
        "config:set",
        json!({"patch": {"modules": {"ctrl": {"enabled": true}}}}),
    )
    .await;
    state_is(&m.ctrl, "ctrl on again", HelperState::Running).await;

    // The harness quit (like the app's) says goodbye to all three.
    a.quit().await;
    for (src, s) in m.processes() {
        assert_eq!(s.status().state, HelperState::Stopped, "{src:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_qrtz_ctrl_never_holds_echo_and_nano_back() {
    let tmp = tempfile::tempdir().unwrap();
    let o = Opts {
        helper_layout: qrtz_app::module_helpers::HelperLayout::Split {
            programs: BTreeMap::from([("ctrl".to_owned(), PathBuf::from("/nonexistent/qrtz-ctrl"))]),
        },
        ..Default::default()
    };
    let a = App::launch(&tmp.path().join("data"), o).await;
    // Readiness and the UI's status follow echo and nano only.
    a.helper_ready().await;
    let m = a.app.service.module_helpers().unwrap().clone();
    wait("ctrl not running", || {
        let s = m.ctrl.status().state;
        async move { matches!(s, HelperState::Backoff | HelperState::Failed).then_some(()) }
    })
    .await;
    let st = a.settings("helper:status", Value::Null).await;
    assert_eq!(st["state"], "running", "{st}");
    assert_eq!(a.session().await["helper"], "simulated");
    a.quit().await;
}
