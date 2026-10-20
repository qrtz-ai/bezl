#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Headless integration: the real router + AppService + config file + a real helper child process (the Rust
//! mock helper binary from qrtz-helper-bridge), no Tauri. Covers what main/ipc.ts + config-store did in Electron.
use qrtz_app::ports::*;
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
        config: ConfigStore::open(cfg).into(),
        runtime: Arc::new(StubRuntime::default()),
        storage: Arc::new(StubStorage),
        talk: Arc::new(StubTalk),
        shell: hud.clone(),
        helper,
        events: Events::new(),
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
    // Audit P0-4: the notices the build carries, for settings → "open-source licenses".
    let lic = call(&r, "settings", "app:licenses", Value::Null).await;
    let text = lic["data"]["text"].as_str().unwrap_or_default();
    assert_eq!(text, qrtz_app::THIRD_PARTY_NOTICES);
    for s in [
        "Draft for legal review by Jared",
        "Licensed by NVIDIA Corporation under the NVIDIA Open Model License",
        "FluidInference/parakeet-tdt-0.6b-v2-coreml",
        "OpenMDW",
        "FluidAudio",
    ] {
        assert!(text.contains(s), "{s}");
    }
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
    // Permission prompts: the exact settings pane (only known panes), "restart qrtz" (this test shell can't), and
    // snap's button reachable from snap's own view. Off macOS nothing opens.
    let pane = call(
        &r,
        "marketplace",
        "permissions:open-settings",
        json!({"pane": "automation"}),
    )
    .await;
    assert_eq!(
        pane["data"],
        json!({"opened": cfg!(target_os = "macos"), "restart": false}),
        "{pane}"
    );
    let sr = call(
        &r,
        "marketplace",
        "permissions:open-settings",
        json!({"pane": "screen-recording"}),
    )
    .await;
    assert_eq!(sr["data"]["restart"], true);
    assert_eq!(
        call(
            &r,
            "marketplace",
            "permissions:open-settings",
            json!({"pane": "camera"})
        )
        .await["error"]["code"],
        "E_VALIDATION"
    );
    assert_eq!(
        call(&r, "marketplace", "app:restart", Value::Null).await["data"]["restarting"],
        false
    );
    assert_eq!(
        call(&r, "gallery", "app:restart", Value::Null).await["error"]["code"],
        "E_FORBIDDEN"
    );
    if !cfg!(target_os = "macos") {
        let snap = call(&r, "snap", "snap:request-access", Value::Null).await;
        assert_eq!(
            snap["data"],
            json!({"opened": false, "axTrusted": false}),
            "{snap}"
        );
        let perms = call(&r, "marketplace", "permissions:get", Value::Null).await;
        assert!(perms["ok"].as_bool().unwrap(), "{perms}");
    }
    // ADR 0078: qrtz keeps no keys, so there are no secrets channels.
    for ch in ["secrets:set", "secrets:list"] {
        let v = call(&r, "settings", ch, json!({"name": "test.token", "value": "sk"})).await;
        assert_eq!(v["ok"], false, "{ch}: {v}");
    }
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
    // The "ask" chat is gone (Jared): its channel is not in the contract any more.
    assert_eq!(
        call(&r, "note", "note:chat", json!({"question": "q"})).await["ok"],
        false
    );
}

