#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Run 45: ctrl's Laya path against the live signed module index on modules.qrtz.ai (ADR 0153 / 0169 / 0171).
//!
//! - Offline, always: the snapshot of the live timestamp + index in `spec/fixtures/ctrl-live-index/` (read
//!   2026-10-18 10:02 PM PT) verifies under the pinned root 1 (`crates/qrtz-modules/roots/`, the `root1Sha256` in
//!   `module_hosts.json`), lists ctrl 0.2.1 depending on `model.laya ^1.0.0`, and `model.laya 1.0.0` is exactly
//!   ctrl's pinned set (`ctrl_laya_model.json`): the same five files, sizes and hashes, each fetched only from its
//!   Hugging Face URL at the pinned commit, nothing but `qrtz.json` on the package host. The helper's layout fixture
//!   (`native/qrtz-helper/Fixtures/ctrl/laya-live-layout.json`) is the same list.
//! - Offline, always: an index shaped like the live one (the live `model.laya` qrtz.json byte for byte, the five
//!   live paths with stand-in bytes) installs with ctrl, and the helper is handed the `.mlmodelc` it would load as is
//!   (every file regular and read-only, no symlink, `tokenizer.json` beside it).
//! - Network, ignored by default: the real install from modules.qrtz.ai and Hugging Face into a temp data dir
//!   (`cargo test -p qrtz-app --features runtime --test ctrl_live_index -- --ignored`), checked against the real pin.
//!   `QRTZ_CTRL_LIVE_DIR=<new dir>` keeps the install there (e.g. for the helper's full-tokenizer tests).
#![cfg(feature = "runtime")]
use qrtz_app::ctrl_model::{CtrlLayaModel, CtrlManifest};
use qrtz_app::local_models::LocalModels;
use qrtz_app::module_hosting::{Hosts, HttpTransport, Packages};
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::storage_port::NoStorageShell;
use qrtz_app::wiring::{build_app, module_sync, AppBuild, BuiltApp, DevKnobs};
use qrtz_app::{AppInfo, Shared};
use qrtz_helper_bridge::SupervisorOptions;
use qrtz_ipc::Router;
use qrtz_modules::index::{is_pinned_hugging_face_url, Index, IndexFile, PackageEntry};
use qrtz_modules::install::Installer;
use qrtz_modules::trust::{Mode, Trust};
use qrtz_publish::dry_run::NoAppleChecks;
use qrtz_publish::keys::Keys;
use qrtz_publish::{build, write_root, Spec, SpecPackage};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ENTRY: &str = "laya_multilingual_e8_L512_options32.mlmodelc";
/// The live `model.laya` 1.0.0's `qrtz.json` (its `manifestSha256` is in the index).
const LIVE_MODEL_QRTZ_JSON: &str = r#"{"kind":"model","model":{"revision":"7b8d7a2b7e28e746c6ecaad44bbcd5cf251a4fcc","task":"decision","upstream":"FluidInference/laya-coreml"},"name":"model.laya","version":"1.0.0"}"#;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(p: &str) -> Vec<u8> {
    std::fs::read(repo().join(p)).unwrap_or_else(|e| panic!("{p}: {e}"))
}

/// The snapshot, verified exactly as a refresh would, at the time it was issued.
fn live_index() -> Index {
    let snap = "spec/fixtures/ctrl-live-index";
    let pin = Hosts::pinned().root1_sha256;
    assert!(pin.is_some(), "module_hosts.json pins root 1");
    let t = Trust::bootstrap(
        pin.as_deref(),
        &read("crates/qrtz-modules/roots/1.root.json"),
        &read("crates/qrtz-modules/roots/1.root.json.sig"),
    )
    .expect("root 1 is the pinned root");
    let ts = read(&format!("{snap}/timestamp.json"));
    let issued: Value = serde_json::from_slice(&ts).unwrap();
    let v = t
        .verify_pair(
            Mode::Network,
            &ts,
            &read(&format!("{snap}/timestamp.json.sig")),
            &read(&format!("{snap}/index.json")),
            &read(&format!("{snap}/index.json.sig")),
            issued["issuedMs"].as_u64().unwrap(),
        )
        .expect("the live timestamp and index verify under root 1");
    assert_eq!((v.timestamp.version, v.index.version), (4, 2));
    v.index
}

