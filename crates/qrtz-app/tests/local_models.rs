//! ADR 0079 / ADR 0157: the Mac helper's pinned models through settings ("speech"), installed by the
//! package manager: status, download with progress, pause and resume, a damaged file refused and retried (only it),
//! the `.qrtz-verified` markers the helper waits for, the `model.*` packages in `qrtz.lock`, delete and prune.
//! Served by the qrtz-speech test kit (loopback, playing Hugging Face), with a small manifest of the same shape.
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
            None => {
                let dir = repo.replace("org/", "");
                sets.push(
                    json!({"id": repo, "package": format!("model.{dir}"), "task": "asr", "dir": dir,
                                 "repo": repo, "revision": rev, "files": [f]}),
                )
            }
        }
    }
    serde_json::from_value(json!({"version": "1.0.0", "base": "https://huggingface.co", "sets": sets}))
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

    let warmed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = warmed.clone();
    mgr.on_installed(Arc::new(move || {
        w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    mgr.download(false).unwrap();
    // Progress moves per finished range; wait for the big file's range to be under way.
    until("the big file under way", || {
        srv.faults.lock().unwrap().hits.contains_key(&big)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let s = mgr.pause();
    assert_eq!(s["download"]["state"], "paused");
    until("paused", || mgr.status()["download"]["state"] == "paused").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !root.join("asr").join(MARKER).exists(),
        "no marker before every file is verified"
    );
    let data = tmp.path();
    assert!(
        !data.join("qrtz.lock").exists(),
        "nothing is installed before every blob is in"
    );

    srv.faults.lock().unwrap().throttle.remove(&big);
    mgr.resume().unwrap();
    until("installed", || mgr.status()["installed"] == "1.0.0").await;
    let s = mgr.status();
    assert_eq!(s["download"]["state"], "idle");
    // N121: the install hook (wiring: `models.warm`) ran once, after the pause and resume, not before.
    until("install hook", || {
        warmed.load(std::sync::atomic::Ordering::SeqCst) == 1
    })
    .await;
    assert!(s["bytes"].as_u64().unwrap() >= total);
    // Closed ranges (the big file is one 2 MiB range: asked again after the pause).
    let ranges = srv.faults.lock().unwrap().ranges.clone();
    assert!(
        ranges
            .iter()
            .filter(|(p, r)| p == &big && r == "bytes=0-2097151")
            .count()
            >= 2,
        "{ranges:?}"
    );
    for set in ["asr", "diar"] {
        assert_eq!(
            std::fs::read_to_string(root.join(set).join(MARKER)).unwrap(),
            "1.0.0"
        );
        assert_eq!(mode(&root.join(set)), 0o700);
    }
    assert_eq!(std::fs::read(root.join("asr/tokenizer.json")).unwrap(), b"{}");
    // They are model packages now: in the lock, read-only under packages/, from the store.
    let lock = std::fs::read_to_string(data.join("qrtz.lock")).unwrap();
    assert!(
        lock.contains("\"model.asr@1.0.0\"") && lock.contains("\"source\": \"pinned\""),
        "{lock}"
    );
    assert!(data.join("packages/model.asr/1.0.0/tokenizer.json").exists());
    assert!(
        std::fs::read_dir(data.join("store/.tmp"))
            .unwrap()
            .next()
            .is_none(),
        "no parts left"
    );

    let s = mgr.delete().unwrap();
    assert_eq!(s["installed"], Value::Null);
    assert!(!root.join("asr").exists() && !root.join("diar").exists());
    assert!(
        !data.join("packages/model.asr").exists(),
        "pruned: nothing else needs it"
    );
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
    // The whole resolved set swaps at once: nothing is marked, but the verified blobs stay in the store.
    assert!(!root.join("asr").join(MARKER).exists());
    assert!(!root.join("diar").join(MARKER).exists());
    assert!(tmp.path().join("store").join(sha(&bytes(BIG, 1))).exists());
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

/// The real pinned files from the model host (about 1.3 GB): `QRTZ_REAL_MODELS_DIR=/tmp/m cargo test -p qrtz-app
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

/// ADR 0175 (N181): echo's view reaches only echo's models. A pinned set for echo (Parakeet's dir) and one for nano
/// (Nemotron's): a download, pause or resume for one module is refused for the other, `check` answers with that
/// module's status only, and a scoped delete removes echo's set and keeps the one switched-on nano needs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn speech_calls_scoped_to_one_module_touch_only_its_models() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = Server::start().await;
    let mut sets: Vec<Value> = vec![];
    for (dir, body) in [
        ("parakeet-tdt-v2", bytes(BIG, 3)),
        ("nemotron-560ms", bytes(BIG, 4)),
    ] {
        let repo = format!("org/{dir}");
        let rev = "c".repeat(40);
        srv.put(&format!("{repo}/resolve/{rev}/w.bin"), body.clone());
        sets.push(
            json!({"id": dir, "package": format!("model.{dir}"), "task": "asr", "dir": dir,
            "repo": repo, "revision": rev,
            "files": [{"remote": "w.bin", "path": "w.bin", "size": body.len(), "sha256": sha(&body)}]}),
        );
    }
    let m: Manifest =
        serde_json::from_value(json!({"version": "1.0.0", "base": "https://huggingface.co", "sets": sets}))
            .unwrap();
    let root = tmp.path().join("models");
    let mgr = SpeechManager::with_local(
        None,
        Some(LocalModels::with_manifest(&root, m, Some(srv.base.clone()))),
        Events::new(),
    );
    mgr.select_extensions(Arc::new(|| vec!["echo".to_string(), "nano".to_string()]));
    let echo = ["echo".to_string()];
    let nano = ["nano".to_string()];

    // check: echo's own status (one set's bytes), no request.
    let c = mgr.check_for(Some(&echo)).await.unwrap();
    assert_eq!(c["latestBytes"], BIG as u64);
    assert_eq!(mgr.check_for(None).await.unwrap()["latestBytes"], 2 * BIG as u64);

    // A throttled download for nano: echo's scope can neither pause nor resume it.
    let nemo = format!("org/nemotron-560ms/resolve/{}/w.bin", "c".repeat(40));
    srv.faults
        .lock()
        .unwrap()
        .throttle
        .insert(nemo.clone(), Duration::from_millis(5));
    mgr.download_for(false, Some(nano.to_vec())).unwrap();
    until("nano's download under way", || {
        srv.faults.lock().unwrap().hits.contains_key(&nemo)
    })
    .await;
    let e = mgr.pause_for(Some(&echo)).unwrap_err();
    assert_eq!(e.code, qrtz_ipc::ErrorCode::Validation);
    assert_ne!(mgr.status()["download"]["state"], "paused");
    mgr.pause_for(Some(&nano)).unwrap();
    until("paused", || mgr.status()["download"]["state"] == "paused").await;
    assert!(mgr.resume_for(Some(&echo)).is_err());
    assert_eq!(mgr.status()["download"]["state"], "paused");
    srv.faults.lock().unwrap().throttle.remove(&nemo);
    mgr.resume_for(Some(&nano)).unwrap();
    until("nano installed", || {
        mgr.status_for(Some(&nano))["installed"] == "1.0.0"
    })
    .await;
    assert_eq!(mgr.status_for(Some(&echo))["installed"], Value::Null);
    assert!(!root.join("parakeet-tdt-v2").exists(), "only nano's set came");

    // echo's own download, then echo's delete: Parakeet goes, Nemotron (nano is on) stays.
    mgr.download_for(false, Some(echo.to_vec())).unwrap();
    until("echo installed", || {
        mgr.status_for(Some(&echo))["installed"] == "1.0.0" && mgr.status()["download"]["state"] == "idle"
    })
    .await;
    // No sleep before the delete (cycle 69): a download that shows idle has ended for delete too (ADR 0209).
    let s = mgr.delete_for(Some(&echo)).unwrap();
    assert_eq!(s["installed"], Value::Null);
    assert!(!root.join("parakeet-tdt-v2").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("nemotron-560ms").join(MARKER)).unwrap(),
        "1.0.0"
    );
    assert_eq!(mgr.status_for(Some(&nano))["installed"], "1.0.0");
}

/// Cycle 69 (backlog P1): `until` with its own bound and the manager's state in the failure.
async fn within(mgr: &SpeechManager, limit: Duration, what: &str, f: impl Fn() -> bool) {
    let end = std::time::Instant::now() + limit;
    while std::time::Instant::now() < end {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("not within {limit:?}: {what}; status {}", mgr.status());
}

/// Echo's set (one small file) and nano's (a 2 MiB file and a 32 KiB one), and the two paths nano's come from.
fn two_modules(srv: &Server, tmp: &Path) -> (Arc<SpeechManager>, String, String) {
    let rev = "d".repeat(40);
    let mut sets: Vec<Value> = vec![];
    for (dir, files) in [
        ("parakeet-tdt-v2", vec![("w.bin", bytes(64 * 1024, 5))]),
        (
            "nemotron-560ms",
            vec![("a.bin", bytes(BIG, 6)), ("b.bin", bytes(32 * 1024, 7))],
        ),
    ] {
        let repo = format!("org/{dir}");
        let mut fs: Vec<Value> = vec![];
        for (name, body) in files {
            srv.put(&format!("{repo}/resolve/{rev}/{name}"), body.clone());
            fs.push(json!({"remote": name, "path": name, "size": body.len(), "sha256": sha(&body)}));
        }
        sets.push(
            json!({"id": dir, "package": format!("model.{dir}"), "task": "asr", "dir": dir,
            "repo": repo, "revision": rev, "files": fs}),
        );
    }
    let m: Manifest =
        serde_json::from_value(json!({"version": "1.0.0", "base": "https://huggingface.co", "sets": sets}))
            .unwrap();
    let mgr = SpeechManager::with_local(
        None,
        Some(LocalModels::with_manifest(
            tmp.join("models"),
            m,
            Some(srv.base.clone()),
        )),
        Events::new(),
    );
    mgr.select_extensions(Arc::new(|| vec!["echo".to_string(), "nano".to_string()]));
    let path = |f: &str| format!("org/nemotron-560ms/resolve/{rev}/{f}");
    (mgr, path("a.bin"), path("b.bin"))
}

/// Backlog P1, the root cause of the `speech_calls_scoped_to_one_module_touch_only_its_models` timeout (6 of 20
/// runs hung at "nano installed" before cycle 69): a resume that arrives after the running download saw the pause
/// but before that download ended was lost (`download_for` only cleared the pause flag while `running`), so the
/// download ended paused (or as a "network" error) and nothing ran again. Made deterministic: nano's small file
/// arrives a 16 KiB chunk per second, so its download is still running long after the big file stopped for the
/// pause; the resume lands in that window every time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_while_the_paused_download_winds_down_is_not_lost() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = Server::start().await;
    let (mgr, a, b) = two_modules(&srv, tmp.path());
    let nano = ["nano".to_string()];
    {
        let mut f = srv.faults.lock().unwrap();
        f.throttle.insert(a.clone(), Duration::from_millis(5));
        f.throttle.insert(b.clone(), Duration::from_millis(1000));
    }
    mgr.download_for(false, Some(nano.to_vec())).unwrap();
    within(&mgr, Duration::from_secs(10), "both files under way", || {
        let f = srv.faults.lock().unwrap();
        f.hits.contains_key(&a) && f.hits.contains_key(&b)
    })
    .await;
    mgr.pause_for(Some(&nano)).unwrap();
    // The big file sees the pause at its next 5 ms chunk; the small one's next chunk is ~1 s away.
    tokio::time::sleep(Duration::from_millis(150)).await;
    srv.faults.lock().unwrap().throttle.clear();
    let s = mgr.resume_for(Some(&nano)).unwrap();
    within(
        &mgr,
        Duration::from_secs(10),
        "nano installed after the resume",
        || mgr.status_for(Some(&nano))["installed"] == "1.0.0",
    )
    .await;
    // While it waited for the running attempt, the row said it was downloading again, not "paused".
    assert_eq!(s["download"]["state"], "downloading", "{s}");
    within(&mgr, Duration::from_secs(5), "idle", || {
        mgr.status()["download"]["state"] == "idle"
    })
    .await;
    assert!(
        !tmp.path().join("models/download.json").exists(),
        "no resume marker left"
    );
}

/// Backlog P1, the same root cause on the second leg of that test: a download asked for while another one runs
/// (echo's while nano's is still finishing) was dropped (`download_for` neither recorded nor started it), so echo's
/// models never came. It runs once the running one ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_download_asked_while_another_runs_runs_after_it() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = Server::start().await;
    let (mgr, _a, b) = two_modules(&srv, tmp.path());
    let (echo, nano) = (["echo".to_string()], ["nano".to_string()]);
    srv.faults
        .lock()
        .unwrap()
        .throttle
        .insert(b.clone(), Duration::from_millis(500));
    mgr.download_for(false, Some(nano.to_vec())).unwrap();
    within(&mgr, Duration::from_secs(10), "nano under way", || {
        srv.faults.lock().unwrap().hits.contains_key(&b)
    })
    .await;
    mgr.download_for(false, Some(echo.to_vec())).unwrap();
    within(&mgr, Duration::from_secs(10), "echo and nano installed", || {
        mgr.status_for(Some(&echo))["installed"] == "1.0.0"
            && mgr.status_for(Some(&nano))["installed"] == "1.0.0"
    })
    .await;
    within(&mgr, Duration::from_secs(5), "idle", || {
        mgr.status()["download"]["state"] == "idle"
    })
    .await;
    // Once idle, a scoped delete is accepted at once (no "pause the download first" from a run still ending).
    mgr.delete_for(Some(&echo)).unwrap();
    assert!(!tmp.path().join("models/parakeet-tdt-v2").exists());
}

/// A pause after a resume that is still waiting for the running download to end stands: nothing runs, the state
/// and the marker say paused, and the next resume finishes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pause_after_a_waiting_resume_stands() {
    let tmp = tempfile::tempdir().unwrap();
    let srv = Server::start().await;
    let (mgr, a, b) = two_modules(&srv, tmp.path());
    let nano = ["nano".to_string()];
    {
        let mut f = srv.faults.lock().unwrap();
        f.throttle.insert(a.clone(), Duration::from_millis(5));
        f.throttle.insert(b.clone(), Duration::from_millis(1000));
    }
    mgr.download_for(false, Some(nano.to_vec())).unwrap();
    within(&mgr, Duration::from_secs(10), "both files under way", || {
        let f = srv.faults.lock().unwrap();
        f.hits.contains_key(&a) && f.hits.contains_key(&b)
    })
    .await;
    mgr.pause_for(Some(&nano)).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    mgr.resume_for(Some(&nano)).unwrap();
    mgr.pause_for(Some(&nano)).unwrap();
    // The small file's next chunk (≤ 1 s) ends the attempt; it stays paused.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let s = mgr.status();
    assert_eq!(s["download"]["state"], "paused", "{s}");
    assert_eq!(mgr.status_for(Some(&nano))["installed"], Value::Null);
    let marker = std::fs::read_to_string(tmp.path().join("models/download.json")).unwrap();
    assert!(marker.contains("\"paused\":true"), "{marker}");
    srv.faults.lock().unwrap().throttle.clear();
    mgr.resume_for(Some(&nano)).unwrap();
    within(&mgr, Duration::from_secs(10), "nano installed", || {
        mgr.status_for(Some(&nano))["installed"] == "1.0.0"
    })
    .await;
}

