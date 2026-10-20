#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Jared, 2026-10-18: the module manager's "shared" section listed `model.localvqe`, `model.nemotron-diarizer`,
//! `model.nemotron` and `model.parakeet-tdt-v2-en` as "used by nothing", and "clean up" then said there was nothing
//! to clean up. Both were right about different things: the four are the models the app pinned for echo and nano
//! (workspace requests in `qrtz.lock`, no extension row above them), which `prune` keeps, while the label read
//! only extension dependents. Now one answer (`QrtzLock::users`, with the app's `pinned_users`) feeds the label,
//! `qrtz ls` and `packages:list`'s `reclaimable`, the dry run of the same sweep `prune` runs.
//!
//! The Mac here is the upgrade case: the four sets were downloaded before the package manager (view dirs with
//! their `.qrtz-verified` markers, no `qrtz.lock`), then adopted through it. The files are tiny stand-ins served
//! from loopback under the real package names and dirs.
use qrtz_app::local_models::DownloadControl;
use qrtz_app::local_models::{LocalModels, Manifest, MARKER};
use qrtz_app::module_hosting::{HttpTransport, Packages};
use qrtz_app::packages_ipc::PackagesIpc;
use qrtz_app::ports::PackagesPort;
use qrtz_modules::index::PackageKind;
use qrtz_modules::install::Installer;
use qrtz_modules::qrtz_lock::{key, QrtzLock};
use qrtz_publish::dry_run::NoAppleChecks;
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

fn body(i: usize) -> Vec<u8> {
    (0..4096 + i * 512).map(|n| (n % 251) as u8 ^ i as u8).collect()
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

/// What the pre-package-manager downloader left: each set's files and its marker, nothing else.
fn legacy_views(models: &Path) {
    for (i, (_, _, dir)) in SETS.iter().enumerate() {
        let d = models.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("w.bin"), body(i)).unwrap();
        std::fs::write(d.join(MARKER), "1.0.0").unwrap();
    }
}

fn packages(data: &Path) -> Packages {
    Packages::with(
        Installer::new(data, None, Box::new(NoAppleChecks)),
        HttpTransport::new(&[], &[]),
    )
}

fn users(p: &Packages) -> Vec<(String, Vec<String>)> {
    p.list()
        .unwrap()
        .into_iter()
        .map(|l| (l.name, l.used_by))
        .collect()
}

fn expected() -> Vec<(String, Vec<String>)> {
    [
        // qrtz.lock's key order, the order of Jared's screen.
        ("model.localvqe", "nano"),
        ("model.nemotron-diarizer", "nano"),
        ("model.nemotron", "nano"),
        ("model.parakeet-tdt-v2-en", "echo"),
    ]
    .iter()
    .map(|(n, u)| (n.to_string(), vec![u.to_string()]))
    .collect()
}

/// ADR 0212: nano declares it reads its three models in its own process, so they have no view (it loads them from
/// the read-only store package); echo's Parakeet keeps its view.
fn on_disk(data: &Path) {
    for (_, package, dir) in SETS {
        assert!(
            data.join(format!("packages/{package}/1.0.0/w.bin")).is_file(),
            "{package}"
        );
        assert_eq!(
            data.join(format!("models/{dir}/{MARKER}")).is_file(),
            package == "model.parakeet-tdt-v2-en",
            "{dir}"
        );
    }
}

