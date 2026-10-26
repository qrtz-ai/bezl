//! QRT-367: echo's opt-in Phonon-2.5 model (gated). Not in echo's required set and never auto-downloaded; nothing is
//! fetched until the setting is on; a manifest with a path-traversal path or over the size cap is refused before any
//! network call; the download is sha256-checked into a read-only package; and the helper gets the path only after
//! every installed file re-hashed to its pin (hash before use). Off deletes it. Served on loopback.
use qrtz_app::echo_phonon_model::{validate, EchoPhononModel, MANIFEST, MAX_TOTAL_BYTES, PACKAGE};
use qrtz_app::local_models::Manifest;
use qrtz_speech::download::DownloadControl;
use qrtz_speech::testkit::{sha, Server};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const REV: &str = "948fac942444637c2549e40246844cea49b6ad1c";
const REPO: &str = "Kayetic/Phonon-2.5-English-CoreML";

fn pinned() -> Manifest {
    serde_json::from_str(MANIFEST).unwrap()
}

fn body() -> Vec<u8> {
    (0..200_000).map(|i| (i % 241) as u8).collect()
}

fn manifest(srv: &Server) -> Manifest {
    let b = body();
    let rev = "d".repeat(40);
    let remote = "Phonon-2.5.mlpackage/Data/com.apple.CoreML/model.mlmodel";
    srv.put(&format!("org/phonon/resolve/{rev}/{remote}"), b.clone());
    serde_json::from_value(json!({
        "version": "1.0.0", "base": "https://example.invalid",
        "sets": [{"id": "phonon25-en", "package": PACKAGE, "task": "asr", "dir": "phonon25-en", "repo": "org/phonon",
          "revision": rev, "files": [{"remote": remote, "path": remote, "size": b.len(), "sha256": sha(&b)}]}]
    }))
    .unwrap()
}

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

fn hits(srv: &Server) -> usize {
    srv.faults.lock().unwrap().hits.values().sum()
}

fn package(d: &Path) -> PathBuf {
    d.join(format!("packages/{PACKAGE}/1.0.0"))
}

fn cleanup(d: tempfile::TempDir) {
    let _ = qrtz_modules::fsutil::remove_tree(d.path());
}

#[test]
fn the_pinned_manifest_is_phonon_at_948fac94_under_the_cap() {
    let m = pinned();
    validate(&m).unwrap();
    let set = &m.sets[0];
    assert_eq!(
        (set.package.as_str(), set.repo.as_str(), set.revision.as_str()),
        (PACKAGE, REPO, REV)
    );
    assert_eq!(set.files.len(), 10);
    assert!(
        set.files[0].path.starts_with("Phonon-2.5.mlpackage/"),
        "the entry is the mlpackage"
    );
    let w = set.files.iter().find(|f| f.path.ends_with("weight.bin")).unwrap();
    assert_eq!(
        w.sha256,
        "71f69ad6f1a77781ff024c52755482a0aa7eb4bf63df50f34568f1affe75be57"
    );
    assert_eq!(m.total_size(), 423_822_752);
    assert!(m.total_size() <= MAX_TOTAL_BYTES);
}

/// Not echo's required set: speech/onboarding downloads for echo never include it.
#[test]
fn it_is_never_in_echos_required_downloads() {
    assert!(!qrtz_app::local_models::packages_needed_by("echo").contains(&PACKAGE.to_owned()));
    assert!(Manifest::pinned().sets.iter().all(|s| s.package != PACKAGE));
    let d = tempfile::tempdir().unwrap();
    let lm =
        qrtz_app::local_models::LocalModels::new(d.path().join("models"), None).for_extensions(&["echo"]);
    assert!(lm.packages().iter().all(|p| p.name != PACKAGE));
    // echo's process may read it (scoped), and nothing else as a package.
    assert!(qrtz_modules::bundled_read_allowed("echo", PACKAGE));
    assert!(!qrtz_modules::bundled_read_allowed(
        "echo",
        "model.parakeet-tdt-v2-en"
    ));
    assert!(!qrtz_modules::bundled_read_allowed("echo", "model.laya"));
    let reads = qrtz_modules::bundled::declared_reads();
    let echo = reads.iter().find(|(e, _)| e == "echo").map(|(_, r)| r.clone());
    assert_eq!(echo, Some(vec![PACKAGE.to_owned()]));
    // Bundled echo still gets no QRTZ_READ_MODELS (its Parakeet set stays reachable).
    assert_eq!(qrtz_app::installed::read_models_env(d.path(), "echo"), None);
}

