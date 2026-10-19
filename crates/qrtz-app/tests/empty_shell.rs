//! ADR 0169 / ADR 0170 end to end on a local signed index (the dev path `bun run modules:dev` uses): qrtz starts
//! empty; installing a module from the marketplace brings its model in the same plan (one consent, one progress,
//! all or nothing) and links the helper's model view; the module's helper entry resolves from its installed
//! package; a shared model stays while another installed module uses it; uninstalling the last user removes the
//! model, its view and its blobs. A package whose notarization record isn't its code, or whose model can't be
//! fetched, installs nothing.
#![cfg(feature = "runtime")]
use qrtz_app::local_models::{LocalModels, Manifest};
use qrtz_app::module_hosting::{HttpTransport, Packages};
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::storage_port::NoStorageShell;
use qrtz_app::wiring::{build_app, module_sync, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::SupervisorOptions;
use qrtz_ipc::Router;
use qrtz_modules::install::Installer;
use qrtz_publish::dry_run::NoAppleChecks;
use qrtz_publish::keys::Keys;
use qrtz_publish::{build, write_root, Spec, SpecPackage};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Shell;
impl ShellHooks for Shell {
    fn set_hud_expanded(&self, e: bool) -> bool {
        e
    }
    fn notify(&self, _n: Notice) {}
}

const WEIGHTS: &[u8] = b"sample weights, shared by echo and nano";
const NANO_ONLY: &[u8] = b"nano's own model";

fn write(p: &Path, b: &[u8], exec: bool) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
    if exec {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn sha(b: &[u8]) -> String {
    qrtz_modules::sha256_hex(b)
}

const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

fn model(dir: &Path, name: &str, file: &[u8]) -> PathBuf {
    let d = dir.join(name);
    write(
        &d.join("qrtz.json"),
        serde_json::to_vec(&json!({"name": name, "version": "1.0.0", "kind": "model",
            "model": {"task": "asr", "upstream": format!("qrtz-test/{name}"), "revision": REVISION}}))
        .unwrap()
        .as_slice(),
        false,
    );
    write(&d.join("weights/a.bin"), file, false);
    // ADR 0171: what Hugging Face serves at the pinned commit (the app's transport reads it from here).
    write(&hf_file(dir.parent().unwrap(), name), file, false);
    d
}

/// The fixture's "Hugging Face": `<pub>/hf/<owner>/<repo>/resolve/<rev>/<path>`.
fn hf_file(pub_dir: &Path, name: &str) -> PathBuf {
    pub_dir.join(format!("hf/qrtz-test/{name}/resolve/{REVISION}/weights/a.bin"))
}

fn extension(dir: &Path, name: &str, version: &str, deps: Value, views: &[&str]) -> PathBuf {
    let d = dir.join(format!("{name}-{version}"));
    write(
        &d.join("qrtz.json"),
        serde_json::to_vec(&json!({"name": name, "version": version, "kind": "extension",
            "dependencies": deps,
            "extension": {"entry": format!("bin/qrtz-{name}"), "protocol": 1, "minApp": "0.2.0",
                          "views": views, "permissions": ["microphone"]}}))
        .unwrap()
        .as_slice(),
        false,
    );
    write(
        &d.join(format!("bin/qrtz-{name}")),
        format!("#!/bin/sh\necho {name}\n").as_bytes(),
        true,
    );
    d
}

/// The published fixture: echo and nano share `model.sample`; nano also needs `model.nanoonly`; `snap 0.9.0`'s
/// notarization record names a different executable of its package than its entry.
fn publish(dir: &Path) -> (PathBuf, String) {
    let src = dir.join("src");
    let out = dir.join("out");
    let shared = model(&src, "model.sample", WEIGHTS);
    let nano_only = model(&src, "model.nanoonly", NANO_ONLY);
    // The models are handed to a service (ADR 0157 §7), like inference.voice for the real echo and nano.
    let svc = src.join("inference.sample");
    write(
        &svc.join("qrtz.json"),
        serde_json::to_vec(
            &json!({"name": "inference.sample", "version": "1.0.0", "kind": "service",
            "service": {"bundle": "sample.xpc", "models": ["model.sample", "model.nanoonly"]}}),
        )
        .unwrap()
        .as_slice(),
        false,
    );
    write(
        &svc.join("sample.xpc/Contents/MacOS/sample"),
        b"#!/bin/sh\n",
        true,
    );
    write(&svc.join("sample.xpc/Contents/Info.plist"), b"<plist/>", false);
    let echo = extension(
        &src,
        "echo",
        "0.3.0",
        json!({"inference.sample": "^1.0.0", "model.sample": "^1.0.0"}),
        &["talk"],
    );
    let nano = extension(
        &src,
        "nano",
        "0.3.0",
        json!({"inference.sample": "^1.0.0", "model.sample": "^1.0.0", "model.nanoonly": "^1.0.0"}),
        &["note"],
    );
    let snap = extension(&src, "snap", "0.9.0", json!({}), &["snap"]);
    write(&snap.join("bin/other"), b"#!/bin/sh\necho other\n", true);
    let mut bad = SpecPackage::fake_notarized(snap.clone());
    let n = bad.notarization.as_mut().unwrap();
    n.code_sha256 = sha(b"#!/bin/sh\necho other\n");
    let keys = Keys::ephemeral();
    let now = qrtz_publish::now_ms();
    let (_, pin) = write_root(&out, &keys, None, 1, now).unwrap();
    let spec = Spec {
        team_id: "ABCDE12345".into(),
        packages: vec![
            SpecPackage::new(shared),
            SpecPackage::new(nano_only),
            SpecPackage::fake_notarized(svc),
            SpecPackage::fake_notarized(echo),
            SpecPackage::fake_notarized(nano),
            bad,
        ],
        pinned_models: vec![],
        models: vec![],
        revoked: vec![],
    };
    build(&out, &spec, &keys, now).unwrap();
    (out, pin)
}

/// The helper's view of `model.sample` (what local_models.json is for the real sets).
fn views(data: &Path) -> LocalModels {
    let m: Manifest = serde_json::from_value(json!({
        "version": "1.0.0",
        "base": "https://huggingface.co",
        "sets": [{
            "id": "sample", "package": "model.sample", "task": "asr", "dir": "sample",
            "repo": "qrtz/sample", "revision": REVISION,
            "files": [{"remote": "weights/a.bin", "path": "weights/a.bin", "size": WEIGHTS.len(), "sha256": sha(WEIGHTS)}]
        }]
    }))
    .unwrap();
    LocalModels::with_manifest(data.join("models"), m, None)
}

fn app(data: &Path, out: &Path, pin: &str) -> BuiltApp {
    std::fs::create_dir_all(data).unwrap();
    let built = build_app(AppBuild {
        info: AppInfo {
            name: "qrtz".into(),
            version: "0.2.0".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            shell: "tauri test".into(),
        },
        data_dir: data.to_path_buf(),
        helper: None,
        supervisor: SupervisorOptions::default(),
        helper_layout: Default::default(),
        // Tracked below with the fixture's model view; the app passes `true` and the pinned sets.
        modules_from_lock: false,
        packages: Some(Packages::with(
            Installer::new(data, Some(pin.to_owned()), Box::new(NoAppleChecks)),
            HttpTransport::local_dir(out)
                .with_url_dir("https://huggingface.co/", out.parent().unwrap().join("hf")),
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
        .track_installed_modules(module_sync(data.to_path_buf(), Some(views(data))));
    built
}

async fn call(r: &Router<Shared>, sub: &str, ch: &str, payload: Value) -> Value {
    r.dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":sub,"payload":payload}))
        .await
}

async fn ok(r: &Router<Shared>, sub: &str, ch: &str, payload: Value) -> Value {
    let v = call(r, sub, ch, payload).await;
    assert_eq!(v["ok"], true, "{ch}: {v}");
    v["data"].clone()
}

/// plan → (the consent the marketplace shows) → apply with that plan's hash and size.
async fn change(r: &Router<Shared>, op: &str, names: &[&str]) -> Result<Value, Value> {
    let plan = ok(
        r,
        "marketplace",
        "packages:plan",
        json!({"op": op, "names": names}),
    )
    .await;
    let v = call(
        r,
        "marketplace",
        "packages:apply",
        json!({"planSha256": plan["planSha256"], "downloadBytes": plan["downloadBytes"]}),
    )
    .await;
    if v["ok"] == true {
        Ok(plan)
    } else {
        Err(v)
    }
}

fn installed(b: &BuiltApp) -> Vec<String> {
    let cfg = b.service.config();
    qrtz_modules::MODULE_IDS
        .iter()
        .filter(|id| cfg["modules"][**id]["installed"] == true)
        .map(|s| (*s).to_owned())
        .collect()
}

fn names(plan: &Value) -> Vec<String> {
    let mut v: Vec<String> = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str().map(str::to_owned))
        .collect();
    v.sort();
    v
}

fn package_dirs(data: &Path, name: &str) -> usize {
    std::fs::read_dir(data.join("packages").join(name))
        .map(|d| d.count())
        .unwrap_or(0)
}

#[tokio::test]
async fn install_brings_the_models_and_uninstall_takes_what_nothing_else_needs() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, pin) = publish(&tmp.path().join("pub"));
    let data = tmp.path().join("data");
    let b = app(&data, &out, &pin);
    let r = Router::new(Shared(b.service.clone())).validate_responses(true);

    // Empty: nothing installed, a module's view is refused, it can't be switched on, and no renderer can
    // pretend it is installed.
    assert!(installed(&b).is_empty(), "{}", b.service.config()["modules"]);
    let v = call(&r, "talk", "module:get-settings", Value::Null).await;
    assert_eq!(v["error"]["code"], "E_UNAVAILABLE", "{v}");
    let v = call(
        &r,
        "marketplace",
        "packages:set-enabled",
        json!({"name": "echo", "enabled": true}),
    )
    .await;
    assert_eq!(v["ok"], false, "{v}");
    let v = call(
        &r,
        "settings",
        "config:set",
        json!({"patch": {"modules": {"echo": {"installed": true}}}}),
    )
    .await;
    assert_eq!(v["ok"], false, "{v}");
    assert!(qrtz_app::installed::entry(&data, "echo")
        .unwrap_err()
        .contains("not installed"));

    ok(&r, "marketplace", "packages:refresh", Value::Null).await;

    // Install echo: one plan carries echo and its model; afterwards echo is installed, its view works, its helper
    // entry is its package's executable and the helper's model view is linked and marked.
    let plan = change(&r, "install", &["echo"]).await.unwrap();
    assert_eq!(
        names(&plan),
        ["echo", "inference.sample", "model.sample"],
        "{plan}"
    );
    assert_eq!(installed(&b), ["echo"]);
    ok(&r, "talk", "module:get-settings", Value::Null).await;
    let e = qrtz_app::installed::entry(&data, "echo").unwrap();
    assert_eq!(e.program, data.join("packages/echo/0.3.0/bin/qrtz-echo"));
    qrtz_app::installed::check_launch(&e, qrtz_app::installed::LaunchCheck::Dev).unwrap();
    let spawn = qrtz_app::module_helpers::HelperLayout::Installed {
        root: data.clone(),
        programs: Default::default(),
    };
    let base = qrtz_helper_bridge::ProcessSpawn::helper("qrtz-installed-module".into());
    let _ = spawn.spawner(&base, "echo");
    let view = data.join("models/sample");
    assert_eq!(std::fs::read(view.join("weights/a.bin")).unwrap(), WEIGHTS);
    assert!(view.join(qrtz_app::local_models::MARKER).is_file());

    // nano shares model.sample: its plan only adds its own model.
    let plan = change(&r, "install", &["nano"]).await.unwrap();
    assert_eq!(names(&plan), ["model.nanoonly", "nano"], "{plan}");
    assert_eq!(installed(&b), ["echo", "nano"]);

    // Removing echo keeps model.sample (nano needs it), its view and its package dir.
    change(&r, "remove", &["echo"]).await.unwrap();
    assert_eq!(installed(&b), ["nano"]);
    assert_eq!(package_dirs(&data, "echo"), 0, "echo pruned");
    assert_eq!(package_dirs(&data, "model.sample"), 1, "still used by nano");
    assert!(view.join("weights/a.bin").is_file());

    // Removing nano, the last user: both models, the service, the view and their blobs go.
    change(&r, "remove", &["nano"]).await.unwrap();
    assert!(installed(&b).is_empty());
    for p in ["nano", "inference.sample", "model.sample", "model.nanoonly"] {
        assert_eq!(package_dirs(&data, p), 0, "{p} pruned");
    }
    assert!(!view.exists(), "the model view went with its last module");
    let lock = qrtz_app::installed::read_lock(&data);
    assert!(lock.packages.is_empty(), "{lock:?}");
}

#[tokio::test]
async fn a_failed_model_or_a_wrong_notarization_installs_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, pin) = publish(&tmp.path().join("pub"));
    let data = tmp.path().join("data");
    let b = app(&data, &out, &pin);
    let r = Router::new(Shared(b.service.clone())).validate_responses(true);
    ok(&r, "marketplace", "packages:refresh", Value::Null).await;

    // snap 0.9.0's notarization record names bin/other, not its entry: refused at staging, nothing changes.
    let e = change(&r, "install", &["snap"]).await.unwrap_err();
    assert!(e.to_string().contains("notarized file"), "{e}");
    assert!(installed(&b).is_empty());
    assert!(qrtz_app::installed::read_lock(&data).packages.is_empty());

    // nano's own model is gone from Hugging Face: the install fails as a whole (nano isn't half installed, and the
    // shared model it fetched isn't left in the lock); the same plan succeeds once the blob is back (retry).
    let blob = hf_file(out.parent().unwrap(), "model.nanoonly");
    // ADR 0171: model bytes were never on the package host.
    assert!(!out
        .join(qrtz_modules::index::layout::blob(&sha(NANO_ONLY)))
        .exists());
    let saved = std::fs::read(&blob).unwrap();
    std::fs::remove_file(&blob).unwrap();
    change(&r, "install", &["nano"]).await.unwrap_err();
    assert!(installed(&b).is_empty());
    assert!(qrtz_app::installed::read_lock(&data).packages.is_empty());
    assert!(!data.join("models/sample").exists());
    std::fs::write(&blob, saved).unwrap();
    change(&r, "install", &["nano"]).await.unwrap();
    assert_eq!(installed(&b), ["nano"]);
    assert!(data.join("models/sample/weights/a.bin").is_file());
}
