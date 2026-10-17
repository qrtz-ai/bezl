//! qrtz talk through the Tauri backend: `build_app` wires `TalkService` (crates/qrtz-talk) over the supervisor,
//! the validated bus, the app secret store and the shell clipboard; the Rust mock helper plays the dictation
//! (`dictation.started`, growing `dictation.asr` partials, `dictation.stopped`, `insert.result`).
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_app::storage_port::StorageShell;
use qrtz_app::wiring::{build_app, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::{ProcessSpawn, SupervisorOptions};
use qrtz_ipc::Router;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Shell;
impl ShellHooks for Shell {
    fn set_hud_expanded(&self, e: bool) -> bool {
        e
    }
    fn notify(&self, _n: Notice) {}
}

#[derive(Default)]
struct Clip(Mutex<Vec<String>>);
impl StorageShell for Clip {
    fn write_clipboard(&self, text: &str) {
        self.0.lock().unwrap().push(text.to_owned());
    }
}

fn mock_helper_bin() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let p = exe.parent().unwrap().parent().unwrap().join("qrtz-mock-helper");
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
    p
}

fn app(dir: &Path, clip: Arc<Clip>, helper: bool) -> BuiltApp {
    app_with(dir, clip, helper, DevKnobs::default())
}

fn app_with(dir: &Path, clip: Arc<Clip>, helper: bool, knobs: DevKnobs) -> BuiltApp {
    build_app(AppBuild {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "tauri test".into(),
        },
        data_dir: dir.to_path_buf(),
        helper: helper.then(|| {
            (
                ProcessSpawn::helper(mock_helper_bin()),
                HelperAvailability::Simulated,
            )
        }),
        supervisor: SupervisorOptions::default(),
        shell: Arc::new(Shell),
        storage_shell: clip,
        talk: None,
        speech: None,
        steps: None,
        knobs,
    })
    .unwrap()
}

async fn call(r: &Router<Shared>, ch: &str, payload: Value) -> Value {
    let v = r
        .dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":"talk","payload":payload}))
        .await;
    assert_eq!(v["ok"], true, "{ch}: {v}");
    v["data"].clone()
}