fn latest<'a>(ix: &'a Index, name: &str) -> (&'a str, &'a PackageEntry) {
    ix.latest(name)
        .unwrap_or_else(|| panic!("{name} in the live index"))
}

#[derive(serde::Deserialize)]
struct Layout {
    entry: String,
    tokenizer: String,
    files: Vec<LayoutFile>,
}
#[derive(serde::Deserialize)]
struct LayoutFile {
    path: String,
    size: u64,
    sha256: String,
}

fn layout() -> Layout {
    serde_json::from_slice(&read("native/qrtz-helper/Fixtures/ctrl/laya-live-layout.json")).unwrap()
}

#[test]
fn the_live_index_lists_ctrl_with_exactly_the_pinned_laya_set() {
    let ix = live_index();

    // ctrl: the newest version is the one main released, and it depends on model.laya as the repo's manifest says.
    let (cv, ctrl) = latest(&ix, "ctrl");
    assert_eq!(cv, "0.2.1");
    let repo_manifest: Value = serde_json::from_slice(&read("modules/ctrl/qrtz.json")).unwrap();
    let deps: std::collections::BTreeMap<String, String> =
        serde_json::from_value(repo_manifest["dependencies"].clone()).unwrap();
    assert_eq!(ctrl.dependencies, deps);
    assert_eq!(ctrl.models, ["model.laya"]);
    assert_eq!(ctrl.identifier.as_deref(), Some("ai.qrtz.ctrl"));
    assert_eq!(ctrl.platform, "macos-arm64");
    let n = ctrl.notarization.as_ref().expect("ADR 0170: notarized");
    assert_eq!(n.status, "Accepted");
    assert!(ctrl
        .files
        .iter()
        .any(|f| f.path == "bin/qrtz-ctrl" && f.executable));
    assert!(
        ctrl.files.iter().all(|f| f.mirrors.is_empty()),
        "ctrl's code is only on the package host"
    );

    // model.laya: ^1.0.0 resolves to 1.0.0, the decision model, ~484 MB.
    let req = semver::VersionReq::parse(&ctrl.dependencies["model.laya"]).unwrap();
    let m = ix.matching("model.laya", &req);
    assert_eq!(m.len(), 1);
    let (mv, laya) = (&m[0].0, m[0].1);
    assert_eq!(mv.to_string(), "1.0.0");
    assert_eq!(laya.task.as_deref(), Some("decision"));
    assert_eq!(laya.size, laya.files.iter().map(|f| f.size).sum::<u64>());
    assert_eq!(laya.size, 483_924_622);
    assert_eq!(
        laya.manifest_sha256,
        qrtz_modules::sha256_hex(LIVE_MODEL_QRTZ_JSON.as_bytes())
    );

    // ADR 0171: every model byte comes from its pinned Hugging Face URL; only qrtz.json is on the package host.
    let pin = CtrlManifest::pinned();
    assert!(!pin.placeholder);
    let set = &pin.manifest.sets[0];
    assert_eq!(
        (set.package.as_str(), set.package_version.as_deref()),
        ("model.laya", Some("1.0.0"))
    );
    let model_files: Vec<&IndexFile> = laya.files.iter().filter(|f| f.path != "qrtz.json").collect();
    assert_eq!(model_files.len(), set.files.len());
    for f in &laya.files {
        if f.path == "qrtz.json" {
            assert!(f.mirrors.is_empty());
            continue;
        }
        let p = set
            .files
            .iter()
            .find(|p| p.path == f.path)
            .unwrap_or_else(|| panic!("{} is in ctrl_laya_model.json", f.path));
        assert_eq!(
            (f.size, f.sha256.as_str()),
            (p.size, p.sha256.as_str()),
            "{}",
            f.path
        );
        let url = format!(
            "{}/{}/resolve/{}/{}",
            pin.manifest.base, set.repo, set.revision, p.remote
        );
        assert_eq!(f.mirrors.len(), 1, "{}", f.path);
        assert_eq!(f.mirrors[0], url, "{}", f.path);
        assert!(is_pinned_hugging_face_url(&url), "{url}");
    }

    // The entry ctrl hands the helper is the live package's only top-level .mlmodelc, with tokenizer.json beside it.
    let tops: std::collections::BTreeSet<&str> = laya
        .files
        .iter()
        .map(|f| f.path.split('/').next().unwrap())
        .collect();
    assert_eq!(
        tops.into_iter().collect::<Vec<_>>(),
        [ENTRY, "qrtz.json", "tokenizer.json"]
    );

    // The helper's layout fixture is this list.
    let l = layout();
    assert_eq!(
        (l.entry.as_str(), l.tokenizer.as_str()),
        (ENTRY, "tokenizer.json")
    );
    let live: Vec<(&str, u64, &str)> = laya
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.size, f.sha256.as_str()))
        .collect();
    let fixture: Vec<(&str, u64, &str)> = l
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.size, f.sha256.as_str()))
        .collect();
    assert_eq!(live, fixture);
}

