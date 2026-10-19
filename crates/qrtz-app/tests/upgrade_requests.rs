//! ADR 0169 upgrade (Bobs and Bozniak, 902bcab review): a Mac upgrading to the empty shell has the app's old model
//! requests in `qrtz.lock` (echo's and nano's models, pinned by the app before modules were packages). At launch,
//! `LocalModels::sync_views` drops the ones whose modules aren't installed, with their views, packages and blobs; a
//! model an installed module uses keeps its request, package and view, and a request the user made from the index is
//! never touched.
use qrtz_app::local_models::{orphaned_app_requests, DownloadControl, LocalModels, Manifest, MARKER};
use qrtz_modules::index::PackageKind;
use qrtz_modules::qrtz_lock::{key, QrtzLock, Source};
use qrtz_speech::testkit::{sha, Server};
use serde_json::{json, Value};
use std::path::Path;

/// (set id, package, dir) as in `local_models.json`.
const SETS: [(&str, &str, &str); 4] = [
    ("asr", "model.nemotron", "nemotron-560ms"),
    ("diarizer", "model.nemotron-diarizer", "nemotron3-diarizer"),
    ("echo", "model.localvqe", "localvqe"),
    ("echo-asr", "model.parakeet-tdt-v2-en", "parakeet-tdt-v2"),
];
const NANO: [&str; 3] = ["model.nemotron", "model.nemotron-diarizer", "model.localvqe"];
const ECHO: &str = "model.parakeet-tdt-v2-en";

fn body(i: usize) -> Vec<u8> {
    (0..4096 + i * 512)
        .map(|n| (n % 251) as u8 ^ (i as u8 + 7))
        .collect()
}

fn serve(srv: &Server) -> Manifest {
    let sets: Vec<Value> = SETS
        .iter()
        .enumerate()
        .map(|(i, (id, package, dir))| {
            let b = body(i);
            let rev = format!("{i}").repeat(40);
            let repo = format!("org/{dir}");
            srv.put(&format!("{repo}/resolve/{rev}/w.bin"), b.clone());
            json!({"id": id, "package": package, "task": "asr", "dir": dir, "repo": repo, "revision": rev,
                   "files": [{"remote": "w.bin", "path": "w.bin", "size": b.len(), "sha256": sha(&b)}]})
        })
        .collect();
    serde_json::from_value(json!({"version": "1.0.0", "base": "https://huggingface.co", "sets": sets}))
        .unwrap()
}

/// `ext`@0.3.0 installed as an extension that depends on `deps` (what the marketplace's install leaves in the lock,
/// and its package dir, so a later plan treats it as installed).
fn install_extension(data: &Path, lock: &mut QrtzLock, ext: &str, deps: &[&str]) {
    std::fs::create_dir_all(data.join(format!("packages/{ext}/0.3.0"))).unwrap();
    let mut l = lock.packages[&key(deps[0], "1.0.0")].clone();
    l.kind = PackageKind::Extension;
    let id = qrtz_modules::qrtz_json::identifier_for(ext, l.kind).unwrap();
    l.designated_requirement = Some(qrtz_modules::manifest::designated_requirement(&id, &l.team_id));
    l.identifier = Some(id);
    l.source = Source::Network;
    l.dependencies = deps.iter().map(|d| (d.to_string(), "1.0.0".to_owned())).collect();
    lock.packages.insert(key(ext, "0.3.0"), l);
    lock.workspace.dependencies.insert(ext.into(), "^0.3.0".into());
    lock.workspace.resolved.insert(ext.into(), "0.3.0".into());
}

fn lock(data: &Path) -> QrtzLock {
    QrtzLock::load(&data.join("qrtz.lock")).unwrap()
}

async fn adopted(srv: &Server, data: &Path) -> LocalModels {
    let models = LocalModels::with_manifest(data.join("models"), serve(srv), Some(srv.base.clone()));
    models
        .download(false, &DownloadControl::default(), &|_| {})
        .await
        .unwrap();
    let l = lock(data);
    for (_, package, _) in SETS {
        assert!(l.workspace.dependencies.contains_key(package), "{package}");
        assert_eq!(
            l.get(package, "1.0.0").unwrap().source,
            Source::Pinned,
            "{package}"
        );
    }
    models
}

