//! Run 41: ADR 0169 / ADR 0171 for ctrl, end to end on a local signed index. Installing ctrl from the marketplace
//! brings `model.laya` in the same plan (one consent, one progress), its files come only from their pinned Hugging
//! Face URL (never from the package host), and with no answer recorded in ctrl's settings the installed model is
//! ctrl's yes: `CtrlLayaModel::consent()` hands the helper the read-only, store-linked `.mlmodelc` inside
//! `packages/model.laya/<version>/` (ADR 0157 §9), with no separate download and no helper view under `models/`.
//! A recorded "no" still wins; removing ctrl prunes the model and the scorer falls back to the heuristic.
#![cfg(feature = "runtime")]
use qrtz_app::ctrl_model::{CtrlLayaModel, CtrlManifest};
use qrtz_app::local_models::LocalModels;
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

const REVISION: &str = "7b8d7a2b7e28e746c6ecaad44bbcd5cf251a4fcc";
const ENTRY: &str = "laya_multilingual_e8_L512_options32.mlmodelc";
/// The model package's files (beside its `qrtz.json`): a compiled model and its tokenizer, like the pinned set.
const FILES: &[(&str, &[u8])] = &[
    (
        "laya_multilingual_e8_L512_options32.mlmodelc/coremldata.bin",
        b"coreml data",
    ),
    (
        "laya_multilingual_e8_L512_options32.mlmodelc/model.mil",
        b"program(1.0)",
    ),
    (
        "laya_multilingual_e8_L512_options32.mlmodelc/weights/weight.bin",
        b"laya weights, int8",
    ),
    ("tokenizer.json", b"{\"model\":{}}"),
];

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

/// What Hugging Face serves at the pinned commit: `<pub>/hf/<repo>/resolve/<rev>/<path>`.
fn hf_file(pub_dir: &Path, path: &str) -> PathBuf {
    pub_dir.join(format!("hf/FluidInference/laya-coreml/resolve/{REVISION}/{path}"))
}

/// The published fixture: `model.laya` (its files on "Hugging Face" only) and `ctrl`, which depends on it and reads
/// it in its own process.
fn publish(dir: &Path) -> (PathBuf, String) {
    let src = dir.join("src");
    let out = dir.join("out");
    let laya = src.join("model.laya");
    write(
        &laya.join("qrtz.json"),
        serde_json::to_vec(&json!({"name": "model.laya", "version": "1.0.0", "kind": "model",
            "model": {"task": "decision", "upstream": "FluidInference/laya-coreml", "revision": REVISION}}))
        .unwrap()
        .as_slice(),
        false,
    );
    for (p, b) in FILES {
        write(&laya.join(p), b, false);
        write(&hf_file(dir, p), b, false);
    }
    let ctrl = src.join("ctrl-0.3.0");
    write(
        &ctrl.join("qrtz.json"),
        serde_json::to_vec(&json!({"name": "ctrl", "version": "0.3.0", "kind": "extension",
            "dependencies": {"model.laya": "^1.0.0"},
            "extension": {"entry": "bin/qrtz-ctrl", "protocol": 1, "minApp": "0.2.0", "channels": ["ctrl.*"],
                          "views": ["ctrl"], "capabilities": ["ctrl.read", "ctrl.write", "module.settings"],
                          "permissions": ["accessibility"],
                          "settings": {"namespace": "ctrl", "legacySections": []},
                          "reads": ["model.laya"]}}))
        .unwrap()
        .as_slice(),
        false,
    );
    write(&ctrl.join("bin/qrtz-ctrl"), b"#!/bin/sh\necho ctrl\n", true);
    let keys = Keys::ephemeral();
    let now = qrtz_publish::now_ms();
    let (_, pin) = write_root(&out, &keys, None, 1, now).unwrap();
    let spec = Spec {
        team_id: "ABCDE12345".into(),
        packages: vec![SpecPackage::new(laya), SpecPackage::fake_notarized(ctrl)],
        pinned_models: vec![],
        models: vec![],
        revoked: vec![],
    };
    build(&out, &spec, &keys, now).unwrap();
    (out, pin)
}

/// ctrl's pin file for the fixture (what `ctrl_laya_model.json` is for the real set).
fn laya_manifest() -> CtrlManifest {
    serde_json::from_value(json!({
        "version": "laya-test-e8-L512",
        "placeholder": false,
        "base": "https://huggingface.co",
        "sets": [{
            "id": "laya", "package": "model.laya", "task": "decision", "packageVersion": "1.0.0", "dir": "laya",
            "repo": "FluidInference/laya-coreml", "revision": REVISION,
            "files": FILES.iter().map(|(p, b)| json!({"remote": p, "path": p, "size": b.len(), "sha256": sha(b)}))
                .collect::<Vec<_>>()
        }]
    }))
    .unwrap()
}