// ---- an index shaped like the live one, installed ----

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

/// Stand-in bytes for a live path (the real weights are 449 MB).
fn stand_in(path: &str) -> Vec<u8> {
    format!("stand-in for {path}").into_bytes()
}

fn model_paths() -> Vec<String> {
    layout()
        .files
        .into_iter()
        .map(|f| f.path)
        .filter(|p| p != "qrtz.json")
        .collect()
}

/// `model.laya` with the live qrtz.json and paths (files only on "Hugging Face"), and ctrl as main ships it.
fn publish(dir: &Path) -> (PathBuf, String) {
    let laya = dir.join("src/model.laya");
    write(&laya.join("qrtz.json"), LIVE_MODEL_QRTZ_JSON.as_bytes(), false);
    let rev = "7b8d7a2b7e28e746c6ecaad44bbcd5cf251a4fcc";
    for p in model_paths() {
        write(&laya.join(&p), &stand_in(&p), false);
        write(
            &dir.join(format!("hf/FluidInference/laya-coreml/resolve/{rev}/{p}")),
            &stand_in(&p),
            false,
        );
    }
    let ctrl = dir.join("src/ctrl");
    write(&ctrl.join("qrtz.json"), &read("modules/ctrl/qrtz.json"), false);
    write(&ctrl.join("bin/qrtz-ctrl"), b"#!/bin/sh\necho ctrl\n", true);
    let out = dir.join("out");
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

/// ctrl's pin with the live paths and the stand-in bytes' sizes and hashes.
fn stand_in_manifest() -> CtrlManifest {
    let mut m = CtrlManifest::pinned();
    m.manifest.version = "laya-live-layout-stand-in".into();
    for f in &mut m.manifest.sets[0].files {
        let b = stand_in(&f.path);
        f.size = b.len() as u64;
        f.sha256 = qrtz_modules::sha256_hex(&b);
    }
    m
}

fn app(data: &Path, packages: Packages, views: CtrlManifest) -> BuiltApp {
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
        packages: Some(packages),
        shell: Arc::new(Shell),
        storage_shell: Arc::new(NoStorageShell),
        speech: None,
        talk: None,
        steps: None,
        knobs: DevKnobs::default(),
    })
    .unwrap();
    let views =
        LocalModels::with_manifest(data.join("models"), views.manifest, None).with_packages_root(data);
    built
        .service
        .track_installed_modules(module_sync(data.to_path_buf(), Some(views)));
    built
}

async fn ok(r: &Router<Shared>, ch: &str, payload: Value) -> Value {
    let v = r
        .dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":"marketplace","payload":payload}))
        .await;
    assert_eq!(v["ok"], true, "{ch}: {v}");
    v["data"].clone()
}