/// A signed, empty index on its own loopback server and its root pin: nano's declared reads install only under a
/// fresh signed list, like code (ADR 0157 §6, ADR 0212).
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_speech_models_echo_and_nano_use_are_labeled_with_them_and_never_pruned() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path();
    let srv = Server::start().await;
    let (_out, ix, pin) = index().await;
    let models = LocalModels::with_manifest(data.join("models"), serve(&srv), Some(srv.base.clone()))
        .with_index(Some(pin), vec![ix.base.clone()]);

    // Upgrade from before the package manager: the views are there, the lock isn't. nano's writable views no longer
    // count (ADR 0212: its process reads only the read-only store packages); echo's still does.
    legacy_views(&data.join("models"));
    assert!(!models.installed());
    assert!(!data.join("qrtz.lock").exists());
    // Switching echo and nano on (or settings → speech) adopts them through the package manager.
    models
        .download(false, &DownloadControl::default(), &|_| {})
        .await
        .unwrap();
    let lock = QrtzLock::load(&data.join("qrtz.lock")).unwrap();
    for (_, package, _) in SETS {
        assert_eq!(
            lock.workspace.resolved.get(package).map(String::as_str),
            Some("1.0.0")
        );
        assert!(
            lock.required_by(package, "1.0.0").iter().all(String::is_empty),
            "{package}: only the workspace asks for it, no extension row"
        );
    }

    let p = packages(data);
    assert_eq!(
        users(&p),
        expected(),
        "the label names echo and nano, not nothing"
    );
    let ipc = PackagesIpc::new(packages(data));
    let contract = qrtz_ipc::contract();
    let s = ipc.call("packages:list", Value::Null).await.unwrap();
    assert!(contract.channels["packages:list"].response.is_valid(&s), "{s}");
    for row in s["packages"].as_array().unwrap() {
        assert_ne!(row["usedBy"], json!([]), "{row}");
    }
    assert_eq!(
        s["reclaimable"],
        json!({"dirs": 0, "blobs": 0, "bytes": 0}),
        "no clean up offered"
    );
    let r = ipc.call("packages:prune", Value::Null).await.unwrap();
    assert_eq!(r, json!({"dirs": 0, "blobs": 0, "bytes": 0}));
    on_disk(data);
    assert!(models.installed());

    // Something really left over (an old version's dir and a blob nothing references): offered, and only that goes.
    std::fs::create_dir_all(data.join("packages/model.nemotron/0.9.0")).unwrap();
    std::fs::write(data.join("packages/model.nemotron/0.9.0/w.bin"), b"old").unwrap();
    let stray = b"stray blob".to_vec();
    let t = data.join("stray");
    std::fs::write(&t, &stray).unwrap();
    assert!(p
        .installer
        .store()
        .insert_file(&t, &sha(&stray), stray.len() as u64)
        .unwrap());
    let s = ipc.call("packages:list", Value::Null).await.unwrap();
    assert_eq!(
        s["reclaimable"],
        json!({"dirs": 1, "blobs": 1, "bytes": stray.len()})
    );
    let r = ipc.call("packages:prune", Value::Null).await.unwrap();
    assert_eq!(r, s["reclaimable"], "prune deletes exactly what was offered");
    assert!(!data.join("packages/model.nemotron/0.9.0").exists());
    on_disk(data);
    assert_eq!(users(&p), expected());
    let s = ipc.call("packages:list", Value::Null).await.unwrap();
    assert_eq!(s["reclaimable"], json!({"dirs": 0, "blobs": 0, "bytes": 0}));

    // ADR 0167 (empty shell): echo and nano arrive as extension packages that depend on these models. The label
    // reads the lock's extension rows then, and the adopted pins name the same modules, each once.
    let mut lock = QrtzLock::load(&data.join("qrtz.lock")).unwrap();
    for (ext, deps) in [
        ("echo", &["model.parakeet-tdt-v2-en"][..]),
        (
            "nano",
            &["model.nemotron", "model.nemotron-diarizer", "model.localvqe"][..],
        ),
    ] {
        let mut l = lock.packages[&key(deps[0], "1.0.0")].clone();
        l.kind = PackageKind::Extension;
        let id = qrtz_modules::qrtz_json::identifier_for(ext, l.kind).unwrap();
        l.designated_requirement = Some(qrtz_modules::manifest::designated_requirement(&id, &l.team_id));
        l.identifier = Some(id);
        l.source = qrtz_modules::qrtz_lock::Source::Network;
        l.dependencies = deps.iter().map(|d| (d.to_string(), "1.0.0".to_owned())).collect();
        lock.packages.insert(key(ext, "0.3.0"), l);
        lock.workspace.dependencies.insert(ext.into(), "^0.3.0".into());
        lock.workspace.resolved.insert(ext.into(), "0.3.0".into());
    }
    lock.save(&data.join("qrtz.lock")).unwrap();
    let models_only = |p: &Packages| -> Vec<(String, Vec<String>)> {
        users(p)
            .into_iter()
            .filter(|(n, _)| n.starts_with("model."))
            .collect()
    };
    assert_eq!(models_only(&p), expected());
    // And once the old pins are dropped, the extension rows alone still name them.
    for (_, package, _) in SETS {
        lock.workspace.dependencies.remove(package);
        lock.workspace.resolved.remove(package);
    }
    lock.save(&data.join("qrtz.lock")).unwrap();
    assert_eq!(models_only(&p), expected());
    for l in p
        .list()
        .unwrap()
        .into_iter()
        .filter(|l| l.name.starts_with("model."))
    {
        assert_eq!(l.requested, None, "{}", l.name);
    }
    let r = ipc.call("packages:prune", Value::Null).await.unwrap();
    assert_eq!(r, json!({"dirs": 0, "blobs": 0, "bytes": 0}));
    on_disk(data);
}
