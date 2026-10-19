//! ADR 0116 / 0129: ctrl's Laya model. Nothing is fetched without the user's yes; a placeholder manifest refuses
//! before any network call; the pinned manifest is laya-coreml @ 7b8d7a2 (e8, L512) + tokenizer.json; with a test
//! manifest the download is verified (size + sha256) through the package manager, a damaged file is
//! refused and fetched again, and only an installed, consented model reaches `select_scorer` with its path.
//! Withdrawing consent deletes the model. Served by the qrtz-speech test kit on loopback.
//!
//! ADR 0157 §6/§9 (Bozniak): model.laya is the one model an extension reads in its own process, so it installs only
//! under a fresh signed list (a loopback-published index here; none at all is refused before any model request),
//! and ctrl loads it read-only straight from its store-linked package dir, never from a writable view.
use qrtz_app::ctrl_model::{CtrlLayaModel, CtrlManifest};
use qrtz_speech::download::DownloadControl;
use qrtz_speech::testkit::{sha, Server};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn body() -> Vec<u8> {
    (0..300_000).map(|i| (i % 251) as u8).collect()
}

fn manifest(srv: &Server, placeholder: bool) -> CtrlManifest {
    let b = body();
    let rev = "c".repeat(40);
    let remote = "coreml/laya.mlpackage/Data/com.apple.CoreML/model.mlmodel";
    srv.put(&format!("org/laya/resolve/{rev}/{remote}"), b.clone());
    serde_json::from_value(json!({
        "version": "1.0.0", "placeholder": placeholder, "base": "https://example.invalid",
        "sets": [{"id": "laya", "package": "model.laya", "task": "decision", "dir": "laya", "repo": "org/laya", "revision": rev, "files": [
            {"remote": remote, "path": "laya.mlpackage/Data/com.apple.CoreML/model.mlmodel",
             "size": b.len(), "sha256": sha(&b)}]}]
    }))
    .unwrap()
}

/// A signed, empty index (root, timestamp, revocations) on its own loopback server, and its root pin.
struct Index {
    _out: tempfile::TempDir,
    srv: Server,
    pin: String,
}

async fn index() -> Index {
    let out = tempfile::tempdir().unwrap();
    let keys = qrtz_publish::keys::Keys::ephemeral();
    let now = qrtz_publish::now_ms();
    let (_, pin) = qrtz_publish::write_root(out.path(), &keys, None, 1, now).unwrap();
    let spec = qrtz_publish::Spec {
        team_id: qrtz_publish::dry_run::TEAM.into(),
        packages: vec![],
        pinned_models: vec![],
        models: vec![],
        revoked: vec![],
    };
    qrtz_publish::build(out.path(), &spec, &keys, now).unwrap();
    let srv = Server::start().await;
    fn serve(dir: &Path, base: &Path, srv: &Server) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                serve(&p, base, srv);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                srv.put(&rel, std::fs::read(&p).unwrap());
            }
        }
    }
    serve(out.path(), out.path(), &srv);
    Index { _out: out, srv, pin }
}

impl Index {
    fn on(&self, m: CtrlLayaModel) -> CtrlLayaModel {
        m.with_index(Some(self.pin.clone()), vec![self.srv.base.clone()])
    }
}

/// Installed package dirs are read-only.
fn cleanup(d: tempfile::TempDir) {
    let _ = qrtz_modules::fsutil::remove_tree(d.path());
}

fn package(d: &Path) -> PathBuf {
    d.join("packages/model.laya/1.0.0")
}

fn hits(srv: &Server) -> usize {
    srv.faults.lock().unwrap().hits.values().sum()
}

const REPO: &str = "FluidInference/laya-coreml";
const REV: &str = "7b8d7a2b7e28e746c6ecaad44bbcd5cf251a4fcc";
const BUCKET: &str = "laya_multilingual_e8_L512_options32.mlmodelc";

