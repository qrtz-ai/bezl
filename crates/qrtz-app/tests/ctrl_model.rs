//! ADR 0116 / 0129: ctrl's Laya model. Nothing is fetched without the user's yes; a placeholder manifest refuses
//! before any network call; the pinned manifest is laya-coreml @ 7b8d7a2 (e8, L512) + tokenizer.json; with a test
//! manifest the download is verified (size + sha256) through the package manager, a damaged file is
//! refused and fetched again, and only an installed, consented model reaches `select_scorer` with its path.
//! Withdrawing consent deletes the model. Served by the qrtz-speech test kit on loopback.
use qrtz_app::ctrl_model::{CtrlLayaModel, CtrlManifest};
use qrtz_speech::download::DownloadControl;
use qrtz_speech::testkit::{sha, Server};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

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
    assert_eq!(model.model_path(), d.path().join("models/ctrl/laya").join(BUCKET));
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
    let model = CtrlLayaModel::with_manifest(&root, &d.path().join("ctrl"), m, Some(srv.base.clone()));
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
    let bucket = root.join("laya").join(BUCKET);
    for f in [
        "coremldata.bin",
        "analytics/coremldata.bin",
        "model.mil",
        "weights/weight.bin",
    ] {
        assert!(bucket.join(f).is_file(), "{f}");
    }
    assert!(root.join("laya/tokenizer.json").is_file());
    assert_eq!(
        bucket.parent().unwrap().join("tokenizer.json"),
        root.join("laya/tokenizer.json")
    );
    assert_eq!(model.consent().installed.as_deref(), Some(bucket.as_path()));
    assert_eq!(model.state(true).state, "installed");
    model.set_consent(false, 2).unwrap();
    assert!(!root.join("laya").exists());
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
    let m = CtrlLayaModel::with_manifest(
        &root,
        &d.path().join("ctrl"),
        manifest(&srv, false),
        Some(srv.base.clone()),
    );
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
    assert_eq!(
        c.installed.as_deref(),
        Some(root.join("laya/laya.mlpackage").as_path())
    );
    // Off this macOS `laya` build the heuristic still answers.
    let (_s, why) = qrtz_ctrl::laya::select_scorer(&c, None);
    assert_eq!(why, Some(qrtz_ctrl::laya::Fallback::NotThisBuild));
    // Withdrawing consent deletes the model; the path no longer reaches the scorer.
    m.set_consent(false, 2).unwrap();
    assert!(!root.join("laya").exists());
    assert_eq!(m.consent().installed, None);
    assert_eq!(m.state(true).state, "off");
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

    /// Through the IPC router with the real pinned manifest on a loopback server that has none of the files: off
    /// (nothing requested) → yes → the download asks loopback for the pinned bucket's first file and fails its
    /// check → `failed` with a code, nothing kept → persisted → no → off; a bad payload is a validation error.
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
        assert!(v["data"]["error"].is_string(), "{v}");
        assert_eq!(v["data"]["scorer"], "heuristic");
        // Only loopback, only the pinned paths (files are fetched in parallel now, so several may be asked for).
        let pinned: Vec<String> = super::CtrlManifest::pinned().manifest.sets[0]
            .files
            .iter()
            .map(|f| format!("{}/resolve/{}/{}", super::REPO, super::REV, f.remote))
            .collect();
        let asked: Vec<String> = hits().into_keys().collect();
        assert!(!asked.is_empty());
        assert!(asked.iter().all(|a| pinned.contains(a)), "{asked:?}");
        assert_eq!(v["data"]["bytesOnDisk"], 0, "nothing kept: {v}");
        // The answer persists (a restart reads the same file).
        assert!(CtrlLayaModel::new(d.path().join("models/ctrl"), &d.path().join("ctrl"), None).consented());
        let v = call(&r, "ctrl:model-consent", json!({"consent": false})).await;
        assert_eq!(v["data"]["state"], "off", "{v}");
        let bad = call(&r, "ctrl:model-consent", json!({"consent": "yes"})).await;
        assert_eq!(bad["error"]["code"], "E_VALIDATION", "{bad}");
    }
}
