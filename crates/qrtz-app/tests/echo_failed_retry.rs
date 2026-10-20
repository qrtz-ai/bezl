//! QRT-291: when qrtz-echo's launch is refused until the supervisor gives up (Failed), `helper:status` says why
//! (the check and reason, no path), and the user's echo switch going on gives it exactly one fresh set of tries.
//! Switching it off never starts it, and an unrelated `config:set` never retries it.
use qrtz_app::ports::*;
use qrtz_app::*;
use qrtz_helper_bridge::{ChildIo, HelperState, Spawn, Supervisor, SupervisorOptions};
use qrtz_ipc::Router;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct NoHud;
impl ShellHooks for NoHud {
    fn set_hud_expanded(&self, e: bool) -> bool {
        e
    }
}

/// A launch refused by the signature check, as on 0.2.4 (counted).
struct Refused(Arc<AtomicUsize>);
impl Spawn for Refused {
    fn spawn(&self) -> std::io::Result<ChildIo> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other(
            "codesign: /Applications/qrtz.app/Contents/MacOS/qrtz-echo: test-requirement: code failed to satisfy specified code requirement(s)",
        ))
    }
}

/// Never started in this test.
struct Idle;
impl Spawn for Idle {
    fn spawn(&self) -> std::io::Result<ChildIo> {
        Err(std::io::Error::other("unused"))
    }
}

async fn call(r: &Router<Shared>, ch: &str, payload: Value) -> Value {
    r.dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":"settings","payload":payload}))
        .await
}

fn opts(name: &str) -> SupervisorOptions {
    SupervisorOptions {
        name: name.into(),
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(2),
        heartbeat_timeout: Duration::ZERO,
        ..Default::default()
    }
}

async fn settle(sup: &Supervisor, tries: &AtomicUsize) -> usize {
    let mut st = sup.watch_status();
    tokio::time::timeout(
        Duration::from_secs(5),
        st.wait_for(|s| s.state == HelperState::Failed),
    )
    .await
    .expect("gave up")
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    tries.load(Ordering::SeqCst)
}

#[tokio::test]
async fn a_refused_echo_says_why_and_a_toggle_retries_it_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let svc = AppService::new(AppDeps {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "test".into(),
        },
        config: ConfigStore::open(dir.path().join("config.json")).into(),
        runtime: Arc::new(StubRuntime::default()),
        storage: Arc::new(StubStorage),
        talk: Arc::new(StubTalk),
        shell: Arc::new(NoHud),
        helper: None,
        events: Events::new(),
    });
    svc.track_installed_modules(qrtz_app::service::ModuleSync {
        root: dir.path().to_path_buf(),
        read: Arc::new(|| qrtz_app::installed::InstalledModules {
            patch: json!({"modules": {"echo": {"installed": true}, "nano": {"installed": false}}}),
            versions: Default::default(),
        }),
    });
    let tries = Arc::new(AtomicUsize::new(0));
    let echo = Supervisor::spawn(
        opts("qrtz-echo"),
        Arc::new(qrtz_app::installed::PublicRefusal(
            Refused(tries.clone()),
            "echo".into(),
        )),
    );
    let idle = || Supervisor::spawn(opts("idle"), Arc::new(Idle));
    svc.set_module_helpers(qrtz_app::module_helpers::ModuleHelpers::split(
        idle(),
        echo.clone(),
        idle(),
    ));
    let r = Router::new(Shared(svc.clone()));
    let res = call(
        &r,
        "config:set",
        json!({"patch": {"modules": {"echo": {"enabled": true}}}}),
    )
    .await;
    assert_eq!(res["ok"], true, "{res}");
    svc.start_helper();

    // Five refusals in the window, then Failed, with the reason shown and no path or codesign output.
    let first = settle(&echo, &tries).await;
    assert_eq!(first, 6, "the first launch and five retries");
    let st = echo.status();
    assert_eq!(st.state, HelperState::Failed);
    let shown = st.last_error.unwrap();
    assert!(shown.starts_with("echo: signature mismatch"), "{shown}");
    assert!(
        !shown.contains('/') && !shown.contains("test-requirement"),
        "{shown}"
    );
    let status = r
        .dispatch(json!({"v":1,"id":"t","channel":"helper:status","version":1,"subApp":"settings"}))
        .await;
    assert_eq!(status["data"]["lastError"], json!(shown), "{status}");

    // An unrelated config:set never retries it.
    for patch in [json!({"theme": "dark"}), json!({"talk": {"showPill": false}})] {
        let res = call(&r, "config:set", json!({ "patch": patch })).await;
        assert_eq!(res["ok"], true, "{patch}: {res}");
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(tries.load(Ordering::SeqCst), first);
    assert_eq!(echo.status().state, HelperState::Failed);

    // Switching echo off never starts it; it stays stopped.
    let res = call(
        &r,
        "config:set",
        json!({"patch": {"modules": {"echo": {"enabled": false}}}}),
    )
    .await;
    assert_eq!(res["ok"], true, "{res}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(tries.load(Ordering::SeqCst), first, "off never starts it");
    assert_ne!(echo.status().state, HelperState::Running);

    // Switching it on: exactly one fresh set of tries, then Failed again and stopped until the next toggle.
    let res = call(
        &r,
        "config:set",
        json!({"patch": {"modules": {"echo": {"enabled": true}}}}),
    )
    .await;
    assert_eq!(res["ok"], true, "{res}");
    let second = settle(&echo, &tries).await;
    assert_eq!(second, 2 * first, "one toggle, one fresh set");
    let res = call(
        &r,
        "config:set",
        json!({"patch": {"modules": {"echo": {"enabled": true}}}}),
    )
    .await;
    assert_eq!(res["ok"], true, "{res}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(tries.load(Ordering::SeqCst), second, "no toggle, no retry");
}
