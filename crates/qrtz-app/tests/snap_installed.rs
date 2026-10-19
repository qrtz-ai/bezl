//! Design call (e) of Bozniak's 902bcab review: qrtz-snap follows `modules.snap.installed` (ADR 0169) through the
//! same switch pass as nano, echo and ctrl (`apply_module_switches`). A snap that isn't installed is never spawned,
//! so its supervisor never retries: not at launch, not on `config:set`, not when its settings change. Installing it
//! (the lock sync after `packages:apply`) starts it; uninstalling stops it.
use qrtz_app::ports::*;
use qrtz_app::*;
use qrtz_helper_bridge::{ChildIo, HelperState, Spawn, Supervisor, SupervisorOptions};
use qrtz_ipc::Router;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct NoHud;
impl ShellHooks for NoHud {
    fn set_hud_expanded(&self, e: bool) -> bool {
        e
    }
}

/// A qrtz-snap launch that can't work (counted).
struct Counting(Arc<AtomicUsize>);
impl Spawn for Counting {
    fn spawn(&self) -> std::io::Result<ChildIo> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other(
            "snap is not installed (install it from the marketplace)",
        ))
    }
}

async fn call(r: &Router<Shared>, ch: &str, payload: Value) -> Value {
    r.dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":"settings","payload":payload}))
        .await
}

#[tokio::test]
async fn snap_follows_installed_and_is_never_retried_while_not_installed() {
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
    let installed = Arc::new(AtomicBool::new(false));
    let i2 = installed.clone();
    svc.track_installed_modules(qrtz_app::service::ModuleSync {
        root: dir.path().to_path_buf(),
        read: Arc::new(move || qrtz_app::installed::InstalledModules {
            patch: json!({"modules": {"snap": {"installed": i2.load(Ordering::SeqCst)}}}),
            versions: Default::default(),
        }),
    });
    let tries = Arc::new(AtomicUsize::new(0));
    let sup = Supervisor::spawn(
        SupervisorOptions {
            name: "qrtz-snap".into(),
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
            heartbeat_timeout: Duration::ZERO,
            ..Default::default()
        },
        Arc::new(Counting(tries.clone())),
    );
    let host = Arc::new(qrtz_app::snap::SnapHost::new(sup.clone()));
    svc.set_snap(host);
    svc.start_helper();
    let r = Router::new(Shared(svc.clone()));
    for patch in [
        json!({"modules": {"snap": {"enabled": true}}}),
        json!({"snap": {"statusItem": false}}),
        json!({"modules": {"enabled": true}}),
        json!({"theme": "dark"}),
    ] {
        let res = call(&r, "config:set", json!({ "patch": patch })).await;
        assert_eq!(res["ok"], true, "{patch}: {res}");
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        tries.load(Ordering::SeqCst),
        0,
        "never spawned while not installed"
    );
    assert_eq!(sup.status().state, HelperState::Stopped);

    // Installed (the lock sync after packages:apply): the same pass starts it.
    installed.store(true, Ordering::SeqCst);
    svc.sync_installed();
    let mut st = sup.watch_status();
    tokio::time::timeout(
        Duration::from_secs(5),
        st.wait_for(|s| s.state == HelperState::Failed),
    )
    .await
    .expect("started, then gave up on the broken launch")
    .unwrap();
    assert!(tries.load(Ordering::SeqCst) > 0);

    // Uninstalled: stopped, and nothing (a settings change) retries it.
    installed.store(false, Ordering::SeqCst);
    svc.sync_installed();
    tokio::time::timeout(
        Duration::from_secs(5),
        st.wait_for(|s| s.state == HelperState::Stopped),
    )
    .await
    .expect("stopped")
    .unwrap();
    let n = tries.load(Ordering::SeqCst);
    let res = call(&r, "config:set", json!({"patch": {"snap": {"statusItem": true}}})).await;
    assert_eq!(res["ok"], true, "{res}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(tries.load(Ordering::SeqCst), n);
    assert_eq!(sup.status().state, HelperState::Stopped);
}
