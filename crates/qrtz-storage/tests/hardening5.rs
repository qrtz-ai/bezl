#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Hardening cycle 5 (N25, N26): the DB, its SQLite companions and the backup snapshot are owner-only.
//! Mirrors `apps/desktop/src/worker/db.hardening5.test.ts`. The box (and most sessions) run with umask 022.
#![cfg(unix)]

use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

use qrtz_storage::Db;

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn insert(db: &Db) {
    db.connection()
        .execute_batch("INSERT INTO meetings (id, title, platform, started_at, status) VALUES ('m1', 't', 'zoom', 1, 'done')")
        .unwrap();
}

#[test]
fn new_db_and_its_wal_and_shm_are_0600() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("qrtz.db");
    let db = Db::open(&file).unwrap();
    insert(&db);
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&dir.path().join("qrtz.db-wal")), 0o600);
    assert_eq!(mode(&dir.path().join("qrtz.db-shm")), 0o600);
}

#[test]
fn files_an_older_build_left_0644_are_tightened_rows_intact() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("qrtz.db");
    insert(&Db::open(&file).unwrap());
    let wal = dir.path().join("qrtz.db-wal");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&wal, b"").unwrap();
    std::fs::set_permissions(&wal, std::fs::Permissions::from_mode(0o644)).unwrap();
    let db = Db::open(&file).unwrap();
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&wal), 0o600);
    let n: i64 = db
        .connection()
        .query_row("SELECT count(*) FROM meetings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn snapshot_is_0600_and_never_follows_a_planted_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("qrtz.db")).unwrap();
    insert(&db);
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, b"keep me").unwrap();
    let snap = dir.path().join("snap.db");
    symlink(&victim, &snap).unwrap();
    db.snapshot_to(&snap).unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
    assert!(!std::fs::symlink_metadata(&snap).unwrap().file_type().is_symlink());
    assert_eq!(mode(&snap), 0o600);
    let copy = Db::open_readonly(&snap).unwrap();
    let n: i64 = copy
        .connection()
        .query_row("SELECT count(*) FROM meetings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}