/// ADR 0184: `onboarding.done` is the host's to write. Any renderer's `config:set` (or a module settings patch) that
/// touches `onboarding` is refused and changes nothing; only the shell's `onboarding:finish` sets it, it persists,
/// and a sub app without `os.permissions` can't call it.
#[tokio::test]
async fn only_the_host_writes_onboarding_done() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.json");
    let (_svc, r, _) = app(&file, None);
    let cfg = call(&r, "settings", "config:get", Value::Null).await["data"].clone();
    assert_eq!(
        cfg["onboarding"],
        json!({"done": false}),
        "a fresh config shows onboarding"
    );
    for patch in [
        json!({"onboarding": {"done": true}}),
        json!({"onboarding": {}}),
        json!({"theme": "light", "onboarding": {"done": true}}),
    ] {
        for sub in ["settings", "shell", "marketplace"] {
            let bad = call(&r, sub, "config:set", json!({ "patch": patch })).await;
            assert_ne!(bad["ok"], true, "{sub} {patch}: {bad}");
        }
        let bad = call(&r, "settings", "config:set", json!({ "patch": patch })).await;
        assert_eq!(bad["error"]["code"], "E_VALIDATION", "{bad}");
    }
    // A module's settings page can't reach it either (it's a global section).
    let bad = call(
        &r,
        "talk",
        "module:set-settings",
        json!({"patch": {"onboarding": {"done": true}}}),
    )
    .await;
    assert_ne!(bad["ok"], true, "{bad}");
    let cfg = call(&r, "settings", "config:get", Value::Null).await["data"].clone();
    assert_eq!(cfg["onboarding"]["done"], false, "nothing was written");
    assert_eq!(cfg["theme"], "dark", "a refused patch writes none of its keys");
    // settings has no os.permissions: it can reopen the screen, not finish it.
    let forbidden = call(&r, "settings", "onboarding:finish", Value::Null).await;
    assert_eq!(forbidden["error"]["code"], "E_FORBIDDEN", "{forbidden}");
    let ok = call(&r, "shell", "onboarding:finish", Value::Null).await;
    assert_eq!(ok["data"], json!({"done": true}), "{ok}");
    // Persisted: a relaunch reads it back.
    drop(r);
    let (_svc2, r2, _) = app(&file, None);
    let cfg = call(&r2, "settings", "config:get", Value::Null).await["data"].clone();
    assert_eq!(cfg["onboarding"]["done"], true);
    // Other writes still work and keep it.
    let ok = call(
        &r2,
        "settings",
        "config:set",
        json!({"patch": {"theme": "light"}}),
    )
    .await;
    assert_eq!(ok["data"]["onboarding"]["done"], true, "{ok}");
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
    // ADR 0181: modules.<id>.bundledFallback is host-written (like installed): a renderer can't set or clear it.
    for v in [json!(true), json!(false)] {
        let bad = call(
            &r,
            "settings",
            "config:set",
            json!({"patch": {"modules": {"echo": {"bundledFallback": v}}}}),
        )
        .await;
        assert_eq!(bad["error"]["code"], "E_VALIDATION", "{bad}");
        assert!(bad.to_string().contains("set by qrtz"), "{bad}");
    }
    // P3.5: replacing modules, or a module's object, with a non-object (which deep_merge would apply wholesale and so
    // wipe installed and bundledFallback) is refused too, and so is a patch that isn't an object.
    let before = call(&r, "settings", "config:get", Value::Null).await["data"].clone();
    for patch in [
        json!({"modules": null}),
        json!({"modules": {"echo": null}}),
        json!({"modules": {"echo": []}}),
        json!(null),
        json!([]),
    ] {
        let bad = call(&r, "settings", "config:set", json!({ "patch": patch })).await;
        assert_eq!(bad["error"]["code"], "E_VALIDATION", "{patch}: {bad}");
    }
    let after = call(&r, "settings", "config:get", Value::Null).await["data"].clone();
    assert_eq!(after, before, "nothing changed");
    assert!(
        after["modules"]["echo"].get("bundledFallback").is_none(),
        "{after}"
    );
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
    // ADR 0146 M1.2: note.* now lives in nano's own file; a corrupt config.json resets the global settings only.
    assert!(on_disk.get("note").is_none());
    std::fs::write(&file, "{garbage").unwrap();
    let c = ConfigStore::open(&file);
    assert_eq!(c.get()["theme"], contract().config_defaults["theme"]);
    assert_eq!(c.get()["logLevel"], contract().config_defaults["logLevel"]);
    assert!(
        c.bool_at(&["note", "autoRecord"]),
        "nano's settings file is separate"
    );
    std::fs::remove_dir_all(dir.path().join("modules")).unwrap();
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