#[test]
fn a_path_traversal_or_oversized_manifest_is_refused() {
    let base = pinned();
    let bad_paths = [
        "../x",
        "a/../../x",
        "/etc/passwd",
        "a//b",
        "./a",
        "a\\b",
        "",
        "a/.",
    ];
    for p in bad_paths {
        for field in ["path", "remote"] {
            let mut m = base.clone();
            match field {
                "path" => m.sets[0].files[0].path = p.into(),
                _ => m.sets[0].files[0].remote = p.into(),
            }
            assert_eq!(validate(&m).unwrap_err(), "phonon.manifest_path", "{field}={p:?}");
        }
    }
    let mut m = base.clone();
    m.sets[0].dir = "../echo".into();
    assert_eq!(validate(&m).unwrap_err(), "phonon.manifest_path");
    let mut m = base.clone();
    m.sets[0].files[1].size = MAX_TOTAL_BYTES;
    assert_eq!(validate(&m).unwrap_err(), "phonon.too_large");
    let mut m = base.clone();
    m.sets[0].files[0].size = u64::MAX;
    assert_eq!(validate(&m).unwrap_err(), "phonon.too_large");
    let mut m = base.clone();
    m.sets[0].files[0].sha256 = "A".repeat(64);
    assert_eq!(validate(&m).unwrap_err(), "phonon.manifest_sha256");
    let mut m = base.clone();
    m.sets[0].files[1].path = m.sets[0].files[0].path.clone();
    assert_eq!(validate(&m).unwrap_err(), "phonon.manifest_duplicate");
    let mut m = base.clone();
    m.sets[0].package = "model.laya".into();
    assert_eq!(validate(&m).unwrap_err(), "phonon.manifest_package");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_manifest_or_the_setting_off_never_touches_the_network() {
    let srv = Server::start().await;
    let d = tempfile::tempdir().unwrap();
    let mut m = manifest(&srv);
    m.sets[0].files[0].path = "../../escape".into();
    let model = EchoPhononModel::with_manifest(
        d.path().join("models/echo"),
        &d.path().join("echo"),
        m,
        Some(srv.base.clone()),
    );
    model.set_enabled(true, 1).unwrap();
    let e = model
        .download(&DownloadControl::default(), &|_| {})
        .await
        .unwrap_err();
    assert!(e.to_string().contains("phonon.manifest_path"), "{e}");
    assert_eq!(model.state().state, "refused");
    let model = EchoPhononModel::with_manifest(
        d.path().join("models/echo"),
        &d.path().join("echo"),
        manifest(&srv),
        Some(srv.base.clone()),
    );
    model.set_enabled(false, 2).unwrap();
    let e = model
        .download(&DownloadControl::default(), &|_| {})
        .await
        .unwrap_err();
    assert!(e.to_string().contains("phonon.off"), "{e}");
    // No answer at all is no.
    std::fs::remove_file(d.path().join("echo/phonon.json")).unwrap();
    assert!(!model.enabled());
    assert_eq!(model.state().state, "off");
    assert_eq!(hits(&srv), 0, "nothing requested");
    cleanup(d);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_download_is_verified_and_hashed_again_before_use() {
    let srv = Server::start().await;
    let idx = index().await;
    let d = tempfile::tempdir().unwrap();
    let mk = || {
        EchoPhononModel::with_manifest(
            d.path().join("models/echo"),
            &d.path().join("echo"),
            manifest(&srv),
            Some(srv.base.clone()),
        )
        .with_index(Some(idx.pin.clone()), vec![idx.srv.base.clone()])
    };
    let m = mk();
    m.set_enabled(true, 1).unwrap();
    assert_eq!(m.state().state, "not_installed");
    // Bytes that don't hash to the pin: refused, nothing installed, nothing handed to the helper.
    let route = srv.routes.lock().unwrap().keys().next().unwrap().clone();
    srv.faults.lock().unwrap().corrupt.insert(route);
    assert!(m.download(&DownloadControl::default(), &|_| {}).await.is_err());
    assert!(!m.installed());
    assert_eq!(m.ready_path(), None);
    srv.faults.lock().unwrap().corrupt.clear();
    m.download(&DownloadControl::default(), &|_| {}).await.unwrap();
    let entry = package(d.path()).join("Phonon-2.5.mlpackage");
    assert_eq!(m.ready_path().as_deref(), Some(entry.as_path()));
    assert_eq!(m.state().state, "installed");
    let f = entry.join("Data/com.apple.CoreML/model.mlmodel");
    let ro = std::fs::metadata(&f).unwrap().permissions();
    assert_eq!(ro.mode() & 0o222, 0, "read-only");
    // Same size and mode, different bytes: a fresh start re-hashes before first use and refuses it.
    let mut bytes = std::fs::read(&f).unwrap();
    bytes[0] ^= 0xff;
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&f, &bytes).unwrap();
    std::fs::set_permissions(&f, ro.clone()).unwrap();
    assert!(mk().installed(), "sizes and modes still match");
    assert_eq!(mk().ready_path(), None, "hash before use");
    bytes[0] ^= 0xff;
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&f, &bytes).unwrap();
    assert_eq!(mk().ready_path(), None, "writable: refused");
    std::fs::set_permissions(&f, ro).unwrap();
    assert_eq!(mk().ready_path().as_deref(), Some(entry.as_path()));
    // Off deletes it.
    m.set_enabled(false, 2).unwrap();
    assert!(!package(d.path()).exists());
    assert_eq!(m.ready_path(), None);
    cleanup(d);
}
