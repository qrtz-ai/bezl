//! ADR 0079: the Mac helper's pinned models through onboarding/settings ("speech"): status, download with progress,
//! pause and resume from the `.part` files, a damaged file refused and retried, the `.qrtz-verified` markers the
//! helper waits for, delete. Served by the qrtz-speech test kit (loopback), with a small manifest of the same shape.
use qrtz_app::local_models::{LocalModels, Manifest, MARKER};
use qrtz_app::service::Events;
use qrtz_app::setup::SpeechManager;
use qrtz_speech::testkit::{sha, Server};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const BIG: usize = 2 * 1024 * 1024;

fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| ((i % 251) as u8) ^ seed).collect()
}

/// Two sets like the real ones (a model bundle dir and a loose file), served at `<repo>/resolve/<rev>/<remote>`.
fn serve(srv: &Server) -> Manifest {
    let files = [
        (
            "org/asr",
            "a".repeat(40),
            "tier/enc.mlmodelc/weights/weight.bin",
            "enc.mlmodelc/weights/weight.bin",
            bytes(BIG, 1),
        ),
        (
            "org/asr",
            "a".repeat(40),
            "tier/tokenizer.json",
            "tokenizer.json",
            b"{}".to_vec(),
        ),
        (
            "org/diar",
            "b".repeat(40),
            "v2/diar.mlmodelc/coremldata.bin",
            "diar.mlmodelc/coremldata.bin",
            bytes(5000, 2),
        ),
    ];
    let mut sets: Vec<Value> = vec![];
    for (repo, rev, remote, path, body) in files {
        srv.put(&format!("{repo}/resolve/{rev}/{remote}"), body.clone());
        let f = json!({"remote": remote, "path": path, "size": body.len(), "sha256": sha(&body)});
        match sets.iter_mut().find(|s| s["repo"] == repo) {
            Some(s) => s["files"].as_array_mut().unwrap().push(f),
            None => sets.push(json!({"id": repo, "dir": repo.replace("org/", ""), "repo": repo, "revision": rev, "files": [f]})),
        }
    }
    serde_json::from_value(json!({"version": "1.0.0", "base": "https://example.invalid", "sets": sets}))
        .unwrap()
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..1200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_helper_models_download_pause_resume_verify_and_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = Server::start().await;
    let m = serve(&srv);
    let total = m.total_size();
    let root = tmp.path().join("models");
    let big = format!(
        "org/asr/resolve/{}/tier/enc.mlmodelc/weights/weight.bin",
        "a".repeat(40)
    );
    srv.faults
        .lock()
        .unwrap()
        .throttle
        .insert(big.clone(), Duration::from_millis(5));
    let mgr = SpeechManager::with_local(
        None,
        Some(LocalModels::with_manifest(&root, m, Some(srv.base.clone()))),
        Events::new(),
    );

    let s = mgr.status();
    assert_eq!(s["configured"], true);
    assert_eq!(s["installed"], Value::Null);
    assert_eq!(s["latest"], "1.0.0");
    assert_eq!(s["latestBytes"], total);
    // No request: the pinned models have no "newer release" to check for.
    assert_eq!(mgr.check().await.unwrap()["latestBytes"], total);
    assert!(srv.faults.lock().unwrap().hits.is_empty());

    mgr.download(false).unwrap();
    until("progress into the big file", || {
        mgr.status()["download"]["done"].as_u64().unwrap() > 300 * 1024
    })
    .await;
    let s = mgr.pause();
    assert_eq!(s["download"]["state"], "paused");
    until("paused", || mgr.status()["download"]["state"] == "paused").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let part = root.join("asr/enc.mlmodelc/weights/weight.bin.part");
    let have = std::fs::metadata(&part).unwrap().len();
    assert!(have > 0 && (have as usize) < BIG, "{have}");
    assert!(
        !root.join("asr").join(MARKER).exists(),
        "no marker before every file is verified"
    );

    srv.faults.lock().unwrap().throttle.remove(&big);
    mgr.resume().unwrap();
    until("installed", || mgr.status()["installed"] == "1.0.0").await;
    let s = mgr.status();
    assert_eq!(s["download"]["state"], "idle");
    assert!(s["bytes"].as_u64().unwrap() >= total);
    // Resumed from the .part, not from scratch.
    assert!(srv
        .faults
        .lock()
        .unwrap()
        .ranges
        .iter()
        .any(|(p, r)| p == &big && r == &format!("bytes={have}-")));
    for set in ["asr", "diar"] {
        assert_eq!(
            std::fs::read_to_string(root.join(set).join(MARKER)).unwrap(),
            "1.0.0"
        );
        assert_eq!(mode(&root.join(set)), 0o700);
    }
    assert_eq!(std::fs::read(root.join("asr/tokenizer.json")).unwrap(), b"{}");
    assert!(!part.exists());

    let s = mgr.delete().unwrap();
    assert_eq!(s["installed"], Value::Null);
    assert!(!root.join("asr").exists() && !root.join("diar").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_damaged_model_file_is_refused_and_a_retry_fetches_it_again() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = Server::start().await;
    let m = serve(&srv);
    let root = tmp.path().join("models");
    let diar = format!(
        "org/diar/resolve/{}/v2/diar.mlmodelc/coremldata.bin",
        "b".repeat(40)
    );
    srv.faults.lock().unwrap().corrupt.insert(diar.clone());
    let mgr: Arc<SpeechManager> = SpeechManager::with_local(
        None,
        Some(LocalModels::with_manifest(&root, m, Some(srv.base.clone()))),
        Events::new(),
    );
    mgr.download(false).unwrap();
    until("error", || mgr.status()["download"]["state"] == "error").await;
    let s = mgr.status();
    assert_eq!(s["download"]["errorKind"], "checksum", "{s}");
    assert_eq!(s["installed"], Value::Null);
    assert!(
        root.join("asr").join(MARKER).exists(),
        "the first set verified and is kept"
    );
    assert!(!root.join("diar").join(MARKER).exists());
    assert!(
        !root.join("diar/diar.mlmodelc/coremldata.bin").exists(),
        "a damaged file never lands under its name"
    );

    srv.faults.lock().unwrap().corrupt.remove(&diar);
    let asr_hits = srv.faults.lock().unwrap().hits.values().sum::<usize>();
    mgr.download(false).unwrap();
    until("installed", || mgr.status()["installed"] == "1.0.0").await;
    assert_eq!(
        srv.faults.lock().unwrap().hits.values().sum::<usize>(),
        asr_hits + 1,
        "only the damaged file again"
    );
}

#[test]
fn a_module_source_takes_precedence_over_local_models() {
    // With a speech module configured the local models are not offered (one speech source at a time).
    let tmp = tempfile::tempdir().unwrap();
    let mgr = SpeechManager::with_local(None, None, Events::new());
    assert_eq!(mgr.status()["configured"], false);
    let local = SpeechManager::with_local(None, Some(LocalModels::new(tmp.path(), None)), Events::new());
    assert_eq!(local.status()["configured"], true);
    assert_eq!(local.status()["latestBytes"], Manifest::pinned().total_size());
}

/// The real pinned files from the model host (about 845 MB): `QRTZ_REAL_MODELS_DIR=/tmp/m cargo test -p qrtz-app
/// --test local_models -- --ignored`. Checks the pins, redirects and range resumes against the live host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn the_pinned_models_download_from_the_real_host() {
    let Ok(dir) = std::env::var("QRTZ_REAL_MODELS_DIR") else {
        return;
    };
    let l = LocalModels::new(&dir, None);
    let ctl = qrtz_speech::download::DownloadControl::default();
    l.download(false, &ctl, &|_| {}).await.unwrap();
    assert!(l.installed());
}
