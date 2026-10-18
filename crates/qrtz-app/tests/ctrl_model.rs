//! ADR 0116: ctrl's Laya model. Nothing is fetched without the user's yes; the pinned placeholder manifest refuses
//! before any network call; with a real (test) manifest the download is verified (size + sha256), a damaged file is
//! refused, a partial one resumes, and only an installed, consented model reaches `select_scorer` with its path.
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
        "sets": [{"id": "laya", "dir": "laya", "repo": "org/laya", "revision": rev, "files": [
            {"remote": remote, "path": "laya.mlpackage/Data/com.apple.CoreML/model.mlmodel",
             "size": b.len(), "sha256": sha(&b)}]}]
    }))
    .unwrap()
}

fn hits(srv: &Server) -> usize {
    srv.faults.lock().unwrap().hits.values().sum()
}

#[test]
fn the_pinned_manifest_is_a_marked_placeholder() {
    let m = CtrlManifest::pinned();
    assert!(m.placeholder, "flip only with a real url, size and sha256");
    assert_eq!(m.manifest.sets.len(), 1);
    assert_eq!(m.manifest.sets[0].dir, "laya");
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
    // A partial file resumes from where it stopped (a Range request).
    let part = root.join("laya/laya.mlpackage/Data/com.apple.CoreML/model.mlmodel.part");
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&part);
    std::fs::write(&part, &body()[..100_000]).unwrap();
    let last = std::sync::Mutex::new((0u64, 0u64));
    m.download(&DownloadControl::default(), &|p| {
        *last.lock().unwrap() = (p.done, p.total)
    })
    .await
    .unwrap();
    let (done, total) = *last.lock().unwrap();
    assert_eq!((done, total), (300_000, 300_000));
    assert!(
        srv.faults
            .lock()
            .unwrap()
            .ranges
            .iter()
            .any(|(_, r)| r.contains("100000")),
        "resumed: {:?}",
        srv.faults.lock().unwrap().ranges
    );
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

    /// Through the IPC router: off → yes on the pinned placeholder → `unavailable` with the refusal code and no
    /// network → no → off again; a bad payload is a validation error.
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
        // The real pinned manifest, pointed at an address nothing listens on: any fetch would fail differently.
        svc.set_ctrl_model(Arc::new(CtrlModelSlot::new(CtrlLayaModel::new(
            d.path().join("models/ctrl"),
            &d.path().join("ctrl"),
            Some("http://127.0.0.1:9".into()),
        ))));
        let r = Router::new(Shared(svc));
        let v = call(&r, "ctrl:model", Value::Null).await;
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["data"]["state"], "off", "{v}");
        assert_eq!(v["data"]["scorer"], "heuristic");
        let v = call(&r, "ctrl:model-consent", json!({"consent": true})).await;
        assert_eq!(v["data"]["consented"], true, "{v}");
        assert_eq!(v["data"]["state"], "unavailable", "{v}");
        assert_eq!(v["data"]["error"], "laya.placeholder", "{v}");
        assert_eq!(v["data"]["scorer"], "heuristic");
        assert!(!d.path().join("models").exists(), "nothing fetched");
        // The answer persists (a restart reads the same file).
        assert!(CtrlLayaModel::new(d.path().join("models/ctrl"), &d.path().join("ctrl"), None).consented());
        let v = call(&r, "ctrl:model-consent", json!({"consent": false})).await;
        assert_eq!(v["data"]["state"], "off", "{v}");
        let bad = call(&r, "ctrl:model-consent", json!({"consent": "yes"})).await;
        assert_eq!(bad["error"]["code"], "E_VALIDATION", "{bad}");
    }
}
