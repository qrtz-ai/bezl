#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Cycle 22: a database an older build left behind (schema v1–v4) opens in this build, migrates forward to the
//! current schema with every row intact, and is searchable, owner-only and in WAL. The end-to-end version of this
//! was the TS test deleted with Electron (apps/desktop/src/worker/db.hardening4.test.ts, "an older DB (v2) still
//! migrates forward on open"); `migrate.rs` only migrates an empty connection.
use qrtz_storage::{migrate, Db, MIGRATIONS};
use rusqlite::{params, Connection};

/// A file DB at schema `v` (the first `v` migrations), filled the way that build would have filled it.
fn older_db(path: &std::path::Path, v: usize) {
    let mut c = Connection::open(path).unwrap();
    c.execute_batch("PRAGMA journal_mode = DELETE").unwrap();
    migrate(&mut c, &MIGRATIONS[..v]).unwrap();
    c.execute(
        "INSERT INTO kv (key, value, updated_at) VALUES ('onboarding', '{\"done\":true}', 1)",
        [],
    )
    .unwrap();
    if v < 2 {
        return;
    }
    c.execute(
        "INSERT INTO meetings (id, title, platform, started_at, ended_at, status, participants, audio_path, duration_ms)
         VALUES ('m1', 'O''Reilly budget sync', 'zoom', 1000, 61000, 'done', '[\"Ana\"]', NULL, 60000)",
        [],
    )
    .unwrap();
    for i in 0..3i64 {
        c.execute(
            "INSERT INTO segments (id, meeting_id, source, start_ms, end_ms, speaker, speaker_name, text, confidence)
             VALUES (?1, 'm1', 'live', ?2, ?3, NULL, 'Ana', ?4, 0.9)",
            params![format!("s{i}"), i * 1000, i * 1000 + 900, format!("the budget is approved, line {i}")],
        )
        .unwrap();
    }
    c.execute(
        "INSERT INTO summaries (meeting_id, provider, json, created_at) VALUES ('m1', 'openrouter', ?1, 2)",
        params![
            r#"{"summary":"Budget approved.","actionItems":[],"qa":[],"provider":"openrouter","createdAt":2}"#
        ],
    )
    .unwrap();
    if v >= 3 {
        let blob: Vec<u8> = [0.6f32, 0.8].iter().flat_map(|x| x.to_le_bytes()).collect();
        c.execute(
            "INSERT INTO segment_vectors (segment_rowid, meeting_id, model, vec)
             SELECT rowid, 'm1', 'Xenova/all-MiniLM-L6-v2@q8', ?1 FROM segments WHERE id = 's0'",
            params![blob],
        )
        .unwrap();
    }
    if v >= 4 {
        c.execute(
            "INSERT INTO notes (id, meeting_id, at_ms, text, created_at) VALUES ('n1', 'm1', 500, 'ask about Q3', 3)",
            [],
        )
        .unwrap();
        c.execute("UPDATE meetings SET template = 'sales' WHERE id = 'm1'", [])
            .unwrap();
    }
}

#[test]
fn every_older_schema_opens_migrates_forward_and_keeps_its_rows() {
    let current = MIGRATIONS.len() as u32;
    for v in 1..MIGRATIONS.len() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("qrtz.db");
        older_db(&path, v);
        let db = Db::open(&path).unwrap_or_else(|e| panic!("v{v}: {e}"));
        assert_eq!(db.schema_version().unwrap(), current, "v{v}");
        let r = db.migration().unwrap();
        assert_eq!((r.from, r.to), (v as u32, current), "v{v}");
        let journal: String = db
            .connection()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal, "wal", "v{v}");
        let kv: String = db
            .connection()
            .query_row("SELECT value FROM kv WHERE key = 'onboarding'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kv, "{\"done\":true}", "v{v}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "v{v}: N25 also for a DB an older build made");
        }
        if v < 2 {
            assert!(db.notes().list_meetings(10, 0).unwrap().is_empty(), "v{v}");
            continue;
        }
        let m = db.notes().need("m1").unwrap();
        assert_eq!(
            (m.title.as_str(), m.duration_ms),
            ("O'Reilly budget sync", Some(60000)),
            "v{v}"
        );
        assert_eq!(
            m.template.as_deref(),
            Some(if v >= 4 { "sales" } else { "general" }),
            "v{v}: migration 4 defaults older meetings to the general template"
        );
        assert_eq!(db.notes().get_segments("m1").unwrap().len(), 3, "v{v}");
        let hits = db.notes().search_segments("approved", 10).unwrap();
        assert_eq!(
            hits.len(),
            3,
            "v{v}: rows indexed by the old build are still searchable"
        );
        let s = db.notes().get_summary("m1").unwrap().unwrap();
        assert_eq!(s.summary, "Budget approved.", "v{v}");
        let vecs = db
            .notes()
            .vector_sources("Xenova/all-MiniLM-L6-v2@q8", Some("m1"))
            .unwrap();
        assert_eq!(vecs.len(), usize::from(v >= 3), "v{v}");
        // The Electron q8 vectors are kept but are not this build's model: the backfill re-embeds every line.
        assert_eq!(
            db.notes()
                .meetings_missing_vectors("all-MiniLM-L6-v2@candle-f32")
                .unwrap(),
            vec!["m1".to_string()],
            "v{v}"
        );
        assert_eq!(
            db.notes().get_notes("m1").unwrap().len(),
            usize::from(v >= 4),
            "v{v}"
        );
        // Migration 5's tables are there and empty: nothing to resume from an older build.
        assert!(db.lifecycle().get_pipeline("m1").unwrap().is_none(), "v{v}");
        // A new write through this build works on the migrated rows.
        db.notes().set_template("m1", "general").unwrap();
        drop(db);
        // Reopening is a no-op migration.
        let db = Db::open(&path).unwrap();
        assert!(db.migration().unwrap().applied.is_empty(), "v{v}");
    }
}