/// ADR 0129: the product default is pinned for real: one set, the compiled e8 L512 bucket's four files and the
/// tokenizer next to it, each with a size and a sha256; the helper gets the `.mlmodelc` and reads
/// `tokenizer.json` beside it.
#[test]
fn the_pinned_manifest_is_laya_coreml_e8_l512_with_its_tokenizer() {
    let m = CtrlManifest::pinned();
    assert!(!m.placeholder);
    assert_eq!(m.manifest.base, "https://huggingface.co");
    assert_eq!(m.manifest.sets.len(), 1);
    let set = &m.manifest.sets[0];
    assert_eq!(
        (set.dir.as_str(), set.repo.as_str(), set.revision.as_str()),
        ("laya", REPO, REV)
    );
    let paths: Vec<&str> = set.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            format!("{BUCKET}/coremldata.bin"),
            format!("{BUCKET}/analytics/coremldata.bin"),
            format!("{BUCKET}/model.mil"),
            format!("{BUCKET}/weights/weight.bin"),
            "tokenizer.json".into()
        ]
    );
    for f in &set.files {
        assert_eq!(f.remote, f.path, "mirrors the repo layout");
        assert_eq!(f.sha256.len(), 64);
        assert!(f
            .sha256
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert!(f.size > 0);
    }
    let weights = set.files.iter().find(|f| f.path.ends_with("weight.bin")).unwrap();
    assert_eq!(weights.size, 449_175_040);
    assert_eq!(
        weights.sha256,
        "70c299c32d118ab1e2549e9c0088d712bfe2e936ad37ab8f9b7366f277afed4b"
    );
    let tok = set.files.iter().find(|f| f.path == "tokenizer.json").unwrap();
    assert_eq!(tok.size, 34_363_188);
    assert_eq!(m.manifest.total_size(), 483_924_446);
    let d = tempfile::tempdir().unwrap();
    let model = CtrlLayaModel::new(d.path().join("models/ctrl"), &d.path().join("ctrl"), None);
    assert_eq!(model.model_path(), None, "not installed");
    assert_eq!(model.state(true).size_bytes, 483_924_446);
}

/// The real file list, served from loopback with small bodies: a consented download installs the bucket and the
/// tokenizer side by side, and only that bucket's path reaches the scorer. Without consent nothing is requested.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pinned_layout_installs_the_bucket_and_its_tokenizer_side_by_side() {
    let srv = Server::start().await;
    let mut m = CtrlManifest::pinned();
    for (i, f) in m.manifest.sets[0].files.iter_mut().enumerate() {
        let b: Vec<u8> = (0..1000 + i * 7).map(|j| (j % 13) as u8).collect();
        srv.put(&format!("{REPO}/resolve/{REV}/{}", f.remote), b.clone());
        f.size = b.len() as u64;
        f.sha256 = sha(&b);
    }
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("models/ctrl");
    let idx = index().await;
    let model = idx.on(CtrlLayaModel::with_manifest(
        &root,
        &d.path().join("ctrl"),
        m,
        Some(srv.base.clone()),
    ));
    let e = model
        .download(&DownloadControl::default(), &|_| {})
        .await
        .unwrap_err();
    assert!(e.to_string().contains("laya.no_consent"), "{e}");
    assert_eq!(hits(&srv), 0, "no request without a yes");
    model.set_consent(true, 1).unwrap();
    model
        .download(&DownloadControl::default(), &|_| {})
        .await
        .unwrap();
    assert_eq!(hits(&srv), 5);
    // Straight from the store-linked package, read-only; no view copy under models/.
    let bucket = package(d.path()).join(BUCKET);
    for f in [
        "coremldata.bin",
        "analytics/coremldata.bin",
        "model.mil",
        "weights/weight.bin",
    ] {
        let m = std::fs::metadata(bucket.join(f)).unwrap();
        assert!(m.is_file(), "{f}");
        assert_eq!(m.permissions().mode() & 0o222, 0, "{f} is read-only");
    }
    assert!(package(d.path()).join("tokenizer.json").is_file());
    assert!(!root.join("laya").exists(), "no writable copy");
    assert_eq!(model.consent().installed.as_deref(), Some(bucket.as_path()));
    assert_eq!(model.state(true).state, "installed");
    model.set_consent(false, 2).unwrap();
    assert!(!package(d.path()).exists(), "removed and pruned");
    assert_eq!(model.consent().installed, None);
    cleanup(d);
}

