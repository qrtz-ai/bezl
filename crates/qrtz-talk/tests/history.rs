//! talk.json on disk: owner-only, atomic, stale tmp, corrupt file, full disk.
mod common;
use common::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::Ordering;

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[tokio::test(start_paused = true)]
async fn history_is_owner_only_and_atomic() {
    let r = rig(RigOpts::default());
    r.dictate(300).await;
    assert_eq!(mode(&r.store), 0o600);
    assert!(!qrtz_talk::store::tmp_path(&r.store).exists());
    let body: serde_json::Value = serde_json::from_slice(&std::fs::read(&r.store).unwrap()).unwrap();
    assert_eq!(body["history"].as_array().unwrap().len(), 1);
    assert!(body["dictionary"].is_array());
}

#[tokio::test(start_paused = true)]
async fn a_stale_tmp_from_a_crash_is_removed_and_never_reused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("talk.json");
    let tmp = qrtz_talk::store::tmp_path(&file);
    std::fs::write(&tmp, b"{\"history\":[]").unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
    qrtz_talk::store::write_private_atomic(&file, b"{}").unwrap();
    assert_eq!(
        mode(&file),
        0o600,
        "a world-readable stale tmp never becomes the file"
    );
    assert!(!tmp.exists());
}

#[tokio::test(start_paused = true)]
async fn stale_tmp_is_removed_at_startup() {
    let r = rig(RigOpts {
        seed_store: Some(br#"{"history":[],"dictionary":[]}"#.to_vec()),
        ..Default::default()
    });
    drop(r);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("talk.json");
    std::fs::write(qrtz_talk::store::tmp_path(&file), b"partial").unwrap();
    let (_, out) = qrtz_talk::store::load(&qrtz_talk::FsIo, &file, 0.0);
    assert!(matches!(out, qrtz_talk::store::LoadOutcome::Missing));
    assert!(!qrtz_talk::store::tmp_path(&file).exists());
}

#[tokio::test(start_paused = true)]
async fn corrupt_history_is_quarantined_and_dictation_still_works() {
    let r = rig(RigOpts {
        seed_store: Some(b"{\"history\": [ {\"id\": \"trunc".to_vec()),
        ..Default::default()
    });
    assert!(r.svc.history().is_empty());
    let moved: Vec<_> = std::fs::read_dir(r.dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("talk.json.corrupt-"))
        .collect();
    assert_eq!(
        moved.len(),
        1,
        "the unreadable file is kept aside, not overwritten"
    );
    assert_eq!(mode(&moved[0].path()), 0o600);
    let v = r.dictate(300).await;
    assert!(v.last.is_some());
    assert_eq!(r.svc.history().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn disk_full_keeps_memory_and_the_previous_file() {
    let r = rig(RigOpts::default());
    r.dictate(300).await;
    let before = std::fs::read(&r.store).unwrap();
    r.io.disk_full.store(true, Ordering::SeqCst);
    let v = r.dictate(300).await;
    assert!(v.last.is_some(), "the dictation itself is unaffected");
    assert_eq!(r.svc.history().len(), 2, "in memory");
    assert_eq!(std::fs::read(&r.store).unwrap(), before, "previous file intact");
    assert!(
        !qrtz_talk::store::tmp_path(&r.store).exists(),
        "no partial tmp left"
    );
    // Space comes back: the next write carries everything.
    r.io.disk_full.store(false, Ordering::SeqCst);
    r.dictate(300).await;
    let body: serde_json::Value = serde_json::from_slice(&std::fs::read(&r.store).unwrap()).unwrap();
    assert_eq!(body["history"].as_array().unwrap().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn real_disk_write_failure_cleans_its_tmp() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("talk.json");
    std::fs::create_dir_all(file.join("occupied")).unwrap(); // a non-empty directory where the file goes: rename fails
    assert!(qrtz_talk::store::write_private_atomic(&file, b"{}").is_err());
    assert!(!qrtz_talk::store::tmp_path(&file).exists());
}
