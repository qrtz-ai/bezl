//! ADR 0172 (v1): a marketplace update of an installed module keeps `installed` true and doesn't touch its running
//! helper: the module is `pendingRestart` until qrtz restarts, and the old version's package dir survives every
//! prune while that helper lives. The next launch runs the new version and prunes the old dir.
#![cfg(feature = "runtime")]
use qrtz_app::module_helpers::{HelperLayout, INSTALLED_PROGRAM};
use qrtz_app::module_hosting::{HttpTransport, Packages};
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::runtime_port::HelperAvailability;
use qrtz_app::storage_port::NoStorageShell;
use qrtz_app::wiring::{build_app, module_sync, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::{ProcessSpawn, SupervisorOptions};
use qrtz_ipc::Router;
use qrtz_modules::install::{Installer, IN_USE_FILE};
use qrtz_publish::dry_run::NoAppleChecks;
use qrtz_publish::keys::Keys;
use qrtz_publish::{build, write_root, Spec, SpecPackage};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
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

fn write(p: &Path, b: &[u8], exec: bool) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
    if exec {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// nano `version`: a helper that logs its start and exit to `log` and runs until stdin closes or `shutdown`.
fn nano(src: &Path, version: &str, log: &Path) -> PathBuf {
    let d = src.join(format!("nano-{version}"));
    write(
        &d.join("qrtz.json"),
        serde_json::to_vec(&json!({"name": "nano", "version": version, "kind": "extension",
            "dependencies": {},
            "extension": {"entry": "bin/qrtz-nano", "protocol": 1, "minApp": "0.2.0",
                          "views": ["note"], "permissions": ["microphone"]}}))
        .unwrap()
        .as_slice(),
        false,
    );
    let log = log.display();
    write(
        &d.join("bin/qrtz-nano"),
        format!(
            "#!/bin/sh\necho \"start $$ {version}\" >> '{log}'\necho '{{\"type\":\"heartbeat\",\"ts\":0}}'\n\
             while read -r l; do case \"$l\" in *shutdown*) break;; esac; done\necho \"exit $$ {version}\" >> '{log}'\n"
        )
        .as_bytes(),
        true,
    );
    d
}

fn publish(out: &Path, keys: &Keys, dir: PathBuf) {
    let spec = Spec {
        team_id: "ABCDE12345".into(),
        packages: vec![SpecPackage::fake_notarized(dir)],
        pinned_models: vec![],
        models: vec![],
        revoked: vec![],
    };
    build(out, &spec, keys, qrtz_publish::now_ms()).unwrap();
}

fn app(data: &Path, out: &Path, pin: &str) -> BuiltApp {
    std::fs::create_dir_all(data).unwrap();
    // QRT-352: nano is experimental; its helper runs only with experimental apps on.
    if !data.join("config.json").exists() {
        std::fs::write(data.join("config.json"), r#"{"app":{"experimentalApps":true}}"#).unwrap();
    }
    let built = build_app(AppBuild {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.2.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "tauri test".into(),
        },
        data_dir: data.to_path_buf(),
        helper: Some((
            ProcessSpawn::helper(INSTALLED_PROGRAM.into()),
            HelperAvailability::Native,
        )),
        supervisor: SupervisorOptions {
            heartbeat_timeout: Duration::ZERO,
            ..Default::default()
        },
        helper_layout: HelperLayout::Installed {
            root: data.to_path_buf(),
            programs: Default::default(),
            bundled: Default::default(),
        },
        modules_from_lock: false,
        packages: Some(Packages::with(
            Installer::new(data, Some(pin.to_owned()), Box::new(NoAppleChecks)),
            HttpTransport::local_dir(out),
        )),
        shell: Arc::new(Shell),
        storage_shell: Arc::new(NoStorageShell),
        speech: None,
        talk: None,
        steps: None,
        knobs: DevKnobs::default(),
    })
    .unwrap();
    built
        .service
        .track_installed_modules(module_sync(data.to_path_buf(), None));
    built.service.start_helper();
    built
}

async fn call(r: &Router<Shared>, ch: &str, payload: Value) -> Value {
    let v = r
        .dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":"marketplace","payload":payload}))
        .await;
    assert_eq!(v["ok"], true, "{ch}: {v}");
    v["data"].clone()
}

async fn change(r: &Router<Shared>, op: Value) {
    let plan = call(r, "packages:plan", op).await;
    call(
        r,
        "packages:apply",
        json!({"planSha256": plan["planSha256"], "downloadBytes": plan["downloadBytes"]}),
    )
    .await;
}

fn lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out: {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_waits_for_a_restart_and_the_old_dir_outlives_every_prune_until_then() {
    let tmp = tempfile::tempdir().unwrap();
    let (src, out, data) = (
        tmp.path().join("src"),
        tmp.path().join("out"),
        tmp.path().join("data"),
    );
    let log = tmp.path().join("spawns.log");
    let keys = Keys::ephemeral();
    let (_, pin) = write_root(&out, &keys, None, 1, qrtz_publish::now_ms()).unwrap();
    publish(&out, &keys, nano(&src, "0.3.0", &log));

    let b = app(&data, &out, &pin);
    let r = Router::new(Shared(b.service.clone())).validate_responses(true);
    call(&r, "packages:refresh", Value::Null).await;
    change(&r, json!({"op": "install", "names": ["nano"]})).await;
    until("nano 0.3.0 runs", || {
        lines(&log).iter().any(|l| l.ends_with(" 0.3.0"))
    })
    .await;
    let first = lines(&log);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(b.service.pending_restart().is_empty());
    assert_eq!(
        call(&r, "packages:list", Value::Null).await["pendingRestart"],
        json!([])
    );

    // nano 0.3.1 is published and the user updates: installed stays true, the running helper is left alone, and
    // the module says it needs a restart.
    publish(&out, &keys, nano(&src, "0.3.1", &log));
    call(&r, "packages:refresh", Value::Null).await;
    change(&r, json!({"op": "update", "names": ["nano"], "latest": true})).await;
    assert_eq!(b.service.config()["modules"]["nano"]["installed"], true);
    assert_eq!(
        qrtz_app::installed::entry(&data, "nano").unwrap().version,
        "0.3.1"
    );
    assert_eq!(b.service.pending_restart(), ["nano"]);
    let st = call(&r, "packages:list", Value::Null).await;
    assert_eq!(st["pendingRestart"], json!(["nano"]), "{st}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(lines(&log), first, "no relaunch: the old process keeps running");

    // The apply's prune, an explicit "clean up" and the installer itself (the CLI's prune) all keep the old dir.
    let old = data.join("packages/nano/0.3.0");
    assert!(old.join("bin/qrtz-nano").is_file(), "the apply's prune kept it");
    assert!(data.join(IN_USE_FILE).is_file());
    call(&r, "packages:prune", Value::Null).await;
    assert!(old.is_dir(), "clean up keeps it");
    Installer::new(&data, Some(pin.clone()), Box::new(NoAppleChecks))
        .prune()
        .unwrap();
    assert!(old.is_dir(), "every prune keeps it");
    let st = call(&r, "packages:list", Value::Null).await;
    assert_eq!(st["reclaimable"]["dirs"], 0, "{st}");

    // Restart qrtz: the old helper exits, the next launch runs 0.3.1, nothing is pending, the old dir goes.
    b.service.shutdown_helpers(Duration::from_secs(2)).await;
    until("nano 0.3.0 exits", || {
        lines(&log)
            .iter()
            .any(|l| l.starts_with("exit ") && l.ends_with(" 0.3.0"))
    })
    .await;
    drop(r);
    drop(b);
    let b = app(&data, &out, &pin);
    until("nano 0.3.1 runs", || {
        lines(&log)
            .iter()
            .any(|l| l.starts_with("start ") && l.ends_with(" 0.3.1"))
    })
    .await;
    assert!(b.service.pending_restart().is_empty());
    until("the old dir is pruned at launch", || !old.exists()).await;
    assert!(data.join("packages/nano/0.3.1").is_dir());
    assert!(!data.join(IN_USE_FILE).exists());
    b.service.shutdown_helpers(Duration::from_secs(2)).await;
}