/// No fresh signed list (none pinned, none served): refused before any model request, the heuristic answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_fresh_signed_list_model_laya_is_refused_like_code() {
    let srv = Server::start().await;
    let d = tempfile::tempdir().unwrap();
    let m = CtrlLayaModel::with_manifest(
        d.path().join("models/ctrl"),
        &d.path().join("ctrl"),
        manifest(&srv, false),
        Some(srv.base.clone()),
    );
    m.set_consent(true, 1).unwrap();
    let e = m
        .download(&DownloadControl::default(), &|_| {})
        .await
        .unwrap_err();
    assert!(matches!(e, qrtz_speech::SpeechError::RevocationExpired), "{e}");
    assert_eq!(hits(&srv), 0, "no model request");
    assert_eq!(m.consent().installed, None);
    assert_eq!(m.state(true).scorer, "heuristic");
    cleanup(d);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_download_without_consent_or_on_the_placeholder() {
    let srv = Server::start().await;
    let d = tempfile::tempdir().unwrap();
    let ctrl_dir = d.path().join("ctrl");
    let m = CtrlLayaModel::with_manifest(
        d.path().join("models/ctrl"),
        &ctrl_dir,
        manifest(&srv, true),
        Some(srv.base.clone()),
    );
    let st = m.state(true);
    assert_eq!((st.consented, st.state, st.scorer), (false, "off", "heuristic"));
    let e = m
        .download(&DownloadControl::default(), &|_| {})
        .await
        .unwrap_err();
    assert!(e.to_string().contains("laya.no_consent"), "{e}");
    m.set_consent(true, 1).unwrap();
    assert!(m.consented());
    let mode = std::fs::metadata(ctrl_dir.join("laya.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(m.state(true).state, "unavailable");
    let e = m
        .download(&DownloadControl::default(), &|_| {})
        .await
        .unwrap_err();
    assert!(e.to_string().contains("laya.placeholder"), "{e}");
    assert_eq!(hits(&srv), 0, "nothing went on the network");
    assert!(!d.path().join("models").exists());
    assert_eq!(m.consent().installed, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_consented_download_is_verified_resumable_and_reaches_the_scorer() {
    let srv = Server::start().await;
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("models/ctrl");
    let idx = index().await;
    let m = idx.on(CtrlLayaModel::with_manifest(
        &root,
        &d.path().join("ctrl"),
        manifest(&srv, false),
        Some(srv.base.clone()),
    ));
    m.set_consent(true, 1).unwrap();
    assert_eq!(m.state(true).state, "not_installed");
    // A damaged file is refused and nothing is installed.
    {
        let mut f = srv.faults.lock().unwrap();
        let p = srv.routes.lock().unwrap().keys().next().unwrap().clone();
        f.corrupt.insert(p);
    }
    assert!(m.download(&DownloadControl::default(), &|_| {}).await.is_err());
    assert!(!m.installed());
    srv.faults.lock().unwrap().corrupt.clear();
    // The retry goes through the package manager's fetcher (range resumes are tested there: qrtz-publish
    // tests/pm.rs and module_hosting.rs).
    let last = std::sync::Mutex::new((0u64, 0u64));
    m.download(&DownloadControl::default(), &|p| {
        *last.lock().unwrap() = (p.done, p.total)
    })
    .await
    .unwrap();
    let (done, total) = *last.lock().unwrap();
    assert_eq!((done, total), (300_000, 300_000));
    assert!(srv
        .faults
        .lock()
        .unwrap()
        .ranges
        .iter()
        .all(|(_, r)| r == "bytes=0-299999"));
    assert!(m.installed());
    let st = m.state(true);
    assert_eq!((st.state, st.scorer), ("installed", "laya"));
    assert_eq!(m.state(false).scorer, "heuristic", "no runtime in this build");
    let c = m.consent();
    assert!(c.consented);
    let entry = package(d.path()).join("laya.mlpackage");
    assert_eq!(c.installed.as_deref(), Some(entry.as_path()));
    // Checked at every load: a file made writable (or changed in size) is not handed to the helper.
    let f = entry.join("Data/com.apple.CoreML/model.mlmodel");
    let ro = std::fs::metadata(&f).unwrap().permissions();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(m.consent().installed, None, "writable: refused");
    std::fs::set_permissions(&f, ro).unwrap();
    assert_eq!(m.consent().installed.as_deref(), Some(entry.as_path()));
    // ADR 0169: with no answer recorded, the model installed (as ctrl's dependency) is the yes.
    let answer = d.path().join("ctrl/laya.json");
    let saved = std::fs::read(&answer).unwrap();
    std::fs::remove_file(&answer).unwrap();
    assert!(m.consented() && m.consent().installed.is_some());
    std::fs::write(&answer, saved).unwrap();
    // Off this macOS `laya` build the heuristic still answers.
    let (_s, why) = qrtz_ctrl::laya::select_scorer(&c, None);
    assert_eq!(why, Some(qrtz_ctrl::laya::Fallback::NotThisBuild));
    // Withdrawing consent deletes the model; the path no longer reaches the scorer.
    m.set_consent(false, 2).unwrap();
    assert!(!package(d.path()).exists());
    assert_eq!(m.consent().installed, None);
    assert_eq!(m.state(true).state, "off");
    cleanup(d);
}

mod router {
    use qrtz_app::ctrl_model::{CtrlLayaModel, CtrlModelSlot};
    use qrtz_app::ports::*;
    use qrtz_app::*;
    use qrtz_ipc::Router;
    use serde_json::{json, Value};
    use std::sync::Arc;

    async fn call(r: &Router<Shared>, ch: &str, payload: Value) -> Value {
        r.dispatch(json!({"v":1,"id":"t","channel":ch,"version":1,"subApp":"ctrl","payload":payload}))
            .await
    }

    /// Through the IPC router with the real pinned manifest and no signed index (as today, before modules.qrtz.ai
    /// is live): off (nothing requested) → yes → the download is refused like code (ADR 0157 §6) → `failed` with
    /// `laya.needs_signed_index`, nothing requested or kept → persisted → no → off; a bad payload is a validation
    /// error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn consent_and_state_through_the_ctrl_channels() {
        let d = tempfile::tempdir().unwrap();
        let svc = AppService::new(AppDeps {
            info: AppInfo {
                name: "qrtz".into(),
                version: "0.1.0".into(),
                platform: "linux".into(),
                arch: "x64".into(),
                shell: "tauri test".into(),
            },
            config: ConfigStore::open(d.path().join("config.json")).into(),
            runtime: Arc::new(StubRuntime::default()),
            storage: Arc::new(StubStorage),
            talk: Arc::new(StubTalk),
            shell: Arc::new(NoShell),
            helper: None,
            events: Events::new(),
        });
        let srv = qrtz_speech::testkit::Server::start().await;
        svc.set_ctrl_model(Arc::new(CtrlModelSlot::new(CtrlLayaModel::new(
            d.path().join("models/ctrl"),
            &d.path().join("ctrl"),
            Some(srv.base.clone()),
        ))));
        let r = Router::new(Shared(svc));
        let v = call(&r, "ctrl:model", Value::Null).await;
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["data"]["state"], "off", "{v}");
        assert_eq!(v["data"]["scorer"], "heuristic");
        assert_eq!(v["data"]["sizeBytes"], 483_924_446u64);
        let hits = || srv.faults.lock().unwrap().hits.clone();
        assert!(hits().is_empty(), "no request before the yes");
        let v = call(&r, "ctrl:model-consent", json!({"consent": true})).await;
        assert_eq!(v["data"]["consented"], true, "{v}");
        let mut v = v;
        for _ in 0..50 {
            if v["data"]["state"] == "failed" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            v = call(&r, "ctrl:model", Value::Null).await;
        }
        assert_eq!(v["data"]["state"], "failed", "{v}");
        assert_eq!(v["data"]["error"], "laya.needs_signed_index", "{v}");
        assert_eq!(v["data"]["scorer"], "heuristic");
        assert!(hits().is_empty(), "no model request without a fresh signed list");
        assert_eq!(v["data"]["bytesOnDisk"], 0, "nothing kept: {v}");
        // The answer persists (a restart reads the same file).
        assert!(CtrlLayaModel::new(d.path().join("models/ctrl"), &d.path().join("ctrl"), None).consented());
        let v = call(&r, "ctrl:model-consent", json!({"consent": false})).await;
        assert_eq!(v["data"]["state"], "off", "{v}");
        let bad = call(&r, "ctrl:model-consent", json!({"consent": "yes"})).await;
        assert_eq!(bad["error"]["code"], "E_VALIDATION", "{bad}");
    }
}
