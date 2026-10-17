//! ADR 0046 §4-5 against a mock bundle server: install, failure then retry, resume, checksum mismatch, bad
//! signature, oversize, pause/resume, update + rollback, anti-rollback, health failure, no swap mid-session, crash
//! loop, denylist (revoke, replay, expiry), key rotation (root + cross-signed only), lazy re-hash, permissions.
mod support;
use async_trait::async_trait;
use qrtz_speech::bundle::Manifest;
use qrtz_speech::download::{DownloadControl, Downloader, Progress};
use qrtz_speech::files::ModuleFiles;
use qrtz_speech::update::{HealthCheck, UpdateOutcome, Updater};
use qrtz_speech::{RollbackKind, SpeechError, SpeechStore};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use support::*;

struct Health(AtomicBool);
#[async_trait]
impl HealthCheck for Health {
    async fn check(&self, m: &Manifest, files: ModuleFiles) -> Result<(), String> {
        assert_eq!(files.entry.path, m.entry);
        assert_eq!(files.clip.path, m.health.clip);
        assert!(files.models.iter().all(|f| f.path != m.entry));
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("golden clip words differ".into())
        }
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    store: SpeechStore,
    dl: Downloader,
    health: Health,
    srv: Server,
    sk: ed25519_dalek::SigningKey,
    root: ed25519_dalek::SigningKey,
}

async fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let (sk, pk) = key(1, "k1");
    let (root, rootpk) = key(9, "root");
    let store = SpeechStore::open(tmp.path().join("speech"), APP, pk, rootpk).unwrap();
    let srv = Server::start().await;
    let (d, s) = denylist(&sk, "k1", 1, FAR, &[]);
    srv.put("denylist.json", d);
    srv.put("denylist.json.sig", s);
    Env {
        dl: Downloader::new(&srv.base),
        _tmp: tmp,
        store,
        health: Health(AtomicBool::new(true)),
        srv,
        sk,
        root,
    }
}