/// T2: `hello` reaches both the recording runtime and talk, and helper status reaches talk too, so a Rust talk
/// port can run the helper-lost path (Electron `TalkService.onHelperLost`). Talk messages still go only to talk.
#[test]
fn hello_and_helper_status_reach_talk() {
    use qrtz_helper_bridge::{HelperState, HelperStatus};
    use std::sync::Mutex;
    #[derive(Default)]
    struct Seen {
        lines: Mutex<Vec<String>>,
        statuses: Mutex<Vec<HelperState>>,
        configured: std::sync::atomic::AtomicUsize,
    }
    struct Talk(Arc<Seen>);
    #[async_trait::async_trait]
    impl TalkPort for Talk {
        async fn call(&self, _c: &str, _p: Value) -> Result<Value, qrtz_ipc::AppError> {
            Ok(Value::Null)
        }
        fn helper_line(&self, m: &Value) {
            self.0
                .lines
                .lock()
                .unwrap()
                .push(m["type"].as_str().unwrap().into());
        }
        fn helper_status(&self, s: &HelperStatus) {
            self.0.statuses.lock().unwrap().push(s.state);
        }
        async fn toggle(&self) -> Result<Value, qrtz_ipc::AppError> {
            Ok(Value::Null)
        }
        fn config_changed(&self) {
            self.0
                .configured
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    struct Runtime(Arc<Seen>);
    #[async_trait::async_trait]
    impl RuntimePort for Runtime {
        async fn call(&self, _c: &str, _p: Value) -> Result<Value, qrtz_ipc::AppError> {
            Ok(Value::Null)
        }
        async fn toggle(&self) -> Result<Value, qrtz_ipc::AppError> {
            Ok(Value::Null)
        }
        fn helper_line(&self, m: &Value) {
            self.0
                .lines
                .lock()
                .unwrap()
                .push(m["type"].as_str().unwrap().into());
        }
        fn helper_status(&self, s: &HelperStatus) {
            self.0.statuses.lock().unwrap().push(s.state);
        }
    }
    let (talk, rt) = (Arc::new(Seen::default()), Arc::new(Seen::default()));
    let dir = tempfile::tempdir().unwrap();
    let svc = AppService::new(AppDeps {
        info: info(),
        config: ConfigStore::open(dir.path().join("config.json")).into(),
        runtime: Arc::new(Runtime(rt.clone())),
        storage: Arc::new(StubStorage),
        talk: Arc::new(Talk(talk.clone())),
        shell: Arc::new(NoShell),
        helper: None,
        events: Events::new(),
    });
    svc.on_helper_line(
        r#"{"features":[],"helperVersion":"0.1.0","os":"macOS 15.5","protocol":1,"type":"hello"}"#,
    );
    svc.on_helper_line(r#"{"action":"down","type":"hotkey"}"#);
    // Talk's own errors and mic revocations reach talk as well; other errors stay with the runtime.
    svc.on_helper_line(
        r#"{"type":"error","code":"dictation.busy","message":"busy","fatal":false,"id":"d2","activeId":"d1"}"#,
    );
    svc.on_helper_line(r#"{"type":"error","code":"audio.unavailable","message":"none","fatal":false}"#);
    assert_eq!(*rt.lines.lock().unwrap(), ["hello", "error", "error"]);
    assert_eq!(*talk.lines.lock().unwrap(), ["hello", "hotkey", "error"]);
    let status = |state| HelperStatus {
        state,
        restarts: 1,
        last_error: Some("exited".into()),
    };
    svc.on_helper_status(&status(HelperState::Backoff));
    svc.on_helper_status(&status(HelperState::Running));
    let want = [HelperState::Backoff, HelperState::Running];
    assert_eq!(*talk.statuses.lock().unwrap(), want);
    assert_eq!(*rt.statuses.lock().unwrap(), want);
    // config:set tells talk to re-configure.
    let rt2 = tokio::runtime::Runtime::new().unwrap();
    rt2.block_on(async {
        use qrtz_ipc::{CallContext, Handler};
        let ctx = CallContext {
            sub_app: "settings".into(),
            request_id: "r1".into(),
        };
        let patch = json!({"patch": {"talk": {"warmMic": true}}});
        svc.handle("config:set", patch, &ctx).await.unwrap();
    });
    assert_eq!(talk.configured.load(std::sync::atomic::Ordering::SeqCst), 1);
}
