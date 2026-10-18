//! The whole Tauri backend headless: `build_app` (AppDirs → config → DB → NoteRuntime → ports) with the real Rust
//! mock helper child, driven through the router like the renderer does.
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_app::storage_port::NoStorageShell;
use qrtz_app::wiring::{build_app, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::{ProcessSpawn, SupervisorOptions};
use qrtz_ipc::Router;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Shell(Mutex<Vec<Notice>>);
impl ShellHooks for Shell {
    fn set_hud_expanded(&self, e: bool) -> bool {
        e
    }
    fn notify(&self, n: Notice) {
        self.0.lock().unwrap().push(n);
    }
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

fn app(dir: &Path, env: &[(&str, &str)], shell: Arc<Shell>) -> BuiltApp {
    app_with(dir, env, shell, Arc::new(NoStorageShell))
}

fn app_with(
    dir: &Path,
    env: &[(&str, &str)],
    shell: Arc<Shell>,
    storage_shell: Arc<dyn qrtz_app::storage_port::StorageShell>,
) -> BuiltApp {
    let mut spawn = ProcessSpawn::helper(mock_helper_bin());
    spawn.env = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    build_app(AppBuild {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "tauri test".into(),
        },
        data_dir: dir.to_path_buf(),
        helper: Some((spawn, HelperAvailability::Simulated)),
        supervisor: SupervisorOptions::default(),
        helper_layout: Default::default(),
        shell,
        storage_shell,
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
    for _ in 0..400 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_stop_process_list_export_with_the_mock_helper() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let shell = Arc::new(Shell::default());
    let a = app(
        &data,
        &[("QRTZ_MOCK_SILENT", "system@150,mic@200")],
        shell.clone(),
    );
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    let svc = a.service.clone();
    until("helper ok", || svc.helper_ready()).await;
    let rt = a.runtime.clone();

    let s = call(&r, "note", "note:start", json!({"title": "Standup"})).await;
    assert_eq!(s["phase"], "recording", "{s}");
    let id = s["meetingId"].as_str().unwrap().to_owned();
    // N29: both sources silent → noAudio → exactly one OS notice for this meeting.
    until("noAudio", || rt.session().no_audio).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        *shell.0.lock().unwrap(),
        vec![Notice::NoAudio {
            meeting_id: id.clone()
        }]
    );
    let live = call(&r, "note", "note:live-transcript", json!({})).await;
    assert_eq!(live["meetingId"], json!(id));
    let note = call(&r, "note", "note:add-note", json!({"text": "ship it"})).await;
    assert_eq!(note["meetingId"], json!(id));

    let stopped = call(&r, "note", "note:stop", json!(null)).await;
    assert!(
        matches!(
            stopped["phase"].as_str(),
            Some("stopping" | "processing" | "done")
        ),
        "{stopped}"
    );
    let rt2 = a.runtime.clone();
    until("done", || rt2.session().phase.as_str() == "done").await;
    let p = call(&r, "note", "note:pipeline", json!({"id": id})).await;
    assert_eq!(p["steps"]["finalize"]["status"], "done", "{p}");
    // ADR 0075: the summary step (SummarySteps, on this Mac) is wired, so the meeting is not summary-less (N53).
    assert_eq!(p["steps"]["summary"]["status"], "done", "{p}");

    let list = call(&r, "note", "note:list", json!({})).await;
    assert_eq!(list[0]["meeting"]["id"], json!(id));
    let detail = call(&r, "note", "note:get", json!({"id": id})).await;
    assert_eq!(detail["meeting"]["title"], "Standup");
    assert_eq!(detail["notes"][0]["text"], "ship it");
    let ex = call(&r, "note", "note:export", json!({"id": id, "save": false})).await;
    assert!(ex["filename"].as_str().unwrap().ends_with("-standup.md"), "{ex}");
    assert!(ex["markdown"].as_str().unwrap().starts_with("# Standup\n"));
    let h = call(&r, "settings", "db:health", json!(null)).await;
    assert_eq!(h["ok"], true);

    // Every data file is owner-only.
    #[cfg(unix)]
    {
        assert_eq!(mode(&data), 0o700);
        assert_eq!(mode(&data.join("recordings")), 0o700);
        assert_eq!(mode(&data.join("qrtz.db")), 0o600);
    }

    // N17: a deferred delete hides it at once and can be undone; then delete for real.
    call(&r, "note", "note:delete", json!({"id": id, "deferMs": 60_000})).await;
    assert_eq!(call(&r, "note", "note:list", json!({})).await, json!([]));
    assert!(data.join(qrtz_storage::PENDING_DELETES_FILE).exists());
    let u = call(&r, "note", "note:undo-delete", json!({"id": id})).await;
    assert_eq!(u, json!({"restored": true}));
    assert_eq!(
        call(&r, "note", "note:list", json!({})).await[0]["meeting"]["id"],
        json!(id)
    );
    call(&r, "note", "note:delete", json!({"id": id})).await;
    let list = call(&r, "note", "note:list", json!({})).await;
    assert_eq!(list, json!([]));
    a.runtime.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_no_audio_off_suppresses_the_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let shell = Arc::new(Shell::default());
    let a = app(
        tmp.path(),
        &[("QRTZ_MOCK_SILENT", "system@100,mic@100")],
        shell.clone(),
    );
    let r = Router::new(Shared(a.service.clone()));
    call(
        &r,
        "settings",
        "config:set",
        json!({"patch": {"note": {"notifyNoAudio": false}}}),
    )
    .await;
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    let svc = a.service.clone();
    until("helper", || svc.helper_ready()).await;
    call(&r, "note", "note:start", json!({})).await;
    let rt = a.runtime.clone();
    until("noAudio", || rt.session().no_audio).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(shell.0.lock().unwrap().is_empty());
    a.runtime.kill();
}

/// N38 end to end through the real bridge: the mock writes capture.degraded{no-mic} in the recording.started chunk;
/// the runtime holds it behind the ack (N37) and the session shows the recording with `no-mic`, not a lost line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mic_fails_at_start_in_the_started_chunk_degrades_the_session() {
    let tmp = tempfile::tempdir().unwrap();
    let a = app(
        tmp.path(),
        &[("QRTZ_MOCK_MIC_FAILS", "start")],
        Arc::new(Shell::default()),
    );
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    let svc = a.service.clone();
    until("helper", || svc.helper_ready()).await;
    let s = call(&r, "note", "note:start", json!({})).await;
    assert_eq!(s["phase"], "recording", "{s}");
    let rt2 = a.runtime.clone();
    until("no-mic", move || {
        serde_json::to_value(rt2.session()).unwrap()["degraded"]
            .as_array()
            .is_some_and(|d| d.contains(&json!("no-mic")))
    })
    .await;
    let s = call(&r, "note", "note:session", json!(null)).await;
    assert_eq!(s["phase"], "recording", "{s}");
    a.runtime.kill();
}

/// rust-core-api.md §9: data:export-backup → zip (progress, done), data:import-backup into a fresh library
/// (verify → import → done, then note:changed "*"); cancel answers zeros / {canceled: true}.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_export_then_import_into_a_fresh_library() {
    struct Paths(std::sync::Mutex<Option<PathBuf>>);
    impl qrtz_app::storage_port::StorageShell for Paths {
        fn backup_save_path(&self, name: &str) -> Option<PathBuf> {
            assert!(
                name.starts_with("qrtz-backup-") && name.ends_with(".zip"),
                "{name}"
            );
            self.0.lock().unwrap().clone()
        }
        fn backup_open_path(&self) -> Option<PathBuf> {
            self.0.lock().unwrap().clone()
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("b.zip");
    let paths = Arc::new(Paths(std::sync::Mutex::new(None)));
    let a = app_with(
        &tmp.path().join("a"),
        &[],
        Arc::new(Shell::default()),
        paths.clone(),
    );
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.service.start_helper();
    a.runtime.recover().await.unwrap();
    let svc = a.service.clone();
    until("helper", || svc.helper_ready()).await;
    // Cancelled dialog: zeros, no file.
    let c = call(&r, "settings", "data:export-backup", json!(null)).await;
    assert_eq!(c, json!({"meetings": 0, "audioFiles": 0, "bytes": 0}));
    let s = call(&r, "note", "note:start", json!({"title": "Backup me"})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    call(&r, "note", "note:stop", json!(null)).await;
    let rt = a.runtime.clone();
    until("done", || rt.session().phase.as_str() == "done").await;

    *paths.0.lock().unwrap() = Some(zip.clone());
    let mut ev = a.service.events.subscribe();
    let rep = call(&r, "settings", "data:export-backup", json!(null)).await;
    assert_eq!(rep["meetings"], 1, "{rep}");
    assert_eq!(rep["savedTo"], json!(zip.to_string_lossy()));
    assert!(zip.exists());
    let mut phases = vec![];
    while let Ok((name, p)) = ev.try_recv() {
        if name == "data:backup-progress" {
            phases.push(p["phase"].as_str().unwrap().to_owned());
        }
    }
    assert_eq!(phases.last().map(String::as_str), Some("done"), "{phases:?}");
    assert!(phases.iter().any(|p| p == "write"), "{phases:?}");
    a.runtime.kill();

    let b = app_with(
        &tmp.path().join("b"),
        &[],
        Arc::new(Shell::default()),
        paths.clone(),
    );
    let rb = Router::new(Shared(b.service.clone())).validate_responses(true);
    b.runtime.recover().await.unwrap();
    let mut ev = b.service.events.subscribe();
    let imp = call(&rb, "settings", "data:import-backup", json!(null)).await;
    assert_eq!(imp["imported"], 1, "{imp}");
    let mut seen = vec![];
    while let Ok((name, p)) = ev.try_recv() {
        match name.as_str() {
            "data:backup-progress" => seen.push(p["phase"].as_str().unwrap().to_owned()),
            "note:changed" => seen.push(format!("changed:{}", p["id"].as_str().unwrap())),
            _ => {}
        }
    }
    let first = |x: &str| {
        seen.iter()
            .position(|p| p == x)
            .unwrap_or_else(|| panic!("{x} in {seen:?}"))
    };
    assert!(
        first("verify") < first("import") && first("import") < first("done"),
        "{seen:?}"
    );
    assert_eq!(seen.last().map(String::as_str), Some("changed:*"), "{seen:?}");
    let list = call(&rb, "note", "note:list", json!({})).await;
    assert_eq!(list[0]["meeting"]["id"], json!(id));
    *paths.0.lock().unwrap() = None;
    let c = call(&rb, "settings", "data:import-backup", json!(null)).await;
    assert_eq!(
        c,
        json!({"canceled": true, "imported": 0, "skipped": 0, "audioFiles": 0})
    );
    b.runtime.kill();
}

#[test]
fn backup_filename_is_utc_minutes_with_dashes() {
    // 2026-10-17T14:32:59.999Z
    assert_eq!(
        qrtz_app::storage_port::backup_filename(1_792_247_579_999),
        "qrtz-backup-2026-10-17-14-32.zip"
    );
    assert_eq!(
        qrtz_app::storage_port::backup_filename(0),
        "qrtz-backup-1970-01-01-00-00.zip"
    );
}