/// Refresh, plan `install ctrl`, apply it; the plan.
async fn install_ctrl(r: &Router<Shared>) -> Value {
    ok(r, "packages:refresh", Value::Null).await;
    let plan = ok(r, "packages:plan", json!({"op": "install", "names": ["ctrl"]})).await;
    ok(
        r,
        "packages:apply",
        json!({"planSha256": plan["planSha256"], "downloadBytes": plan["downloadBytes"]}),
    )
    .await;
    plan
}

fn plan_names(plan: &Value) -> Vec<String> {
    let mut v: Vec<String> = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str().map(str::to_owned))
        .collect();
    v.sort();
    v
}

/// What the helper's `CtrlLayaLoad.plan(fromHost: true)` + `CtrlLayaWarm.tokenizerPath` accept, from the host side:
/// a `.mlmodelc` dir whose every entry is a dir or a regular file with no write bit (no symlink), at least one file,
/// and `tokenizer.json` beside it.
fn helper_would_load(model: &Path) -> Result<(), String> {
    if model.extension().and_then(|e| e.to_str()) != Some("mlmodelc") {
        return Err("laya.needs_compiled".into());
    }
    let mut files = 0;
    let mut stack = vec![model.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| e.to_string())? {
            let p = e.map_err(|e| e.to_string())?.path();
            let m = std::fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
            if m.file_type().is_symlink() {
                return Err(format!("laya.not_read_only: symlink {}", p.display()));
            } else if m.is_dir() {
                stack.push(p);
            } else if !m.is_file() || m.permissions().mode() & 0o222 != 0 {
                return Err(format!("laya.not_read_only: {}", p.display()));
            } else {
                files += 1;
            }
        }
    }
    if files == 0 {
        return Err("laya.not_read_only: empty".into());
    }
    let tok = model.parent().unwrap().join("tokenizer.json");
    if !tok.is_file() {
        return Err("laya.no_tokenizer".into());
    }
    Ok(())
}

#[tokio::test]
async fn an_index_shaped_like_the_live_one_installs_ctrl_and_hands_the_helper_its_mlmodelc() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, pin) = publish(&tmp.path().join("pub"));
    let data = tmp.path().join("data");
    let packages = Packages::with(
        Installer::new(&data, Some(pin), Box::new(NoAppleChecks)),
        HttpTransport::local_dir(&out).with_url_dir("https://huggingface.co/", tmp.path().join("pub/hf")),
    );
    let b = app(&data, packages, stand_in_manifest());
    let r = Router::new(Shared(b.service.clone())).validate_responses(true);
    let m = CtrlLayaModel::with_manifest(
        data.join("models/ctrl"),
        &data.join("ctrl"),
        stand_in_manifest(),
        None,
    );
    assert_eq!(m.consent().installed, None);

    // The fixture's model manifest is the live one, byte for byte.
    let ix: Index = serde_json::from_slice(&read("spec/fixtures/ctrl-live-index/index.json")).unwrap();
    let live = &ix.packages["model.laya"]["1.0.0"];
    let pub_ix: Value = {
        let ts: Value =
            serde_json::from_slice(&std::fs::read(out.join("v1/timestamp.json")).unwrap()).unwrap();
        let sha = ts["index"]["sha256"].as_str().unwrap().to_owned();
        serde_json::from_slice(&std::fs::read(out.join(format!("v1/index/{sha}.json"))).unwrap()).unwrap()
    };
    let fixture_laya = &pub_ix["packages"]["model.laya"]["1.0.0"];
    assert_eq!(fixture_laya["manifestSha256"], live.manifest_sha256.as_str());
    let paths = |files: &Value| -> Vec<String> {
        files
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap().to_owned())
            .collect()
    };
    let live_paths: Vec<String> = live.files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(paths(&fixture_laya["files"]), live_paths);
    for f in fixture_laya["files"].as_array().unwrap() {
        let mirrors = f["mirrors"].as_array().map(Vec::len).unwrap_or(0);
        assert_eq!(mirrors, usize::from(f["path"] != "qrtz.json"), "{f}");
    }

    let plan = install_ctrl(&r).await;
    assert_eq!(plan_names(&plan), ["ctrl", "model.laya"], "{plan}");

    let pkg = data.join("packages/model.laya/1.0.0");
    let model = m.consent().installed.expect("installed model.laya is ctrl's yes");
    assert_eq!(model, pkg.join(ENTRY));
    helper_would_load(&model).unwrap();
    for p in model_paths() {
        assert_eq!(std::fs::read(pkg.join(&p)).unwrap(), stand_in(&p), "{p}");
    }
    assert!(!data.join("models/laya").exists() && !data.join("models/ctrl/laya").exists());

    // The helper-side rule refuses what the store never produces: a writable file, a symlink.
    let w = pkg.join(format!("{ENTRY}/analytics/coremldata.bin"));
    let ro = std::fs::metadata(&w).unwrap().permissions();
    std::fs::set_permissions(&w, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(helper_would_load(&model)
        .unwrap_err()
        .starts_with("laya.not_read_only"));
    assert_eq!(m.consent().installed, None, "the host refuses it too");
    std::fs::set_permissions(&w, ro).unwrap();
    assert!(m.consent().installed.is_some());
    let _ = qrtz_modules::fsutil::remove_tree(tmp.path());
}

