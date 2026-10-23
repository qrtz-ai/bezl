//! `note:delete` through the app service onto the real qrtz-lifecycle `NoteRuntime` (feature "runtime"):
//! the runtime deletes rows and audio and emits `note:changed`, the response stays `{ok: true}`, and the
//! meeting being recorded is refused with a clear E_VALIDATION (nothing removed, no event).
use qrtz_app::ports::*;
use qrtz_app::runtime_port::{HelperAvailability, NoteRuntimePort};
use qrtz_app::*;
use qrtz_ipc::Router;
use qrtz_lifecycle::model::{RecordingPhase, StopReason};
use qrtz_lifecycle::runtime::*;
use qrtz_storage::Db;
use serde_json::{json, Value};
use std::sync::Arc;

/// QRT-352: nano and ctrl are experimental apps; these tests run them, so experimental apps is on (if unset).
fn experimental_apps_on(cfg: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    let cfg = cfg.as_ref();
    if !cfg.exists() {
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(cfg, r#"{"app":{"experimentalApps":true}}"#).unwrap();
    }
    cfg.to_path_buf()
}

/// A scripted helper: every command goes to the test, which answers like the mock helper.
struct Helper(tokio::sync::mpsc::UnboundedSender<HelperCommand>);
impl HelperPort for Helper {
    fn send(&self, cmd: &HelperCommand) -> bool {
        self.0.send(cmd.clone()).is_ok()
    }
}

async fn call(r: &Router<Shared>, ch: &str, payload: Value) -> Value {
    r.dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":"note","payload":payload}))
        .await
}

fn changed(rx: &mut tokio::sync::broadcast::Receiver<(String, Value)>) -> Vec<Value> {
    let mut v = vec![];
    while let Ok((n, p)) = rx.try_recv() {
        if n == "note:changed" {
            v.push(p["id"].clone());
        }
    }
    v
}

#[tokio::test]
async fn note_delete_runs_the_runtime_delete_and_refuses_the_live_meeting() {
    let dir = tempfile::tempdir().unwrap();
    let rec = dir.path().join("rec");
    let (tx, mut cmds) = tokio::sync::mpsc::unbounded_channel::<HelperCommand>();
    std::fs::create_dir_all(&rec).unwrap();
    let events = Events::new();
    let cfg: SharedConfig = ConfigStore::open(experimental_apps_on(dir.path().join("c.json"))).into();
    let db = InlineDb::new(Db::open(dir.path().join("qrtz.db")).unwrap());
    let rt = NoteRuntime::new(RuntimeDeps {
        store: Arc::new(Store(InlineDb(db.0.clone()))),
        steps: Arc::new(CoreSteps {
            db: InlineDb(db.0.clone()),
        }),
        helper: Arc::new(Helper(tx)),
        events: Arc::new(qrtz_app::wiring::AppSink::new(
            events.clone(),
            Arc::new(NoShell),
            cfg.clone(),
        )),
        config: Arc::new(RuntimeConfig::default),
        clock: Arc::new(SystemClock),
        ids: Arc::new(|| "m1".into()),
        random: Arc::new(|| 0.5),
        recordings_dir: rec.clone(),
        retry_scale: 0.01,
        fault_step: None,
        default_title: Arc::new(utc_recording_title),
        pending_delete: None,
        calendar: None,
    });
    let svc = AppService::new(AppDeps {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "tauri test".into(),
        },
        config: cfg,
        runtime: Arc::new(NoteRuntimePort::new(rt.clone(), HelperAvailability::Native)),
        storage: Arc::new(StubStorage),
        talk: Arc::new(StubTalk),
        shell: Arc::new(NoShell),
        helper: None,
        events,
    });
    let r = Router::new(Shared(svc.clone()));
    let mut rx = svc.events.subscribe();

    let answer = {
        let rt = rt.clone();
        tokio::spawn(async move {
            while let Some(c) = cmds.recv().await {
                let reply = match c {
                    HelperCommand::PermissionsCheck => json!({"type":"permissions.status","status":{
                        "accessibility":"granted","screen-recording":"granted","microphone":"granted"}}),
                    HelperCommand::RecordingStart { audio_path, .. } => {
                        json!({"type":"recording.started","audioPath": audio_path})
                    }
                    HelperCommand::RecordingStop { audio_path } => {
                        json!({"type":"recording.stopped","audioPath": audio_path,"durationMs":1000})
                    }
                    _ => continue,
                };
                rt.helper_json(&reply);
            }
        })
    };
    rt.helper_json(&json!({"type":"hello","protocol":1,"helperVersion":"t","os":"macos","features":[]}));
    rt.recover().await.unwrap(); // startup recovery opens the gate for start
    let wav = rec.join("m1.wav");
    let started = rt.start(Some("Standup".into())).await.unwrap();
    assert_eq!(started.phase, RecordingPhase::Recording);
    std::fs::write(&wav, b"RIFF").unwrap();

    // The live meeting: refused, nothing removed, no event, still recording.
    changed(&mut rx); // the start's own note:changed
    let refused = call(&r, "note:delete", json!({"id":"m1"})).await;
    assert_eq!(refused["error"]["code"], "E_VALIDATION");
    assert_eq!(refused["error"]["message"], DELETE_WHILE_RECORDING);
    assert_eq!(refused["error"]["retryable"], false);
    assert!(wav.exists());
    assert_eq!(rt.session().phase, RecordingPhase::Recording);
    assert!(changed(&mut rx).is_empty());

    rt.stop(StopReason::Manual).await.unwrap();
    changed(&mut rx);

    // Processing or done: the runtime deletes rows + audio; the runtime emits note:changed; `{ok: true}`.
    let ok = call(&r, "note:delete", json!({"id":"m1"})).await;
    assert_eq!(ok["data"], json!({"ok": true}), "{ok}");
    assert!(!wav.exists(), "audio removed");
    assert_eq!(changed(&mut rx), vec![json!("m1")]);
    assert_eq!(
        rt.pipeline_debug(),
        [0, 0, 0, 0],
        "no pipeline outlives the delete"
    );
    rt.kill();
    answer.abort();
}