async fn until<F: Fn() -> bool>(what: &str, f: F) {
    for _ in 0..400 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn until_phase(r: &Router<Shared>, phase: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..1200 {
        last = call(r, "talk:state", Value::Null).await;
        if last["phase"] == phase {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for talk phase {phase}; last {last}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dictation_through_the_tauri_backend_streams_inserts_and_lands_in_history() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let clip = Arc::new(Clip::default());
    let a = app(&data, clip.clone(), true);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    let mut bus = a.service.events.subscribe();
    a.service.start_helper();
    let svc = a.service.clone();
    until("helper", || svc.helper_ready()).await;
    assert_eq!(call(&r, "talk:state", Value::Null).await["phase"], "idle");

    call(&r, "talk:start", Value::Null).await;
    let v = until_phase(&r, "listening").await;
    assert!(v["id"].is_string(), "{v}");
    // The mock's growing partials stream into the view.
    let mut partial = String::new();
    for _ in 0..200 {
        let v = call(&r, "talk:state", Value::Null).await;
        partial = format!(
            "{} {}",
            v["text"].as_str().unwrap_or(""),
            v["partial"].as_str().unwrap_or("")
        );
        if partial.contains("from") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(partial.contains("hello"), "{partial}");

    call(&r, "talk:stop", Value::Null).await;
    until_phase(&r, "idle").await;

    let h = call(&r, "talk:history", Value::Null).await;
    let entries = h.as_array().expect("history list");
    assert_eq!(entries.len(), 1, "{h}");
    let e = &entries[0];
    assert!(
        e["text"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .starts_with("hello"),
        "{e}"
    );
    // Persisted where Electron kept it.
    assert!(data.join("talk.json").exists());

    // The events reached the validated bus.
    let mut seen = Vec::new();
    while let Ok((name, payload)) = bus.try_recv() {
        if name.starts_with("talk:") {
            seen.push((name, payload));
        }
    }
    assert!(seen
        .iter()
        .any(|(n, p)| n == "talk:state" && p["phase"] == "listening"));
    assert!(seen.iter().any(|(n, _)| n == "talk:history-changed"));

    // talk:copy goes through the shell clipboard.
    let id = e["id"].as_str().unwrap();
    assert_eq!(call(&r, "talk:copy", json!({"id": id})).await["ok"], true);
    assert_eq!(
        clip.0.lock().unwrap().last().map(String::as_str),
        e["text"].as_str()
    );

    // Reopening reads the same history back.
    if let Some(h) = a.service.helper() {
        h.stop();
    }
    drop(r);
    drop(a);
    let b = app(&data, Arc::new(Clip::default()), false);
    let r = Router::new(Shared(b.service.clone())).validate_responses(true);
    let h = call(&r, "talk:history", Value::Null).await;
    assert_eq!(h.as_array().map(Vec::len), Some(1), "{h}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_helper_a_dictation_fails_cleanly_and_holds_no_live_work() {
    let tmp = tempfile::tempdir().unwrap();
    let a = app(&tmp.path().join("data"), Arc::new(Clip::default()), false);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    let v = r
        .dispatch(json!({"v":1,"id":"t","channel":"talk:start","version":1,"subApp":"talk","payload":null}))
        .await;
    // Either an error reply or an error view; never a silent listening state.
    let phase = v["data"]["phase"].as_str().unwrap_or("");
    assert!(v["ok"] == false || phase == "error" || phase == "idle", "{v}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_ne!(call(&r, "talk:state", Value::Null).await["phase"], "listening");
}

async fn settings(r: &Router<Shared>, ch: &str, payload: Value) -> Value {
    let v = r
        .dispatch(json!({"v":1,"id":"s","channel":ch,"version":1,"subApp":"settings","payload":payload}))
        .await;
    assert_eq!(v["ok"], true, "{ch}: {v}");
    v["data"].clone()
}

async fn dictate_once(r: &Router<Shared>) -> Value {
    call(r, "talk:start", Value::Null).await;
    until_phase(r, "listening").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    call(r, "talk:stop", Value::Null).await;
    until_phase(r, "idle").await;
    let h = call(r, "talk:history", Value::Null).await;
    h[0].clone()
}

/// Jared: talk has no LLM cleanup (speed), and there is no LLM client at all (ADR 0076). The text goes in as soon
/// as the key is released (on-device rules only: fillers, punctuation, dictionary).
/// An old config that still has the removed `aiCleanup` / `cleanupModel` / `cleanupTimeoutMs` loads; those keys
/// are dropped. Parity rows 34/35 (docs/e2e-parity.md) are retired by this.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn talk_cleans_up_on_device_and_old_llm_cleanup_settings_load() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("config.json"),
        json!({"talk": {"aiCleanup": true, "cleanupModel": "anthropic/claude-haiku-5.5", "cleanupTimeoutMs": 900,
                        "showPill": false}})
        .to_string(),
    )
    .unwrap();
    let a = app_with(&data, Arc::new(Clip::default()), true, DevKnobs::default());
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    let cfg = settings(&r, "config:get", Value::Null).await;
    assert_eq!(
        cfg["talk"]["showPill"], false,
        "the rest of the old talk settings survive: {cfg}"
    );
    for k in ["aiCleanup", "cleanupModel", "cleanupTimeoutMs"] {
        assert!(cfg["talk"].get(k).is_none(), "talk.{k} is gone: {cfg}");
    }
    a.service.start_helper();
    let svc = a.service.clone();
    until("helper", || svc.helper_ready()).await;
    let e = dictate_once(&r).await;
    assert_eq!(e["cleanup"], "rules", "{e}");
    if let Some(h) = a.service.helper() {
        h.stop();
    }
}
