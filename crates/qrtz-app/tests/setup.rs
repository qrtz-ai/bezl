//! Onboarding / settings "speech" E2E: the real router, the real updater and health check, the speech module
//! served by the qrtz-speech test kit's bundle server. Download with progress, pause and resume, a quit mid-way
//! resumes on the next launch (from the `.part` files), a user pause survives a relaunch, a damaged download
//! errors and retries, delete, private dirs; the search model's download error leaves search full-text only.
mod support;

use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_app::speech::SpeechSetup;
use qrtz_app::storage_port::NoStorageShell;
use qrtz_app::wiring::{build_app, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::{ProcessSpawn, SupervisorOptions};
use qrtz_ipc::Router;
use qrtz_speech::testkit::{mock_bundle_with, sha, sign, Bundle};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use support::speech::*;

const PAD: &str = "engine/pad.bin";
const PAD_BYTES: usize = 3 * 1024 * 1024;

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

async fn call(r: &Router<Shared>, ch: &str, payload: Value) -> Value {
    let v = r
        .dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":"settings","payload":payload}))
        .await;
    assert_eq!(v["ok"], true, "{ch}: {v}");
    v["data"].clone()
}

async fn until(what: &str, f: impl AsyncFn() -> bool) {
    for _ in 0..1200 {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

/// The mock module bundle plus a padding file big enough to pause in the middle of.
fn padded(env: &SpeechEnv, version: &str) -> Bundle {
    let entry = std::fs::read(bin("qrtz-mock-speech", "qrtz-speech")).unwrap();
    let cfg = json!({"version": version, "say": "the quick brown fox"});
    let mut b = mock_bundle_with(&env.sk, "k1", version, cfg, "The quick brown fox.", entry);
    let pad: Vec<u8> = (0..PAD_BYTES).map(|i| (i % 253) as u8).collect();
    let mut m: Value = serde_json::from_slice(&b.manifest).unwrap();
    m["files"]
        .as_array_mut()
        .unwrap()
        .push(json!({"path": PAD, "size": pad.len(), "sha256": sha(&pad)}));
    b.manifest = serde_json::to_vec(&m).unwrap();
    b.sig = sign(&env.sk, &b.manifest);
    b.files.push((PAD.into(), pad));
    b
}

fn throttle(env: &SpeechEnv, version: &str, on: bool) {
    let p = format!("{version}/{PAD}");
    let mut f = env.srv.faults.lock().unwrap();
    if on {
        f.throttle.insert(p, Duration::from_millis(4));
    } else {
        f.throttle.remove(&p);
    }
}

/// Part of the (throttled) padding file is on disk: progress reports every 256 KiB per file.
fn into_pad(s: &Value) -> bool {
    let d = &s["download"];
    d["total"].as_u64().unwrap() > 0
        && d["done"].as_u64().unwrap() > d["total"].as_u64().unwrap() - PAD_BYTES as u64 + 300 * 1024
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn speech_downloads_with_progress_pauses_resumes_and_deletes() {
    let tmp = tempfile::tempdir().unwrap();
    let env = speech_env(tmp.path()).await;
    let b = padded(&env, "1.0.0");
    env.srv.publish(&b);
    throttle(&env, "1.0.0", true);
    let a = app(&tmp.path().join("data"), &env);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    let mut ev = a.service.events.subscribe();

    let s = call(&r, "speech:status", json!(null)).await;
    assert_eq!(s["configured"], true);
    assert_eq!(s["installed"], Value::Null);
    let s = call(&r, "speech:check", json!(null)).await;
    assert_eq!(s["latest"], "1.0.0");
    assert!(s["latestBytes"].as_u64().unwrap() > PAD_BYTES as u64, "{s}");

    call(&r, "speech:download", json!({})).await;
    until("progress", async || {
        let s = call(&r, "speech:status", json!(null)).await;
        s["download"]["state"] == "downloading" && into_pad(&s)
    })
    .await;
    let s = call(&r, "speech:pause", json!(null)).await;
    assert_eq!(s["download"]["state"], "paused", "{s}");
    let marker = env.store.root().join("download.json");
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&marker).unwrap()).unwrap()["paused"],
        true
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = call(&r, "speech:status", json!(null)).await;
    assert_eq!(s["download"]["state"], "paused", "stays paused: {s}");
    assert_eq!(s["installed"], Value::Null);
    // delete is refused only while the transfer runs; paused is fine but we resume instead.
    throttle(&env, "1.0.0", false);
    call(&r, "speech:resume", json!(null)).await;
    until("installed", async || {
        let s = call(&r, "speech:status", json!(null)).await;
        s["installed"] == "1.0.0" && s["download"]["state"] == "idle"
    })
    .await;
    assert!(!marker.exists(), "the marker goes once the install finished");
    let ranges = env.srv.faults.lock().unwrap().ranges.clone();
    assert!(
        ranges.iter().any(|(p, _)| p == &format!("1.0.0/{PAD}")),
        "resume continued from the .part file: {ranges:?}"
    );
    let mut states = vec![];
    while let Ok((name, p)) = ev.try_recv() {
        if name == "speech:status" {
            states.push(p["download"]["state"].as_str().unwrap().to_owned());
        }
    }
    for want in ["checking", "downloading", "paused", "idle"] {
        assert!(states.iter().any(|s| s == want), "{want} in {states:?}");
    }
    let s = call(&r, "speech:status", json!(null)).await;
    assert!(s["bytes"].as_u64().unwrap() > PAD_BYTES as u64, "{s}");
    assert_eq!(mode(env.store.root()), 0o700);

    let s = call(&r, "speech:delete", json!(null)).await;
    assert_eq!(s["installed"], Value::Null, "{s}");
    assert_eq!(s["bytes"], 0, "{s}");
    a.runtime.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_download_a_quit_cut_short_resumes_on_the_next_launch_and_a_user_pause_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let env = speech_env(tmp.path()).await;
    env.srv.publish(&padded(&env, "1.0.0"));
    throttle(&env, "1.0.0", true);
    let data = tmp.path().join("data");
    {
        let a = app(&data, &env);
        let r = Router::new(Shared(a.service.clone())).validate_responses(true);
        call(&r, "speech:download", json!({})).await;
        until("progress", async || {
            into_pad(&call(&r, "speech:status", json!(null)).await)
        })
        .await;
        // The user paused, then quit: the next launch shows paused and fetches nothing.
        call(&r, "speech:pause", json!(null)).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        a.runtime.kill();
    }
    let hits = |e: &SpeechEnv| {
        e.srv
            .faults
            .lock()
            .unwrap()
            .hits
            .get(&format!("1.0.0/{PAD}"))
            .copied()
            .unwrap_or(0)
    };
    let before = hits(&env);
    {
        let a = app(&data, &env);
        let r = Router::new(Shared(a.service.clone())).validate_responses(true);
        a.setup.speech.resume_pending();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let s = call(&r, "speech:status", json!(null)).await;
        assert_eq!(s["download"]["state"], "paused", "{s}");
        assert_eq!(hits(&env), before, "a user pause is not undone by a relaunch");
        a.runtime.kill();
    }
    // A quit in the middle (the marker says wanted, not paused): the next launch resumes in the background.
    let marker = env.store.root().join("download.json");
    std::fs::write(&marker, br#"{"force":false,"paused":false}"#).unwrap();
    throttle(&env, "1.0.0", false);
    let a = app(&data, &env);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    a.setup.speech.resume_pending();
    until("installed after relaunch", async || {
        call(&r, "speech:status", json!(null)).await["installed"] == "1.0.0"
    })
    .await;
    assert!(
        env.srv
            .faults
            .lock()
            .unwrap()
            .ranges
            .iter()
            .any(|(p, _)| p == &format!("1.0.0/{PAD}")),
        "continued from the .part file"
    );
    assert!(!marker.exists());
    a.runtime.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_damaged_download_is_an_error_with_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let env = speech_env(tmp.path()).await;
    env.srv.publish(&padded(&env, "1.0.0"));
    env.srv
        .faults
        .lock()
        .unwrap()
        .corrupt
        .insert(format!("1.0.0/{PAD}"));
    let a = app(&tmp.path().join("data"), &env);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    call(&r, "speech:download", json!({})).await;
    until("error", async || {
        call(&r, "speech:status", json!(null)).await["download"]["state"] == "error"
    })
    .await;
    let s = call(&r, "speech:status", json!(null)).await;
    assert_eq!(s["download"]["errorKind"], "checksum", "{s}");
    let text = s["download"]["error"].as_str().unwrap();
    assert_eq!(text, text.to_lowercase(), "lowercase");
    assert_eq!(s["installed"], Value::Null);
    env.srv.faults.lock().unwrap().corrupt.clear();
    call(&r, "speech:download", json!({})).await;
    until("installed on retry", async || {
        call(&r, "speech:status", json!(null)).await["installed"] == "1.0.0"
    })
    .await;
    a.runtime.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_config_with_the_search_model_settings_loads_and_nothing_is_fetched() {
    // The "ask" chat and its on-device search model are gone (search is full-text): the old keys are dropped
    // and the model is never downloaded.
    let tmp = tempfile::tempdir().unwrap();
    let env = speech_env(tmp.path()).await;
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("config.json"),
        br#"{"note":{"searchModelDownload":true,"backgroundIndexing":true,"autoSummarize":false}}"#,
    )
    .unwrap();
    let a = app(&data, &env);
    let r = Router::new(Shared(a.service.clone())).validate_responses(true);
    let cfg = a.service.config();
    assert_eq!(cfg["note"]["autoSummarize"], false, "{cfg}");
    assert!(cfg["note"].get("searchModelDownload").is_none(), "{cfg}");
    assert!(cfg["note"].get("backgroundIndexing").is_none(), "{cfg}");
    let _ = call(&r, "speech:status", json!(null)).await;
    let hits = env.srv.faults.lock().unwrap().hits.clone();
    assert!(
        hits.keys().all(|k| k.starts_with("denylist")),
        "no model fetch: {hits:?}"
    );
    a.runtime.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_saved_key_is_deleted_at_launch() {
    // ADR 0076/0078: qrtz is entirely offline and keeps no keys, so whatever an older build saved (cloud keys and
    // any other) is deleted, along with a crash-left `.tmp`.
    let tmp = tempfile::tempdir().unwrap();
    let env = speech_env(tmp.path()).await;
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("secrets.json"),
        br#"{"asr.deepgram":"dg-old","asr.elevenlabs":"el-1","llm.openrouter":"sk-or-1","test.token":"t-1"}"#,
    )
    .unwrap();
    std::fs::write(data.join("secrets.json.tmp"), b"{\"half").unwrap();
    let a = app(&data, &env);
    assert!(!data.join("secrets.json").exists());
    assert!(!data.join("secrets.json.tmp").exists());
    a.runtime.kill();
}