fn laya(data: &Path) -> CtrlLayaModel {
    CtrlLayaModel::with_manifest(
        data.join("models/ctrl"),
        &data.join("ctrl"),
        laya_manifest(),
        None,
    )
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
    // The host's view sync, with ctrl's set in it: an extension-read set must get no view.
    let views = LocalModels::with_manifest(data.join("models"), laya_manifest().manifest, None)
        .with_packages_root(data);
    built
        .service
        .track_installed_modules(module_sync(data.to_path_buf(), Some(views)));
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

fn ctrl_installed(b: &BuiltApp) -> bool {
    b.service.config()["modules"]["ctrl"]["installed"] == true
}

#[tokio::test]
async fn installing_ctrl_brings_laya_from_hugging_face_and_that_is_its_yes() {
    let tmp = tempfile::tempdir().unwrap();
    let pub_dir = tmp.path().join("pub");
    let (out, pin) = publish(&pub_dir);
    let data = tmp.path().join("data");
    let b = app(&data, &out, &pin);
    let r = Router::new(Shared(b.service.clone())).validate_responses(true);
    let m = laya(&data);

    // Before: not installed, no answer, the heuristic answers.
    assert!(!ctrl_installed(&b));
    assert!(!m.consented());
    assert_eq!(m.consent().installed, None);

    // ADR 0171: no model byte is on the package host.
    for (_, bytes) in FILES {
        assert!(!out.join(qrtz_modules::index::layout::blob(&sha(bytes))).exists());
    }

    ok(&r, "marketplace", "packages:refresh", Value::Null).await;
    let plan = change(&r, "install", &["ctrl"]).await.unwrap();
    assert_eq!(
        names(&plan),
        ["ctrl", "model.laya"],
        "one plan, one consent: {plan}"
    );
    let model_bytes: usize = FILES.iter().map(|(_, b)| b.len()).sum();
    assert!(
        plan["downloadBytes"].as_u64().unwrap() >= model_bytes as u64,
        "the consent covers the model's bytes: {plan}"
    );
    assert!(ctrl_installed(&b));
    let e = qrtz_app::installed::entry(&data, "ctrl").unwrap();
    assert_eq!(e.program, data.join("packages/ctrl/0.3.0/bin/qrtz-ctrl"));

    // No answer was recorded in ctrl's settings: the installed model is the yes, and the helper gets the compiled
    // model inside the read-only package dir, never a view under models/.
    assert!(!data.join("ctrl/laya.json").exists());
    let pkg = data.join("packages/model.laya/1.0.0");
    assert!(m.consented() && m.installed());
    assert_eq!(m.consent().installed, Some(pkg.join(ENTRY)));
    for (p, bytes) in FILES {
        let f = pkg.join(p);
        assert_eq!(std::fs::read(&f).unwrap(), *bytes, "{p}");
        assert!(
            std::fs::metadata(&f).unwrap().permissions().readonly(),
            "{p} read-only"
        );
    }
    assert!(!data.join("models/laya").exists() && !data.join("models/ctrl/laya").exists());
    let state = m.state(false);
    assert_eq!((state.consented, state.state), (true, "installed"));

    // A model file changed in place is not handed to the helper.
    let w = pkg.join(FILES[2].0);
    let ro = std::fs::metadata(&w).unwrap().permissions();
    std::fs::set_permissions(&w, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(m.consent().installed, None, "writable: refused");
    std::fs::set_permissions(&w, ro).unwrap();
    assert!(m.consent().installed.is_some());

    // A recorded "no" still wins over the installed model (and deletes nothing it doesn't own here: the package
    // stays until ctrl is removed).
    std::fs::create_dir_all(data.join("ctrl")).unwrap();
    std::fs::write(data.join("ctrl/laya.json"), br#"{"consented":false,"at":1}"#).unwrap();
    assert!(!m.consented());
    assert_eq!(m.consent().installed, None);
    std::fs::remove_file(data.join("ctrl/laya.json")).unwrap();
    assert!(m.consent().installed.is_some());

    // Removing ctrl, the model's only user, prunes it: back to the heuristic.
    change(&r, "remove", &["ctrl"]).await.unwrap();
    assert!(!ctrl_installed(&b));
    assert!(!pkg.exists(), "model.laya pruned with ctrl");
    assert!(!m.consented());
    assert_eq!(m.consent().installed, None);
    let (_s, why) = qrtz_ctrl::laya::select_scorer(&m.consent(), None);
    assert!(why.is_some(), "the heuristic answers");
    let _ = qrtz_modules::fsutil::remove_tree(tmp.path());
}

#[tokio::test]
async fn a_laya_file_gone_from_hugging_face_installs_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let pub_dir = tmp.path().join("pub");
    let (out, pin) = publish(&pub_dir);
    let data = tmp.path().join("data");
    let b = app(&data, &out, &pin);
    let r = Router::new(Shared(b.service.clone())).validate_responses(true);
    ok(&r, "marketplace", "packages:refresh", Value::Null).await;

    // The weights are missing at the pinned commit: ctrl isn't half installed, and the package host is never asked
    // for the model instead.
    let blob = hf_file(&pub_dir, FILES[2].0);
    let saved = std::fs::read(&blob).unwrap();
    std::fs::remove_file(&blob).unwrap();
    change(&r, "install", &["ctrl"]).await.unwrap_err();
    assert!(!ctrl_installed(&b));
    assert!(qrtz_app::installed::read_lock(&data).packages.is_empty());
    assert_eq!(laya(&data).consent().installed, None);

    // Back at the pinned URL: the same install succeeds.
    std::fs::write(&blob, saved).unwrap();
    change(&r, "install", &["ctrl"]).await.unwrap();
    assert!(ctrl_installed(&b));
    assert!(laya(&data).consent().installed.is_some());
    let _ = qrtz_modules::fsutil::remove_tree(tmp.path());
}
