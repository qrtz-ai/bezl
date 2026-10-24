//! ADR 0169 upgrade (Bobs and Bozniak, 902bcab review): a Mac upgrading to the empty shell has the app's old model
//! requests in `qrtz.lock` (echo's and nano's models, pinned by the app before modules were packages). At launch,
//! `LocalModels::sync_views` drops the ones whose modules aren't installed, with their views, packages and blobs. ADR
//! 0237: echo and nano are both built into qrtz.app now, so their requests are always kept.
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

/// A signed, empty index on its own loopback server and its root pin: nano declares it reads its three models in
/// its own process (ADR 0212), so like code they install only under a fresh signed list (ADR 0157 §6).
async fn index() -> (tempfile::TempDir, Server, String) {
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
    fn put(dir: &Path, base: &Path, srv: &Server) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                put(&p, base, srv);
            } else {
                srv.put(
                    &p.strip_prefix(base).unwrap().to_string_lossy(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    put(out.path(), out.path(), &srv);
    (out, srv, pin)
}

async fn adopted(srv: &Server, data: &Path) -> LocalModels {
    let (out, ix, pin) = index().await;
    let models = LocalModels::with_manifest(data.join("models"), serve(srv), Some(srv.base.clone()))
        .with_index(Some(pin), vec![ix.base.clone()]);
    models
        .download(false, &DownloadControl::default(), &|_| {})
        .await
        .unwrap();
    drop((out, ix));
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

/// ADR 0237: every module whose models the app pinned (echo and nano) is now built into qrtz.app, so none of those
/// requests is ever orphaned: a launch keeps each request, package and view, with or without old marketplace rows
/// for echo or nano in the lock (a revoked one included).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bundled_echo_and_nano_keep_every_app_request_at_an_upgrade() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path();
    let srv = Server::start().await;
    let models = adopted(&srv, data).await;
    let keeps_all = |l: &QrtzLock| {
        for (_, package, dir) in SETS {
            assert!(l.workspace.dependencies.contains_key(package), "{package}: kept");
            assert!(l.get(package, "1.0.0").is_some(), "{package}: locked");
            assert!(
                data.join(format!("packages/{package}/1.0.0/w.bin")).is_file(),
                "{package}"
            );
            assert!(has_view(data, dir), "{dir}: linked for the bundled helper");
        }
    };

    // Nothing installed from the marketplace.
    assert!(orphaned_app_requests(&lock(data)).is_empty());
    assert_eq!(models.sync_views(), Vec::<String>::new());
    keeps_all(&lock(data));

    // An old echo row, and an old nano row that was revoked: still nothing goes.
    let mut l = lock(data);
    install_extension(data, &mut l, "echo", &[ECHO]);
    install_extension(data, &mut l, "nano", &NANO);
    l.packages.get_mut(&key("nano", "0.3.0")).unwrap().disabled = Some("revoked".into());
    l.save(&data.join("qrtz.lock")).unwrap();
    assert!(orphaned_app_requests(&l).is_empty());
    assert_eq!(models.sync_views(), Vec::<String>::new());
    let l = lock(data);
    keeps_all(&l);
    assert_eq!(models.sync_views(), Vec::<String>::new());
    assert_eq!(lock(data), l, "a second launch changes nothing");

    // Nothing installed and no app requests: a fresh empty shell is a no-op.
    let tmp2 = tempfile::tempdir().unwrap();
    let empty = LocalModels::with_manifest(tmp2.path().join("models"), serve(&srv), Some(srv.base.clone()));
    assert_eq!(empty.sync_views(), Vec::<String>::new());
    assert!(orphaned_app_requests(&QrtzLock::default()).is_empty());
}
