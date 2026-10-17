//! Hardening cycle 4 (N19): a database from a newer app is refused before anything writes to it. Mirrors
//! apps/desktop/src/worker/db.hardening4.test.ts.

use qrtz_storage::{Db, StorageError};

#[test]
fn newer_database_is_refused_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("qrtz.db");
    {
        let c = rusqlite::Connection::open(&file).unwrap();
        c.execute_batch(
            "PRAGMA journal_mode = DELETE; CREATE TABLE future (x); INSERT INTO future VALUES (42); PRAGMA user_version = 99;",
        )
        .unwrap();
    }
    let before = std::fs::read(&file).unwrap();
    match Db::open(&file) {
        Err(StorageError::SchemaTooNew { db, .. }) => assert_eq!(db, 99),
        Err(e) => panic!("wrong error {e}"),
        Ok(_) => panic!("opened a newer database"),
    }
    assert_eq!(std::fs::read(&file).unwrap(), before, "file bytes changed");
    assert!(!dir.path().join("qrtz.db-wal").exists());
    let c = rusqlite::Connection::open(&file).unwrap();
    let mode: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, "delete");
    let x: i64 = c.query_row("SELECT x FROM future", [], |r| r.get(0)).unwrap();
    assert_eq!(x, 42);
}

#[test]
fn current_database_reopens_in_wal() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("qrtz.db");
    drop(Db::open(&file).unwrap());
    let db = Db::open(&file).unwrap();
    let mode: String = db
        .connection()
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}
