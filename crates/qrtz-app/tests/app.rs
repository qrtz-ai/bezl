//! Headless integration: the real router + AppService + config file + a real helper child process (the Rust
//! mock helper binary from qrtz-helper-bridge), no Tauri. Covers what main/ipc.ts + config-store did in Electron.
use qrtz_app::ports::*;
use qrtz_app::secrets::MemorySecretStore;
use qrtz_app::*;
use qrtz_helper_bridge::{ProcessSpawn, Supervisor, SupervisorOptions};
use qrtz_ipc::{contract, Router};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

fn info() -> AppInfo {
    AppInfo {
        name: "qrtz".into(),
        version: "0.1.0".into(),
        platform: "linux".into(),
        arch: "x64".into(),
        shell: "tauri test".into(),
    }
}

struct Hud(AtomicBool);
impl ShellHooks for Hud {
    fn set_hud_expanded(&self, e: bool) -> bool {
        self.0.store(e, Ordering::SeqCst);
        e
    }
}

fn mock_helper_bin() -> PathBuf {
    // Built alongside this test by cargo (workspace member bin).
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap();
    let p = dir.join("qrtz-mock-helper");
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

fn app(cfg: &Path, helper: Option<Supervisor>) -> (Arc<AppService>, Router<Shared>, Arc<Hud>) {
    let hud = Arc::new(Hud(AtomicBool::new(false)));
    let svc = AppService::new(AppDeps {
        info: info(),
        config: ConfigStore::open(cfg),
        secrets: Arc::new(MemorySecretStore::default()),
        runtime: Arc::new(StubRuntime::default()),
        storage: Arc::new(StubStorage),
        talk: Arc::new(StubTalk),
        shell: hud.clone(),
        helper,
    });
    let router = Router::new(Shared(svc.clone()));
    (svc, router, hud)
}

async fn call(r: &Router<Shared>, sub: &str, ch: &str, payload: Value) -> Value {
    r.dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":sub,"payload":payload}))
        .await
}

#[test]
fn every_contract_channel_has_exactly_one_owner() {
    for name in contract().channels.keys() {
        assert!(
            AppService::owner(name).is_some(),
            "IPC channel {name} is not routed in crates/qrtz-app/src/ports.rs: add it to a *_CHANNELS list"
        );
        let n = [RUNTIME_CHANNELS, STORAGE_CHANNELS, APP_CHANNELS]
            .iter()
            .filter(|l| l.contains(&name.as_str()))
            .count();
        assert!(n <= 1, "{name} routed twice");
    }
    for l in [RUNTIME_CHANNELS, STORAGE_CHANNELS, APP_CHANNELS] {
        for c in l {
            assert!(
                contract().channels.contains_key(*c),
                "{c} routed but not in the contract"
            );
        }
    }
}

#[tokio::test]
async fn app_channels_and_capabilities() {
    let dir = tempfile::tempdir().unwrap();
    let (_svc, r, hud) = app(&dir.path().join("config.json"), None);
    let res = call(&r, "settings", "app:get-info", Value::Null).await;
    assert_eq!(res["data"]["electron"], "tauri test");
    assert_eq!(
        call(&r, "shell", "helper:status", Value::Null).await["data"]["state"],
        "stopped"
    );
    assert_eq!(
        call(&r, "shell", "hud:set-expanded", json!({"expanded": true})).await["data"]["expanded"],
        true
    );
    assert!(hud.0.load(Ordering::SeqCst));
    assert_eq!(
        call(&r, "note", "config:set", json!({"patch": {}})).await["error"]["code"],
        "E_FORBIDDEN"
    );
    assert_eq!(
        call(&r, "gallery", "note:list", json!({})).await["error"]["code"],
        "E_FORBIDDEN"
    );
    // secrets: names only come back
    assert_eq!(
        call(
            &r,
            "settings",
            "secrets:set",
            json!({"name": "llm.openrouter", "value": "sk"})
        )
        .await["ok"],
        true
    );
    assert_eq!(
        call(&r, "settings", "secrets:list", Value::Null).await["data"],
        json!(["llm.openrouter"])
    );
    assert_eq!(
        call(
            &r,
            "settings",
            "secrets:set",
            json!({"name": "llm.openrouter", "value": null})
        )
        .await["ok"],
        true
    );
    assert_eq!(
        call(&r, "settings", "secrets:list", Value::Null).await["data"],
        json!([])
    );
}

#[tokio::test]
async fn stub_ports_answer_idle_and_refuse_actions_honestly() {
    let dir = tempfile::tempdir().unwrap();
    let (_svc, r, _) = app(&dir.path().join("c.json"), None);
    assert_eq!(
        call(&r, "note", "note:list", json!({"query": ""})).await["data"],
        json!([])
    );
    assert_eq!(
        call(&r, "note", "note:session", Value::Null).await["data"]["phase"],
        "idle"
    );
    assert_eq!(
        call(&r, "note", "note:live-transcript", Value::Null).await["data"],
        Value::Null
    );
    let start = call(&r, "note", "note:start", json!({})).await;
    assert_eq!(start["error"]["code"], "E_UNAVAILABLE");
    assert_eq!(start["error"]["retryable"], false);
    assert_eq!(
        call(&r, "talk", "talk:state", Value::Null).await["data"]["phase"],
        "idle"
    );
    assert_eq!(
        call(&r, "talk", "talk:start", Value::Null).await["error"]["code"],
        "E_UNAVAILABLE"
    );
    assert_eq!(
        call(&r, "settings", "db:health", Value::Null).await["data"]["ok"],
        false
    );
    assert_eq!(
        call(&r, "note", "note:chat", json!({"question": "q"})).await["error"]["code"],
        "E_UNAVAILABLE"
    );
}

#[tokio::test]
async fn config_defaults_validation_persistence_and_events() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.json");
    let (svc, r, _) = app(&file, None);
    let mut ev = svc.events.subscribe();
    let cfg = call(&r, "settings", "config:get", Value::Null).await["data"].clone();
    assert_eq!(cfg, contract().config_defaults);
    let bad = call(&r, "settings", "config:set", json!({"patch": {"theme": "neon"}})).await;
    assert_eq!(bad["error"]["code"], "E_VALIDATION");
    let ok = call(
        &r,
        "settings",
        "config:set",
        json!({"patch": {"theme": "light", "app": {"launchAtLogin": true}, "bogus": 1}}),
    )
    .await;
    assert_eq!(ok["data"]["theme"], "light");
    assert_eq!(
        ok["data"]["app"]["menuBarIcon"], true,
        "deep merge keeps siblings"
    );
    assert!(
        ok["data"].get("bogus").is_none(),
        "unknown keys stripped like zod"
    );
    let (name, payload) = ev.recv().await.unwrap();
    assert_eq!(name, "config:changed");
    assert_eq!(payload["app"]["launchAtLogin"], true);
    // scoped write from Note
    assert_eq!(
        call(&r, "note", "note:set-auto-record", json!({"enabled": true})).await["data"]["autoRecord"],
        true
    );
    // persisted + reloaded
    let reloaded = ConfigStore::open(&file);
    assert_eq!(reloaded.get()["theme"], "light");
    assert!(reloaded.bool_at(&["note", "autoRecord"]));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn config_load_falls_back_per_field_and_migrates_memo() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.json");
    std::fs::write(
        &file,
        r#"{"theme":"neon","logLevel":"debug","memo":{"autoRecord":true},"note":{"floatingHud":false}}"#,
    )
    .unwrap();
    let c = ConfigStore::open(&file);
    assert_eq!(c.get()["theme"], "dark", "invalid key → its default only");
    assert_eq!(c.get()["logLevel"], "debug", "valid siblings survive");
    assert!(c.bool_at(&["note", "autoRecord"]), "memo.* migrated to note.*");
    assert!(!c.bool_at(&["note", "floatingHud"]));
    let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(on_disk.get("memo").is_none(), "legacy file rewritten once");
    std::fs::write(&file, "{garbage").unwrap();
    assert_eq!(ConfigStore::open(&file).get(), &contract().config_defaults);
}