fn has_view(data: &Path, dir: &str) -> bool {
    data.join(format!("models/{dir}/{MARKER}")).is_file()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upgrade_drops_old_app_requests_only_for_modules_that_arent_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path();
    let srv = Server::start().await;
    let models = adopted(&srv, data).await;

    // echo is installed from the marketplace; nano isn't.
    let mut l = lock(data);
    install_extension(data, &mut l, "echo", &[ECHO]);
    l.save(&data.join("qrtz.lock")).unwrap();
    let mut want: Vec<String> = NANO.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(orphaned_app_requests(&l), want);

    assert_eq!(models.sync_views(), Vec::<String>::new());
    let l = lock(data);
    for n in NANO {
        assert!(!l.workspace.dependencies.contains_key(n), "{n}: request dropped");
        assert!(l.get(n, "1.0.0").is_none(), "{n}: package left the lock");
        assert!(
            !data.join(format!("packages/{n}/1.0.0")).exists(),
            "{n}: package dir pruned"
        );
    }
    for (_, package, dir) in SETS {
        assert_eq!(has_view(data, dir), package == ECHO, "{dir}");
    }
    // echo's model: still requested, locked, on disk and linked for the helper.
    assert!(l.workspace.dependencies.contains_key(ECHO));
    assert!(l.get(ECHO, "1.0.0").is_some());
    assert!(data.join(format!("packages/{ECHO}/1.0.0/w.bin")).is_file());
    assert!(orphaned_app_requests(&l).is_empty(), "idempotent");
    assert_eq!(models.sync_views(), Vec::<String>::new());
    assert_eq!(lock(data), l, "a second launch changes nothing");

    // Removing echo later: its old app request goes too, and with nothing left the model is gone.
    let mut l = lock(data);
    l.packages.remove(&key("echo", "0.3.0"));
    l.workspace.dependencies.remove("echo");
    l.workspace.resolved.remove("echo");
    l.save(&data.join("qrtz.lock")).unwrap();
    assert_eq!(orphaned_app_requests(&l), vec![ECHO.to_owned()]);
    assert_eq!(models.sync_views(), Vec::<String>::new());
    assert!(lock(data).get(ECHO, "1.0.0").is_none());
    assert!(!has_view(data, "parakeet-tdt-v2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_installed_module_and_a_users_own_request_keep_their_models() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path();
    let srv = Server::start().await;
    let models = adopted(&srv, data).await;

    let mut l = lock(data);
    // nano installed but revoked: not installed, so its old requests may go...
    install_extension(data, &mut l, "nano", &NANO);
    l.packages.get_mut(&key("nano", "0.3.0")).unwrap().disabled = Some("revoked".into());
    // ...except one the user asked for from the index (`qrtz install model.localvqe`).
    l.packages
        .get_mut(&key("model.localvqe", "1.0.0"))
        .unwrap()
        .source = Source::Network;
    // echo installed with an engine swap onto nano's Nemotron (ADR 0157 §5): nano isn't installed, but an installed
    // extension depends on it, so that request stays.
    install_extension(data, &mut l, "echo", &[ECHO, "model.nemotron"]);
    l.save(&data.join("qrtz.lock")).unwrap();
    assert_eq!(
        orphaned_app_requests(&l),
        vec!["model.nemotron-diarizer".to_owned()]
    );

    assert_eq!(models.sync_views(), Vec::<String>::new());
    let l = lock(data);
    assert!(!l.workspace.dependencies.contains_key("model.nemotron-diarizer"));
    for n in [ECHO, "model.nemotron", "model.localvqe"] {
        assert!(l.workspace.dependencies.contains_key(n), "{n}: kept");
        assert!(data.join(format!("packages/{n}/1.0.0/w.bin")).is_file(), "{n}");
    }
    // The diarizer is still reachable through the revoked nano row, so its package stays locked (nothing dropped
    // that something in the lock needs); only the app's own request for it went.
    assert!(l.get("model.nemotron-diarizer", "1.0.0").is_some());

    // Nothing installed and no app requests: a fresh empty shell is a no-op.
    let tmp2 = tempfile::tempdir().unwrap();
    let empty = LocalModels::with_manifest(tmp2.path().join("models"), serve(&srv), Some(srv.base.clone()));
    assert_eq!(empty.sync_views(), Vec::<String>::new());
    assert!(orphaned_app_requests(&QrtzLock::default()).is_empty());
}
