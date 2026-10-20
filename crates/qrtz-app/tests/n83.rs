#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! N83: the Tauri backend runs the full step chain, SummarySteps (ADR 0075: the summary on this Mac, here the mock
//! helper's model) → CoreSteps (finalize; index is a no-op since search is full-text), end to end through
//! `build_app` and the router with the Rust mock helper. There is no provider to call (ADR 0076: entirely offline):
//! an old saved OpenRouter key is deleted at launch and can't be stored again.
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_app::storage_port::NoStorageShell;
use qrtz_app::wiring::{build_app, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::{ProcessSpawn, SupervisorOptions};
use qrtz_ipc::Router;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

struct Shell;
impl ShellHooks for Shell {
    fn set_hud_expanded(&self, e: bool) -> bool {
        e
    }
    fn notify(&self, _n: Notice) {}
}

fn mock_helper_bin() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let p = exe.parent().unwrap().parent().unwrap().join("qrtz-mock-helper");
    if !p.exists() {
        let st = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "-q",
                "-p",
                "qrtz-helper-bridge",
                "--bin",
                "qrtz-mock-helper",
            ])
            .status()
            .unwrap();
        assert!(st.success());
    }
    p
}

fn app(dir: &Path) -> BuiltApp {
    build_app(AppBuild {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "tauri test".into(),
        },
        data_dir: dir.to_path_buf(),
        helper: Some((
            ProcessSpawn::helper(mock_helper_bin()),
            HelperAvailability::Simulated,
        )),
        supervisor: SupervisorOptions::default(),
        helper_layout: Default::default(),
        modules_from_lock: false,
        packages: None,
        shell: Arc::new(Shell),
        storage_shell: Arc::new(NoStorageShell),
        talk: None,
        speech: None,
        steps: None,
        knobs: DevKnobs {
            retry_scale: Some(0.001),
            ..Default::default()
        },
    })
    .unwrap()
}

async fn call(r: &Router<Shared>, sub: &str, ch: &str, payload: Value) -> Value {
    let v = r
        .dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":sub,"payload":payload}))
        .await;
    assert_eq!(v["ok"], true, "{ch}: {v}");
    v["data"].clone()
}

async fn until<F: Fn() -> bool>(what: &str, f: F) {
    for _ in 0..600 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Record with the mock helper (it finalizes one line), stop, wait for the pipeline to settle.
async fn record_one(a: &BuiltApp, r: &Router<Shared>, title: &str) -> (String, Value) {
    let svc = a.service.clone();
    until("helper", || svc.helper_ready()).await;
    let s = call(r, "note", "note:start", json!({"title": title})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    call(r, "note", "note:stop", json!(null)).await;
    let rt2 = a.runtime.clone();
    until("done", || rt2.session().phase.as_str() == "done").await;
    // `done` is the session; wait for the pipeline's last step too.
    let mut p = Value::Null;
    for _ in 0..400 {
        p = call(r, "note", "note:pipeline", json!({"id": id})).await;
        if ["summary", "index"].iter().all(|s| {
            matches!(
                p["steps"][s]["status"].as_str(),
                Some("done" | "failed" | "skipped")
            )
        }) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    (id, p)
}

/// ADR 0075/0076: the summary is written on this Mac (the helper's on-device model; here the mock helper's). An
/// OpenRouter key saved by an old build is deleted at launch, and there is no `secrets:set` (ADR 0078). Search is full-text
/// over the notes DB.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_on_device_summary_through_the_tauri_backend_and_full_text_search() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("secrets.json"), br#"{"llm.openrouter":"sk-or-old"}"#).unwrap();
    let a = app(&data);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    assert!(!data.join("secrets.json").exists(), "deleted at launch");
    let v = r
        .dispatch(
            json!({"v":1,"id":"t","channel":"secrets:set","version":1,"subApp":"settings",
                         "payload":{"name": "llm.openrouter", "value": "sk-or-test"}}),
        )
        .await;
    assert_eq!(v["ok"], false, "no secrets channel: {v}");

    let (id, p) = record_one(&a, &r, "Ship review").await;
    assert_eq!(p["steps"]["summary"]["status"], "done", "{p}");
    // No embeddings since the "ask" chat went: the index step is a no-op, never a failure.
    assert!(
        matches!(p["steps"]["index"]["status"].as_str(), Some("done" | "skipped")),
        "{p}"
    );
    let d = call(&r, "note", "note:get", json!({"id": id})).await;
    assert_eq!(d["summary"]["provider"], "apple-intelligence", "{d}");
    assert!(
        d["summary"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("mock on-device"),
        "{d}"
    );
    assert!(d["summary"].get("llmError").is_none_or(Value::is_null), "{d}");

    // Search is full-text over the notes DB (FTS): the transcript finds the meeting.
    let hits = call(&r, "note", "note:list", json!({"query": "mock helper"})).await;
    assert!(
        hits.as_array()
            .is_some_and(|h| h.iter().any(|m| m["meeting"]["id"] == id.as_str())),
        "{hits}"
    );
    a.runtime.kill();
}