impl Env {
    async fn install(&self, force: bool) -> Result<UpdateOutcome, SpeechError> {
        self.install_with(force, &DownloadControl::default(), &|_| {})
            .await
    }
    async fn install_with(
        &self,
        force: bool,
        ctl: &DownloadControl,
        p: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<UpdateOutcome, SpeechError> {
        let now = || NOW;
        Updater {
            store: &self.store,
            dl: &self.dl,
            health: &self.health,
            now_ms: &now,
        }
        .install_latest(force, ctl, p)
        .await
    }
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn installs_verifies_and_makes_it_read_only() {
    let e = env().await;
    let b = bundle(&e.sk, "k1", "1.0.0", 4096);
    e.srv.publish(&b);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let out = e
        .install_with(false, &DownloadControl::default(), &move |p| {
            s2.lock().unwrap().push(p)
        })
        .await
        .unwrap();
    assert_eq!(
        out,
        UpdateOutcome::Installed {
            version: "1.0.0".into(),
            previous: None
        }
    );
    let last = *seen.lock().unwrap().last().unwrap();
    assert_eq!(last.done, last.total);
    let st = e.store.status(NOW);
    assert_eq!(st.current.as_deref(), Some("1.0.0"));
    // N72: no engine metadata in the status.
    assert!(!serde_json::to_string(&st).unwrap().contains("opaque-engine"));
    let root = e.store.root();
    assert_eq!(mode(root), 0o700);
    assert_eq!(mode(&root.join("current.json")), 0o600);
    let v = e.store.version_dir("1.0.0");
    assert_eq!(mode(&v), 0o500);
    assert_eq!(mode(&v.join("engine")), 0o500);
    assert_eq!(mode(&v.join("engine/weights.bin")), 0o400);
    assert_eq!(mode(&v.join("bin/qrtz-speech")), 0o500);
    assert!(
        std::fs::read_dir(root.join("staging")).unwrap().next().is_none(),
        "staging renamed away"
    );
    assert_eq!(
        e.install(false).await.unwrap(),
        UpdateOutcome::UpToDate {
            version: "1.0.0".into()
        }
    );
    // First load after the install re-hashes, then the session is pinned.
    let pin = e.store.pin_session(NOW).unwrap();
    assert_eq!(pin.version, "1.0.0");
    // N73: opened, hashed fds; the module never gets a path.
    let files = pin.open_files().unwrap();
    assert_eq!(files.models.len(), 1);
    assert_eq!(files.models[0].size, 4096);
}

#[tokio::test]
async fn a_failed_download_is_retryable_and_then_succeeds() {
    let e = env().await;
    let b = bundle(&e.sk, "k1", "1.0.0", 4096);
    e.srv.publish(&b);
    e.srv
        .faults
        .lock()
        .unwrap()
        .fail_once
        .insert("1.0.0/engine/weights.bin".into());
    let err = e.install(false).await.unwrap_err();
    assert!(matches!(err, SpeechError::Network(_)) && err.retryable(), "{err}");
    assert_eq!(e.store.status(NOW).current, None);
    assert!(matches!(
        e.install(false).await.unwrap(),
        UpdateOutcome::Installed { .. }
    ));
}

#[tokio::test]
async fn an_interrupted_download_resumes_with_a_range_request() {
    let e = env().await;
    let b = bundle(&e.sk, "k1", "1.0.0", 1 << 20);
    e.srv.publish(&b);
    e.srv
        .faults
        .lock()
        .unwrap()
        .truncate_once
        .insert("1.0.0/engine/weights.bin".into(), 300_000);
    assert!(e.install(false).await.unwrap_err().retryable());
    let part = e.store.staging_dir("1.0.0").join("engine/weights.bin.part");
    let have = std::fs::metadata(&part).unwrap().len();
    assert!(have > 0 && have <= 300_000, "partial kept: {have}");
    assert_eq!(mode(&part), 0o600);
    // "The app quit": a fresh store + downloader over the same dir resumes.
    let (_, pk) = key(1, "k1");
    let (_, rootpk) = key(9, "root");
    let store2 = SpeechStore::open(e.store.root(), APP, pk, rootpk).unwrap();
    let now = || NOW;
    let out = Updater {
        store: &store2,
        dl: &e.dl,
        health: &e.health,
        now_ms: &now,
    }
    .install_latest(false, &DownloadControl::default(), &|_| {})
    .await
    .unwrap();
    assert!(matches!(out, UpdateOutcome::Installed { .. }));
    let f = e.srv.faults.lock().unwrap();
    assert!(
        f.ranges
            .iter()
            .any(|(p, r)| p == "1.0.0/engine/weights.bin" && r == &format!("bytes={have}-")),
        "{:?}",
        f.ranges
    );
    assert_eq!(
        f.hits["1.0.0/bin/qrtz-speech"], 1,
        "finished files are not fetched again"
    );
}

#[tokio::test]
async fn pause_keeps_the_partial_and_resume_finishes() {
    let e = env().await;
    let b = bundle(&e.sk, "k1", "1.0.0", 2 << 20);
    e.srv.publish(&b);
    let ctl = DownloadControl::default();
    let c2 = ctl.clone();
    let err = e
        .install_with(false, &ctl, &move |p| {
            if p.done > 300_000 {
                c2.pause()
            }
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SpeechError::Paused), "{err}");
    assert!(e
        .store
        .staging_dir("1.0.0")
        .join("engine/weights.bin.part")
        .exists());
    ctl.resume();
    assert!(matches!(
        e.install_with(false, &ctl, &|_| {}).await.unwrap(),
        UpdateOutcome::Installed { .. }
    ));
}

#[tokio::test]
async fn a_checksum_mismatch_drops_the_file_and_a_retry_fixes_it() {
    let e = env().await;
    let b = bundle(&e.sk, "k1", "1.0.0", 4096);
    e.srv.publish(&b);
    e.srv
        .faults
        .lock()
        .unwrap()
        .corrupt
        .insert("1.0.0/engine/weights.bin".into());
    let err = e.install(false).await.unwrap_err();
    assert!(
        matches!(&err, SpeechError::Checksum(p) if p == "engine/weights.bin") && err.retryable(),
        "{err}"
    );
    let st = e.store.staging_dir("1.0.0");
    assert!(!st.join("engine/weights.bin.part").exists() && !st.join("engine/weights.bin").exists());
    assert_eq!(e.store.status(NOW).current, None);
    e.srv.faults.lock().unwrap().corrupt.clear();
    assert!(matches!(
        e.install(false).await.unwrap(),
        UpdateOutcome::Installed { .. }
    ));
}

#[tokio::test]
async fn more_bytes_than_the_manifest_says_are_refused() {
    let e = env().await;
    let b = bundle(&e.sk, "k1", "1.0.0", 4096);
    e.srv.publish(&b);
    e.srv
        .faults
        .lock()
        .unwrap()
        .oversize
        .insert("1.0.0/engine/weights.bin".into());
    assert!(matches!(
        e.install(false).await.unwrap_err(),
        SpeechError::TooLarge
    ));
}

#[tokio::test]
async fn a_bad_signature_is_refused_before_any_download() {
    let e = env().await;
    let (other, _) = key(2, "k1");
    let mut b = bundle(&e.sk, "k1", "1.0.0", 4096);
    b.sig = sign(&other, &b.manifest);
    e.srv.publish(&b);
    assert!(matches!(
        e.install(false).await.unwrap_err(),
        SpeechError::Signature(_)
    ));
    // A tampered manifest with the original signature too.
    let mut b = bundle(&e.sk, "k1", "1.0.0", 4096);
    b.manifest = String::from_utf8(b.manifest)
        .unwrap()
        .replace("opaque-engine", "evil-engine")
        .into_bytes();
    e.srv.publish(&b);
    assert!(matches!(
        e.install(false).await.unwrap_err(),
        SpeechError::Signature(_)
    ));
    assert_eq!(
        e.srv.faults.lock().unwrap().hits.get("1.0.0/engine/weights.bin"),
        None
    );
    assert!(!e.store.staging_dir("1.0.0").exists());
}

#[tokio::test]
async fn update_keeps_n_minus_1_and_rollback_returns_to_it() {
    let e = env().await;
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    e.install(false).await.unwrap();
    e.srv.publish(&bundle(&e.sk, "k1", "1.1.0", 4096));
    assert_eq!(
        e.install(false).await.unwrap(),
        UpdateOutcome::Installed {
            version: "1.1.0".into(),
            previous: Some("1.0.0".into())
        }
    );
    e.srv.publish(&bundle(&e.sk, "k1", "1.2.0", 4096));
    e.install(false).await.unwrap();
    assert!(!e.store.version_dir("1.0.0").exists(), "only N and N-1 are kept");
    let rb = e.store.rollback(RollbackKind::User, NOW).unwrap();
    assert_eq!((rb.from.as_str(), rb.to.as_deref()), ("1.2.0", Some("1.1.0")));
    assert!(!e.store.version_dir("1.2.0").exists());
    assert_eq!(e.store.state().highest_verified.as_deref(), Some("1.2.0"));
    // Anti-rollback: the server offering something older than the highest verified is refused…
    e.srv.publish(&bundle(&e.sk, "k1", "1.1.5", 4096));
    assert!(matches!(
        e.install(false).await.unwrap_err(),
        SpeechError::Rollback { .. }
    ));
    // …but the same highest version may be fetched again.
    e.srv.publish(&bundle(&e.sk, "k1", "1.2.0", 4096));
    assert!(matches!(
        e.install(false).await.unwrap(),
        UpdateOutcome::Installed { .. }
    ));
}

#[tokio::test]
async fn a_failed_health_check_never_goes_live() {
    let e = env().await;
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    e.install(false).await.unwrap();
    e.health.0.store(false, Ordering::SeqCst);
    e.srv.publish(&bundle(&e.sk, "k1", "1.1.0", 4096));
    assert!(matches!(
        e.install(false).await.unwrap_err(),
        SpeechError::Health(_)
    ));
    assert_eq!(e.store.status(NOW).current.as_deref(), Some("1.0.0"));
    assert!(!e.store.version_dir("1.1.0").exists());
    assert_eq!(e.store.state().highest_verified.as_deref(), Some("1.0.0"));
}

#[tokio::test]
async fn no_swap_while_a_session_runs() {
    let e = env().await;
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    e.install(false).await.unwrap();
    let pin = e.store.pin_session(NOW).unwrap();
    e.srv.publish(&bundle(&e.sk, "k1", "1.1.0", 4096));
    assert_eq!(
        e.install(false).await.unwrap(),
        UpdateOutcome::Waiting {
            version: "1.1.0".into()
        }
    );
    assert_eq!(e.store.status(NOW).current.as_deref(), Some("1.0.0"));
    assert!(matches!(e.store.delete_all().unwrap_err(), SpeechError::Busy));
    assert!(matches!(
        e.store.rollback(RollbackKind::User, NOW).unwrap_err(),
        SpeechError::Busy
    ));
    drop(pin);
    e.store.activate("1.1.0", NOW).unwrap();
    assert_eq!(e.store.status(NOW).current.as_deref(), Some("1.1.0"));
}

#[tokio::test]
async fn three_failures_in_five_minutes_roll_back() {
    let e = env().await;
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    e.install(false).await.unwrap();
    e.srv.publish(&bundle(&e.sk, "k1", "1.1.0", 4096));
    e.install(false).await.unwrap();
    let t = NOW;
    assert_eq!(e.store.record_failure(t).unwrap(), None);
    // Old failures fall out of the window.
    assert_eq!(e.store.record_failure(t + 6 * 60_000).unwrap(), None);
    assert_eq!(e.store.record_failure(t + 7 * 60_000).unwrap(), None);
    let rb = e.store.record_failure(t + 8 * 60_000).unwrap().unwrap();
    assert_eq!(
        (rb.from.as_str(), rb.to.as_deref(), rb.kind),
        ("1.1.0", Some("1.0.0"), RollbackKind::CrashLoop)
    );
}

#[tokio::test]
async fn denylist_revokes_ignores_replays_and_expiry_blocks_installs() {
    let e = env().await;
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    e.install(false).await.unwrap();
    e.srv.publish(&bundle(&e.sk, "k1", "1.1.0", 4096));
    e.install(false).await.unwrap();
    let (d, s) = denylist(&e.sk, "k1", 2, FAR, &["1.1.0"]);
    let rb = e.store.apply_denylist(&d, &s, NOW).unwrap().unwrap();
    assert_eq!((rb.from.as_str(), rb.to.as_deref()), ("1.1.0", Some("1.0.0")));
    // Replay of an older list: nothing changes.
    let (d1, s1) = denylist(&e.sk, "k1", 1, FAR, &[]);
    assert_eq!(e.store.apply_denylist(&d1, &s1, NOW).unwrap(), None);
    assert!(e.store.state().denied.contains(&"1.1.0".to_string()));
    // A revoked version is refused as an install.
    e.srv.put("denylist.json", d);
    e.srv.put("denylist.json.sig", s);
    assert!(matches!(
        e.install(false).await.unwrap_err(),
        SpeechError::Denied(_)
    ));
    // A forged list is refused.
    let (bad, _) = key(3, "k1");
    let (d3, s3) = denylist(&bad, "k1", 9, FAR, &["1.0.0"]);
    assert!(matches!(
        e.store.apply_denylist(&d3, &s3, NOW).unwrap_err(),
        SpeechError::Signature(_)
    ));
    // Freeze: only an expired list available → no installs (sessions keep running).
    let e2 = env().await;
    let (dx, sx) = denylist(&e2.sk, "k1", 1, NOW - 1, &[]);
    e2.srv.put("denylist.json", dx);
    e2.srv.put("denylist.json.sig", sx);
    e2.srv.publish(&bundle(&e2.sk, "k1", "1.0.0", 4096));
    assert!(matches!(
        e2.install(false).await.unwrap_err(),
        SpeechError::RevocationExpired
    ));
}

fn rotation(
    root: &ed25519_dalek::SigningKey,
    new: &ed25519_dalek::SigningKey,
    seq: u64,
    expires: u64,
) -> (Vec<u8>, Vec<u8>) {
    let b = serde_json::to_vec(&serde_json::json!({
        "seq": seq, "expires": expires, "previousKeyId": "k1", "keyId": "k2",
        "publicKey": hex::encode(new.verifying_key().to_bytes()),
    }))
    .unwrap();
    let sigs = serde_json::to_vec(&serde_json::json!({
        "root": String::from_utf8(sign(root, &b)).unwrap(),
        "new": String::from_utf8(sign(new, &b)).unwrap(),
    }))
    .unwrap();
    (b, sigs)
}

#[tokio::test]
async fn rotation_needs_the_root_key_and_the_new_key() {
    let e = env().await;
    let (k2, _) = key(4, "k2");
    // Signed only by the (possibly stolen) old signing key: refused.
    let (b, _) = rotation(&e.root, &k2, 1, FAR);
    let forged = serde_json::to_vec(&serde_json::json!({
        "root": String::from_utf8(sign(&e.sk, &b)).unwrap(), "new": String::from_utf8(sign(&k2, &b)).unwrap(),
    }))
    .unwrap();
    assert!(e.store.apply_rotation(&b, &forged, NOW).is_err());
    // Missing the new key's cross-signature: refused.
    let (_, other) = key(5, "x");
    let no_cross = serde_json::to_vec(&serde_json::json!({
        "root": String::from_utf8(sign(&e.root, &b)).unwrap(), "new": String::from_utf8(sign(&e.sk, &b)).unwrap(),
    })).unwrap();
    let _ = other;
    assert!(e.store.apply_rotation(&b, &no_cross, NOW).is_err());
    // Expired: refused.
    let (bx, sx) = rotation(&e.root, &k2, 1, NOW - 1);
    assert!(e.store.apply_rotation(&bx, &sx, NOW).is_err());
    // Proper rotation pins k2; k1 bundles are now refused, k2 bundles accepted; a replay is refused.
    let (b, s) = rotation(&e.root, &k2, 1, FAR);
    assert_eq!(e.store.apply_rotation(&b, &s, NOW).unwrap().key_id, "k2");
    assert!(e.store.apply_rotation(&b, &s, NOW).is_err());
    let (d, ds) = denylist(&k2, "k2", 5, FAR, &[]);
    e.srv.put("denylist.json", d);
    e.srv.put("denylist.json.sig", ds);
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    assert!(matches!(
        e.install(false).await.unwrap_err(),
        SpeechError::Signature(_)
    ));
    e.srv.publish(&bundle(&k2, "k2", "1.0.0", 4096));
    assert!(matches!(
        e.install(false).await.unwrap(),
        UpdateOutcome::Installed { .. }
    ));
}

#[tokio::test]
async fn a_tampered_install_fails_the_lazy_rehash_on_first_load() {
    let e = env().await;
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    e.install(false).await.unwrap();
    let f = e.store.version_dir("1.0.0").join("engine/weights.bin");
    std::fs::set_permissions(f.parent().unwrap(), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&f, b"tampered").unwrap();
    // N73: what is opened is hashed on every load.
    let pin = e.store.pin_session(NOW).unwrap();
    assert!(matches!(pin.open_files().unwrap_err(), SpeechError::Checksum(_)));
    drop(pin);
    assert_eq!(e.store.active_sessions(), 0, "the pin is released");
}

#[tokio::test]
async fn delete_keeps_anti_rollback_state_and_redownload_reinstalls() {
    let e = env().await;
    e.srv.publish(&bundle(&e.sk, "k1", "1.1.0", 4096));
    e.install(false).await.unwrap();
    assert!(
        matches!(e.install(true).await.unwrap(), UpdateOutcome::Installed { .. }),
        "re-download"
    );
    e.store.delete_all().unwrap();
    assert_eq!(e.store.status(NOW).current, None);
    assert!(matches!(
        e.store.pin_session(NOW).unwrap_err(),
        SpeechError::NotInstalled
    ));
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    assert!(matches!(
        e.install(false).await.unwrap_err(),
        SpeechError::Rollback { .. }
    ));
}

// ---- Fail-open on an expired revocation list (Bozniak, 07:58 PT): conditions 1-5 ----

/// 1.0.0 then 1.1.0 installed at NOW under a list issued NOW that expires NOW + 7 days.
async fn two_versions() -> Env {
    let e = env().await;
    let (d, s) = denylist_at(&e.sk, "k1", 1, NOW, NOW + 7 * DAY, &[]);
    e.srv.put("denylist.json", d);
    e.srv.put("denylist.json.sig", s);
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    e.install(false).await.unwrap();
    e.srv.publish(&bundle(&e.sk, "k1", "1.1.0", 4096));
    e.install(false).await.unwrap();
    e
}

#[tokio::test]
async fn c1_the_cached_list_and_the_floor_keep_applying_after_expiry() {
    let e = two_versions().await;
    let (d, s) = denylist_at(&e.sk, "k1", 2, NOW, NOW + 7 * DAY, &["1.0.0"]);
    assert_eq!(e.store.apply_denylist(&d, &s, NOW).unwrap(), None);
    assert_eq!(e.store.state().previous, None, "a denied N-1 is dropped");
    // Expired now: sessions still run on the clean current version (fail-open)…
    let later = NOW + 10 * DAY;
    let pin = e.store.pin_session(later).unwrap();
    assert_eq!(pin.version, "1.1.0");
    drop(pin);
    let rev = e.store.status(later).revocation;
    assert!(rev.expired && !rev.stale);
    assert_eq!(rev.age_ms, 10 * DAY);
    // …a later list that no longer names 1.0.0 does not un-deny it (denials never shrink)…
    let (d3, s3) = denylist_at(&e.sk, "k1", 3, later, later + 7 * DAY, &[]);
    e.store.apply_denylist(&d3, &s3, later).unwrap();
    assert!(e.store.state().denied.contains(&"1.0.0".to_string()));
    // …and the anti-rollback floor holds.
    assert_eq!(e.store.state().highest_verified.as_deref(), Some("1.1.0"));
    assert!(matches!(
        e.store
            .check_candidate(&manifest_of(&bundle(&e.sk, "k1", "1.0.0", 4096)))
            .unwrap_err(),
        SpeechError::Denied(_)
    ));
    assert!(matches!(
        e.store
            .check_candidate(&manifest_of(&bundle(&e.sk, "k1", "1.0.5", 4096)))
            .unwrap_err(),
        SpeechError::Rollback { .. }
    ));
}

fn manifest_of(b: &Bundle) -> Manifest {
    serde_json::from_slice(&b.manifest).unwrap()
}

#[tokio::test]
async fn c2_moving_the_clock_back_does_not_unexpire_the_list() {
    let e = two_versions().await;
    assert!(e.store.denylist_fresh(NOW + DAY));
    assert!(!e.store.denylist_fresh(NOW + 8 * DAY), "expired");
    // Clock set back: still expired (high-water mark), also after a restart.
    assert!(!e.store.denylist_fresh(NOW + DAY));
    let (_, pk) = key(1, "k1");
    let (_, rootpk) = key(9, "root");
    let reopened = SpeechStore::open(e.store.root(), APP, pk, rootpk).unwrap();
    assert!(!reopened.denylist_fresh(NOW + DAY));
    assert_eq!(
        reopened.status(NOW).revocation.age_ms,
        8 * DAY,
        "age measured against the mark too"
    );
    // A list's own `issued` raises the mark: an old list cannot be made fresh by an early clock.
    let e2 = env().await;
    let (d, s) = denylist_at(&e2.sk, "k1", 7, NOW + 5 * DAY, NOW + 6 * DAY, &[]);
    e2.store.apply_denylist(&d, &s, NOW).unwrap();
    assert_eq!(e2.store.state().clock_hwm, NOW + 5 * DAY);
    // N63: an `issued` beyond the skew bound is refused (it would push the mark into the future).
    let (dfut, sfut) = denylist_at(&e2.sk, "k1", 9, NOW + 30 * DAY, NOW + 31 * DAY, &[]);
    assert!(e2.store.apply_denylist(&dfut, &sfut, NOW).is_err());
    let (d8, s8) = denylist_at(&e2.sk, "k1", 8, NOW, NOW + 3 * DAY, &[]);
    e2.store.apply_denylist(&d8, &s8, NOW).unwrap();
    assert!(
        !e2.store.denylist_fresh(NOW),
        "issued in the 'past' of the mark: expired"
    );
}

#[tokio::test]
async fn c3_expired_means_no_install_update_or_rollback_except_a_crash_loop_to_a_clean_n_minus_1() {
    let e = two_versions().await;
    let later = NOW + 10 * DAY;
    e.store.effective_now(later);
    // No install / update (the server still serves the old, expired list).
    e.srv.publish(&bundle(&e.sk, "k1", "1.2.0", 4096));
    let err = e.install(false).await.unwrap_err();
    assert!(matches!(err, SpeechError::RevocationExpired), "{err}");
    assert!(
        matches!(e.install(true).await.unwrap_err(), SpeechError::RevocationExpired),
        "no re-download either"
    );
    assert!(matches!(
        e.store.activate("1.0.0", later).unwrap_err(),
        SpeechError::RevocationExpired
    ));
    // No user rollback.
    assert!(matches!(
        e.store.rollback(RollbackKind::User, later).unwrap_err(),
        SpeechError::RevocationExpired
    ));
    assert_eq!(e.store.status(later).current.as_deref(), Some("1.1.0"));
    // Crash loop to a clean N-1: allowed.
    for i in 0..2 {
        assert_eq!(e.store.record_failure(later + i).unwrap(), None);
    }
    let rb = e.store.record_failure(later + 2).unwrap().unwrap();
    assert_eq!(
        (rb.to.as_deref(), rb.kind),
        (Some("1.0.0"), RollbackKind::CrashLoop)
    );
}

#[tokio::test]
async fn c3_no_crash_loop_rollback_to_a_denied_n_minus_1() {
    let e = two_versions().await;
    // 1.0.0 denied by a list we saw; even if it were still on disk it must not run.
    let (d, s) = denylist_at(&e.sk, "k1", 2, NOW, NOW + 7 * DAY, &["1.0.0"]);
    e.store.apply_denylist(&d, &s, NOW).unwrap();
    let later = NOW + 10 * DAY;
    for i in 0..3 {
        assert_eq!(e.store.record_failure(later + i).unwrap(), None);
    }
    assert_eq!(e.store.status(later).current.as_deref(), Some("1.1.0"));
}

#[tokio::test]
async fn c4_a_newer_expired_list_still_applies_its_denials() {
    let e = two_versions().await;
    let later = NOW + 10 * DAY;
    let (d, s) = denylist_at(&e.sk, "k1", 2, NOW + DAY, NOW + 8 * DAY, &["1.1.0"]);
    // Expired itself, validly signed: the denial applies; with the list expired there is no rollback (condition 3),
    // so the denied current version just stops running.
    assert_eq!(e.store.apply_denylist(&d, &s, later).unwrap(), None);
    assert!(e.store.state().denied.contains(&"1.1.0".to_string()));
    assert!(matches!(
        e.store.pin_session(later).unwrap_err(),
        SpeechError::Denied(_)
    ));
    // Freshness is not granted by it.
    assert!(!e.store.denylist_fresh(later));
    // A forged "newer" list still does nothing.
    let (bad, _) = key(3, "k1");
    let (df, sf) = denylist_at(&bad, "k1", 9, later, later + 7 * DAY, &[]);
    assert!(e.store.apply_denylist(&df, &sf, later).is_err());
}

#[tokio::test]
async fn c5_settings_shows_the_age_and_sessions_stop_after_90_days() {
    let e = two_versions().await;
    let d89 = NOW + 89 * DAY;
    let rev = e.store.status(d89).revocation;
    assert!(rev.expired && !rev.stale);
    assert_eq!((rev.issued, rev.age_ms), (NOW, 89 * DAY));
    drop(e.store.pin_session(d89).unwrap());
    let d91 = NOW + 91 * DAY;
    assert!(e.store.status(d91).revocation.stale);
    assert!(matches!(
        e.store.pin_session(d91).unwrap_err(),
        SpeechError::Stale { days: 91 }
    ));
    assert_eq!(e.store.active_sessions(), 0);
    // A fresh list (back online) lifts it.
    let (d, s) = denylist_at(&e.sk, "k1", 2, d91, d91 + 7 * DAY, &[]);
    e.store.apply_denylist(&d, &s, d91).unwrap();
    drop(e.store.pin_session(d91).unwrap());
}

#[tokio::test]
async fn n73_a_symlink_in_an_installed_tree_is_never_followed() {
    let e = env().await;
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    e.install(false).await.unwrap();
    let d = e.store.version_dir("1.0.0").join("engine");
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
    let w = d.join("weights.bin");
    let real = e.store.root().join("real.bin");
    std::fs::copy(&w, &real).unwrap();
    std::fs::remove_file(&w).unwrap();
    std::os::unix::fs::symlink(&real, &w).unwrap();
    let pin = e.store.pin_session(NOW).unwrap();
    assert!(matches!(pin.open_files().unwrap_err(), SpeechError::Invalid(_)));
}

#[tokio::test]
async fn n74_a_pinned_version_is_not_swept_by_a_rollback() {
    let e = env().await;
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    e.install(false).await.unwrap();
    e.srv.publish(&bundle(&e.sk, "k1", "1.1.0", 4096));
    e.install(false).await.unwrap();
    let pin = e.store.pin_session(NOW).unwrap();
    assert!(matches!(
        e.store.rollback(RollbackKind::User, NOW).unwrap_err(),
        SpeechError::Busy
    ));
    assert!(pin.open_files().is_ok(), "the pinned tree is intact");
}

#[tokio::test]
async fn n75_manifests_with_more_than_32_model_files_are_refused() {
    let e = env().await;
    let (sk, _) = key(1, "k1");
    let mut files: Vec<serde_json::Value> = (0..33)
        .map(|i| serde_json::json!({"path": format!("m/{i}.bin"), "size": 1, "sha256": sha(b"x")}))
        .collect();
    files.push(serde_json::json!({"path": "bin/qrtz-speech", "size": 1, "sha256": sha(b"x")}));
    files.push(serde_json::json!({"path": "health/golden.pcm", "size": 1, "sha256": sha(b"x")}));
    let m = serde_json::to_vec(&serde_json::json!({
        "module": "qrtz-speech", "version": "1.0.0", "protocol": 1, "minApp": "0.1.0", "entry": "bin/qrtz-speech",
        "files": files, "health": {"clip": "health/golden.pcm", "expected": "x"}, "keyId": "k1",
    }))
    .unwrap();
    let sig = sign(&sk, &m);
    assert!(qrtz_speech::bundle::verify_manifest(
        &e.store.key(),
        &m,
        &sig,
        &semver::Version::parse(APP).unwrap()
    )
    .is_err());
}

#[tokio::test]
async fn n67_a_corrupt_state_with_no_good_copy_fails_closed() {
    let e = env().await;
    e.srv.publish(&bundle(&e.sk, "k1", "1.0.0", 4096));
    e.install(false).await.unwrap();
    std::fs::write(e.store.root().join("current.json"), b"{").unwrap();
    std::fs::write(e.store.root().join("current.json.bak"), b"{").unwrap();
    let (_, pk) = key(1, "k1");
    let (_, rootpk) = key(9, "root");
    assert!(matches!(
        SpeechStore::open(e.store.root(), APP, pk, rootpk),
        Err(SpeechError::StateCorrupt)
    ));
}