// ---- the real thing (network) ----

#[tokio::test]
#[ignore = "network: installs ctrl + model.laya (~484 MB) from modules.qrtz.ai and Hugging Face"]
async fn live_install_of_ctrl_from_modules_qrtz_ai() {
    let keep = std::env::var_os("QRTZ_CTRL_LIVE_DIR").map(PathBuf::from);
    let tmp = tempfile::tempdir().unwrap();
    let data = keep.clone().unwrap_or_else(|| tmp.path().join("data"));
    assert!(
        !data.join("packages").exists(),
        "{} must be a fresh dir",
        data.display()
    );
    let packages = Packages::with(
        // The real pin and hosts; only the Apple code checks are off (no codesign on Linux).
        Installer::new(&data, Hosts::pinned().root1_sha256, Box::new(NoAppleChecks)),
        HttpTransport::pinned(),
    );
    let b = app(&data, packages, CtrlManifest::pinned());
    let r = Router::new(Shared(b.service.clone())).validate_responses(true);
    let m = CtrlLayaModel::new(data.join("models/ctrl"), &data.join("ctrl"), None);
    assert_eq!(m.consent().installed, None);

    let t0 = std::time::Instant::now();
    let plan = install_ctrl(&r).await;
    eprintln!("live install: {:?}, plan {plan}", t0.elapsed());
    assert_eq!(plan_names(&plan), ["ctrl", "model.laya"], "{plan}");
    assert!(plan["downloadBytes"].as_u64().unwrap() >= 483_924_622, "{plan}");

    // The real pin (ctrl_laya_model.json) accepts the installed files: read-only, of their pinned sizes.
    let model = m.consent().installed.expect("the live model.laya is ctrl's yes");
    let pkg = data.join("packages/model.laya/1.0.0");
    assert_eq!(model, pkg.join(ENTRY));
    helper_would_load(&model).unwrap();
    for f in &CtrlManifest::pinned().manifest.sets[0].files {
        let b = std::fs::read(pkg.join(&f.path)).unwrap();
        assert_eq!(
            (b.len() as u64, qrtz_modules::sha256_hex(&b)),
            (f.size, f.sha256.clone()),
            "{}",
            f.path
        );
    }
    let ctrl = data.join("packages/ctrl/0.2.1/bin/qrtz-ctrl");
    assert!(ctrl.is_file(), "{}", ctrl.display());
    eprintln!(
        "live install kept at {} (model {})",
        data.display(),
        model.display()
    );
    if keep.is_none() {
        let _ = qrtz_modules::fsutil::remove_tree(tmp.path());
    }
}
