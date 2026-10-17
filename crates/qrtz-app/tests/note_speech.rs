//! Step 9 / N84 for note, end to end on Linux: with the speech module configured, every recording part gets a
//! module session (attached to the mock helper's capture before `recording.start`), its words become the live
//! transcript through `NoteRuntime::speech_event`, and the helper's own `asr` lines are no longer used. Without an
//! installed module the part is `asr-unavailable` (audio still recorded) and nothing goes to a cloud engine.
#![cfg(target_os = "linux")]
mod support;
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_app::speech::SpeechSetup;
use qrtz_app::storage_port::NoStorageShell;
use qrtz_app::wiring::{build_app, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::{ProcessSpawn, SupervisorOptions};
use qrtz_ipc::Router;
use qrtz_speech::testkit::key;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use support::speech::*;

struct Shell;
impl ShellHooks for Shell {
    fn set_hud_expanded(&self, e: bool) -> bool {
        e
    }
    fn notify(&self, _n: Notice) {}
}

fn app(dir: &Path, env: &SpeechEnv) -> BuiltApp {
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
            ProcessSpawn::helper(bin("qrtz-mock-helper", "qrtz-helper-bridge")),
            HelperAvailability::Simulated,
        )),
        supervisor: SupervisorOptions::default(),
        shell: Arc::new(Shell),
        storage_shell: Arc::new(NoStorageShell),
        speech: Some(SpeechSetup {
            store: env.store.clone(),
            opts: env.opts.clone(),
            base_url: env.srv.base.clone(),
        }),
        talk: None,
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
    for _ in 0..800 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Record ~2.5 s, stop, wait until the pipeline settled; the meeting's note:get.
async fn record(a: &BuiltApp, r: &Router<Shared>) -> (Value, Value) {
    let svc = a.service.clone();
    until("helper", || svc.helper_ready()).await;
    let s = call(r, "note", "note:start", json!({"title": "speech"})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(2600)).await;
    let live = serde_json::to_value(a.runtime.session()).unwrap();
    call(r, "note", "note:stop", json!(null)).await;
    let rt2 = a.runtime.clone();
    until("done", || rt2.session().phase.as_str() == "done").await;
    let mut d = Value::Null;
    for _ in 0..200 {
        d = call(r, "note", "note:get", json!({"id": id})).await;
        if d["pipeline"]["steps"]["summary"]["status"] == "done" || d["summary"].is_object() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (live, d)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_module_transcribes_each_part_and_the_helpers_own_asr_is_not_used() {
    let tmp = tempfile::tempdir().unwrap();
    let env = speech_env(tmp.path()).await;
    env.install("1.0.0", "the quick brown fox").await;
    let a = app(&tmp.path().join("data"), &env);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    let (live, d) = record(&a, &r).await;
    assert!(!live.to_string().contains("asr-unavailable"), "{live}");
    let text = d.to_string();
    assert!(
        !text.contains("Hello from the mock helper."),
        "the helper's asr lines are dropped once the module transcribes: {d}"
    );
    assert!(
        text.contains("the") && text.contains("quick"),
        "module words in the transcript: {d}"
    );
    assert_eq!(
        a.note_speech.as_ref().unwrap().active(),
        0,
        "the part's session ended"
    );
    a.runtime.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_an_installed_module_the_part_is_speech_unavailable_and_still_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let env = speech_env(tmp.path()).await; // nothing installed
    let a = app(&tmp.path().join("data"), &env);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    let (live, d) = record(&a, &r).await;
    assert!(live.to_string().contains("asr-unavailable"), "shown live: {live}");
    assert!(!d.to_string().contains("Hello from the mock helper."), "{d}");
    assert!(
        d["durationMs"].as_f64().unwrap_or(0.0) > 0.0 || d.to_string().contains("audio"),
        "{d}"
    );
    a.runtime.kill();
}

#[test]
fn the_builds_speech_source_is_all_or_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, pk) = key(1, "k1");
    let (_, root) = key(9, "root");
    let none = SpeechSetup::from_vars(tmp.path(), "0.1.0", |_| None).unwrap();
    assert!(none.is_none(), "nothing set: no module in this build");
    let partial = SpeechSetup::from_vars(tmp.path(), "0.1.0", |k| {
        (k == "QRTZ_SPEECH_URL").then(|| "http://x".into())
    });
    assert!(partial.err().unwrap().contains("QRTZ_SPEECH_ROOT_KEY"));
    let all = |bad: bool| {
        let (pk, root) = (pk.hex(), root.hex());
        move |k: &str| match k {
            "QRTZ_SPEECH_URL" => Some("http://127.0.0.1:1".to_owned()),
            "QRTZ_SPEECH_KEY_ID" => Some("k1".into()),
            "QRTZ_SPEECH_KEY" => Some(if bad { "zz".into() } else { pk.clone() }),
            "QRTZ_SPEECH_ROOT_KEY_ID" => Some("root".into()),
            "QRTZ_SPEECH_ROOT_KEY" => Some(root.clone()),
            _ => None,
        }
    };
    assert!(SpeechSetup::from_vars(tmp.path(), "0.1.0", all(true)).is_err());
    let s = SpeechSetup::from_vars(tmp.path(), "0.1.0", all(false))
        .unwrap()
        .unwrap();
    assert_eq!(s.base_url, "http://127.0.0.1:1");
    assert!(tmp.path().join("speech").is_dir());
}
