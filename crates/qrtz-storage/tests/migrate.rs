//! Port of packages/core/src/db/migrate.test.ts plus the shared-file guarantees.

use std::path::Path;

use qrtz_storage::{migrate, schema_version, Db, Migration, StorageError, MIGRATIONS};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

#[test]
fn applies_in_order_and_is_idempotent() {
    let mut c = Connection::open_in_memory().unwrap();
    let r = migrate(&mut c, MIGRATIONS).unwrap();
    assert_eq!((r.from, r.to), (0, 6));
    assert_eq!(
        r.applied,
        [
            "1_kv",
            "2_memo",
            "3_segment_vectors",
            "4_notes_templates",
            "5_recording_lifecycle",
            "6_calendar"
        ]
    );
    assert!(migrate(&mut c, MIGRATIONS).unwrap().applied.is_empty());
}

#[test]
fn rolls_back_a_failing_migration() {
    let mut c = Connection::open_in_memory().unwrap();
    let mut ms = MIGRATIONS.to_vec();
    ms.push(Migration {
        version: 7,
        name: "bad",
        sql: "CREATE TABLE t(x); NOT SQL",
    });
    let e = migrate(&mut c, &ms).unwrap_err();
    assert!(e.to_string().contains("7_bad"), "{e}");
    assert_eq!(schema_version(&c).unwrap(), 6);
    let t: Option<String> = c
        .query_row("SELECT name FROM sqlite_master WHERE name='t'", [], |r| r.get(0))
        .ok();
    assert_eq!(t, None);
    assert!(c.is_autocommit());
}

#[test]
fn rejects_gaps_and_newer_databases() {
    let mut c = Connection::open_in_memory().unwrap();
    let e = migrate(
        &mut c,
        &[Migration {
            version: 2,
            name: "x",
            sql: "",
        }],
    )
    .unwrap_err();
    assert!(matches!(e, StorageError::NonContiguous { .. }) && e.to_string().contains("contiguous"));
    c.execute_batch("PRAGMA user_version = 9").unwrap();
    let e = migrate(&mut c, MIGRATIONS).unwrap_err();
    assert!(matches!(e, StorageError::SchemaTooNew { db: 9, app: 6 }) && e.to_string().contains("newer"));
    assert_eq!(e.code(), "E_VALIDATION");
}

#[test]
fn bundled_sqlite_has_fts5_and_is_at_least_electrons_minor() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE VIRTUAL TABLE f USING fts5(x)").unwrap();
    let v: String = c.query_row("SELECT sqlite_version()", [], |r| r.get(0)).unwrap();
    let p: Vec<u32> = v.split('.').map(|x| x.parse().unwrap()).collect();
    assert!(
        p[0] == 3 && p[1] >= 53,
        "bundled SQLite {v} is older than Electron's 3.53.x"
    );
}

#[test]
fn matches_the_shared_migration_manifest() {
    let m: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("spec/schema/migrations.json")).unwrap())
            .unwrap();
    let rows = m["migrations"].as_array().unwrap();
    assert_eq!(rows.len(), MIGRATIONS.len());
    for (row, mig) in rows.iter().zip(MIGRATIONS) {
        assert_eq!(row["version"].as_u64().unwrap() as u32, mig.version);
        assert_eq!(row["name"].as_str().unwrap(), mig.name);
        let text = std::fs::read_to_string(root().join(row["file"].as_str().unwrap())).unwrap();
        assert_eq!(
            mig.sql, text,
            "include_str! must be the shared file byte for byte"
        );
        let hash: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hash,
            row["sha256"].as_str().unwrap(),
            "shipped migration {} was edited",
            mig.name
        );
    }
}

/// meetings_fts is keyed by the implicit meetings.rowid, which a plain VACUUM may renumber (see crate docs).
/// Until the fix migration lands, no Rust source may run one; `VACUUM INTO` (snapshots) is fine.
#[test]
fn no_plain_vacuum_anywhere_in_the_rust_crates() {
    let mut bad = Vec::new();
    let crates = root().join("crates");
    let mut stack = vec![crates];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                if !p.ends_with("target") && !p.ends_with("tests") {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&p).unwrap().to_uppercase();
                for (i, line) in src.lines().enumerate() {
                    let code = line.split("//").next().unwrap();
                    if let Some(at) = code.find("VACUUM") {
                        if !code[at..].starts_with("VACUUM INTO") {
                            bad.push(format!("{}:{}", p.display(), i + 1));
                        }
                    }
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "plain VACUUM renumbers meetings.rowid and breaks meetings_fts: {bad:?}"
    );
}

#[test]
fn open_sets_the_worker_pragmas() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("q.db")).unwrap();
    let c = db.connection();
    let jm: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
    let bt: i64 = c.query_row("PRAGMA busy_timeout", [], |r| r.get(0)).unwrap();
    assert_eq!((jm.as_str(), fk, bt), ("wal", 1, 5000));
    assert_eq!(db.migration().unwrap().to, 6);
    drop(db);
    assert!(Db::open(dir.path().join("q.db"))
        .unwrap()
        .migration()
        .unwrap()
        .applied
        .is_empty());
}