async fn next_event(
    rx: &mut tokio::sync::broadcast::Receiver<(String, Value)>,
    want: impl Fn(&str, &Value) -> bool,
) -> Value {
    timeout(Duration::from_secs(10), async {
        loop {
            let (n, p) = rx.recv().await.unwrap();
            if want(&n, &p) {
                return p;
            }
        }
    })
    .await
    .expect("event never arrived")
}

#[tokio::test]
async fn helper_child_process_end_to_end_through_the_service() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("crash-once");
    let mut spawn = ProcessSpawn::helper(mock_helper_bin());
    spawn.env = vec![
        ("QRTZ_MOCK_PERMISSIONS".into(), "microphone=denied".into()),
        ("QRTZ_MOCK_CRASH_ONCE".into(), marker.to_string_lossy().into()),
    ];
    let sup = Supervisor::spawn(
        SupervisorOptions {
            base_delay: Duration::from_millis(50),
            ..Default::default()
        },
        Arc::new(spawn),
    );
    let (svc, r, _) = app(&dir.path().join("c.json"), Some(sup));
    let mut ev = svc.events.subscribe();
    svc.start_helper();
    // first run crashes → backoff is broadcast as helper:status, then it comes back
    next_event(&mut ev, |n, p| n == "helper:status" && p["state"] == "backoff").await;
    let up = next_event(&mut ev, |n, p| n == "helper:status" && p["state"] == "running").await;
    assert_eq!(up["restarts"], 1);
    // permissions.check is sent once running; the status fans out and lands in permissions:get
    let perms = next_event(&mut ev, |n, _| n == "permissions:changed").await;
    assert_eq!(perms["microphone"], "denied");
    assert_eq!(
        call(&r, "note", "permissions:get", Value::Null).await["data"]["microphone"],
        "denied"
    );
    assert_eq!(
        call(&r, "note", "note:session", Value::Null).await["data"]["helper"],
        "ok"
    );
    assert_eq!(
        call(&r, "shell", "helper:status", Value::Null).await["data"]["state"],
        "running"
    );
    svc.helper().unwrap().stop();
}

#[test]
fn talk_messages_route_to_talk() {
    assert!(is_talk_message("dictation.asr"));
    assert!(is_talk_message("insert.result"));
    assert!(is_talk_message("hotkey"));
    assert!(!is_talk_message("asr"));
    assert!(!is_talk_message("recording.started"));
}