// ---- Backlog P3 (cycle 70, ADR 0210): an installed and verified Parakeet never touches or waits on the network ----

/// A loopback "network" that accepts connections and never answers (a captive portal, a dead route): every
/// connection is counted and held open. A request to it hangs until the downloader's read timeout (60 s).
struct BlackHole {
    base: String,
    accepted: Arc<std::sync::atomic::AtomicUsize>,
}

async fn black_hole() -> BlackHole {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n = accepted.clone();
    tokio::spawn(async move {
        let mut held = vec![];
        while let Ok((s, _)) = l.accept().await {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            held.push(s);
        }
    });
    BlackHole { base, accepted }
}

impl BlackHole {
    fn hits(&self) -> usize {
        self.accepted.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Echo's set (Parakeet's dir) and nano's, served from loopback; returns the manifest.
fn echo_and_nano(srv: &Server) -> Manifest {
    let rev = "e".repeat(40);
    let mut sets: Vec<Value> = vec![];
    for (dir, body) in [
        ("parakeet-tdt-v2", bytes(64 * 1024, 8)),
        ("nemotron-560ms", bytes(32 * 1024, 9)),
    ] {
        let repo = format!("org/{dir}");
        srv.put(&format!("{repo}/resolve/{rev}/w.bin"), body.clone());
        sets.push(
            json!({"id": dir, "package": format!("model.{dir}"), "task": "asr", "dir": dir,
            "repo": repo, "revision": rev,
            "files": [{"remote": "w.bin", "path": "w.bin", "size": body.len(), "sha256": sha(&body)}]}),
        );
    }
    serde_json::from_value(json!({"version": "1.0.0", "base": "https://huggingface.co", "sets": sets}))
        .unwrap()
}

/// Parakeet downloaded and verified from the loopback host; then the same data dir with the network down: the
/// model host and the package hosts are a black hole.
async fn installed_parakeet_offline(tmp: &Path) -> (Manifest, BlackHole) {
    let srv = Server::start().await;
    let m = echo_and_nano(&srv);
    let root = tmp.join("models");
    let echo = LocalModels::with_manifest(&root, m.clone(), Some(srv.base.clone())).for_extensions(&["echo"]);
    let ctl = qrtz_speech::download::DownloadControl::default();
    echo.download(false, &ctl, &|_| {}).await.unwrap();
    assert!(echo.installed());
    drop(srv);
    (m, black_hole().await)
}

/// An index refresh stuck on a dead network (the qrtz page's refresh, "check for updates", `qrtz update` on a
/// captive portal): it holds the package manager's install lock while it waits up to the read timeout.
async fn refresh_stuck_on(tmp: &Path, net: &BlackHole) {
    let installer = qrtz_modules::install::Installer::new(
        tmp,
        Some("0".repeat(64)),
        Box::new(qrtz_app::module_hosting::AppleCodeChecks),
    );
    let t = qrtz_app::module_hosting::HttpTransport::new(std::slice::from_ref(&net.base), &[]);
    let before = net.hits();
    tokio::spawn(async move {
        let _ = installer
            .refresh(&t, &qrtz_modules::install::RefreshConsent::user_asked(), 0)
            .await;
    });
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while net.hits() == before {
        assert!(
            std::time::Instant::now() < end,
            "the refresh never reached the network"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Backlog P3: with Parakeet installed and verified, echo's download asks nothing of the network and waits on
/// nothing that does: not with the network down, and not while an index refresh stuck on that network holds the
/// install lock (before cycle 70 the empty install took the lock and failed "busy", reported as "the download
/// failed its signature check").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_installed_parakeet_downloads_with_the_network_down_without_touching_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (m, net) = installed_parakeet_offline(tmp.path()).await;
    let root = tmp.path().join("models");
    let echo = LocalModels::with_manifest(&root, m, Some(net.base.clone()))
        .with_index(Some("0".repeat(64)), vec![net.base.clone()])
        .for_extensions(&["echo"]);
    let ctl = qrtz_speech::download::DownloadControl::default();

    let t0 = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(2), echo.download(false, &ctl, &|_| {}))
        .await
        .expect("returns promptly")
        .unwrap();
    assert_eq!(net.hits(), 0, "no connection");

    refresh_stuck_on(tmp.path(), &net).await;
    let hits = net.hits();
    let last = std::sync::Mutex::new(None);
    tokio::time::timeout(
        Duration::from_secs(2),
        echo.download(false, &ctl, &|p| *last.lock().unwrap() = Some((p.done, p.total))),
    )
    .await
    .expect("returns promptly")
    .unwrap();
    assert_eq!(net.hits(), hits, "no connection of its own");
    let total = echo.total_size();
    assert_eq!(
        *last.lock().unwrap(),
        Some((total, total)),
        "progress says complete"
    );
    assert!(echo.installed());
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
}

/// Backlog P3, through settings / echo's view: with Parakeet installed, `speech:status` and `speech:download`
/// answer at once, the row goes back to idle with no error and the model stays installed, all with the network
/// down and an index refresh hanging on it; no connection is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn speech_status_and_download_with_parakeet_installed_never_wait_on_the_network() {
    let tmp = tempfile::tempdir().unwrap();
    let (m, net) = installed_parakeet_offline(tmp.path()).await;
    let mgr = SpeechManager::with_local(
        None,
        Some(
            LocalModels::with_manifest(tmp.path().join("models"), m, Some(net.base.clone()))
                .with_index(Some("0".repeat(64)), vec![net.base.clone()]),
        ),
        Events::new(),
    );
    mgr.select_extensions(Arc::new(|| vec!["echo".to_string(), "nano".to_string()]));
    let echo = ["echo".to_string()];
    refresh_stuck_on(tmp.path(), &net).await;
    let hits = net.hits();

    let t0 = std::time::Instant::now();
    let s = mgr.status_for(Some(&echo));
    assert_eq!(s["installed"], "1.0.0", "{s}");
    mgr.download_for(false, Some(echo.to_vec())).unwrap();
    within(&mgr, Duration::from_secs(2), "idle", || {
        mgr.status()["download"]["state"] == "idle"
    })
    .await;
    let s = mgr.status_for(Some(&echo));
    assert_eq!(s["installed"], "1.0.0", "{s}");
    assert_eq!(s["download"]["state"], "idle", "{s}");
    assert_eq!(s["download"]["error"], Value::Null, "{s}");
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    assert_eq!(net.hits(), hits, "no connection");
    assert!(
        !tmp.path().join("models/download.json").exists(),
        "no resume marker left"
    );
}
