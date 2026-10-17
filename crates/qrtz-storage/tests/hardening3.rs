//! Hardening cycle 3, storage: search across one huge meeting (N10) and atomic backup import (N11).
//! Mirrors packages/core/src/note/hardening3.stress.test.ts and apps/desktop/src/worker/import.hardening3.test.ts.

use std::collections::HashMap;

use qrtz_storage::model::{Meeting, Segment, SegmentSource};
use qrtz_storage::Db;

fn meeting(id: &str, title: &str, started_at: i64) -> Meeting {
    serde_json::from_value(serde_json::json!({
        "id": id, "title": title, "platform": "zoom", "startedAt": started_at, "status": "done", "participants": []
    }))
    .unwrap()
}

fn seg(id: &str, start_ms: i64, text: &str) -> Segment {
    serde_json::from_value(serde_json::json!({
        "id": id, "startMs": start_ms, "endMs": start_ms + 1, "text": text, "source": "live"
    }))
    .unwrap()
}

fn page_count(db: &Db) -> i64 {
    db.connection()
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn search_is_not_starved_by_one_long_meeting() {
    let db = Db::open_in_memory().unwrap();
    let n = db.notes();
    n.upsert_meeting(&meeting("allhands", "All hands", 1)).unwrap();
    let lines: Vec<_> = (0..600)
        .map(|i| seg(&format!("a{i}"), i * 10_000, &format!("budget budget {i}")))
        .collect();
    n.replace_segments("allhands", SegmentSource::Live, &lines)
        .unwrap();
    for k in 0..5 {
        let id = format!("m{k}");
        n.upsert_meeting(&meeting(&id, &format!("Sync {k}"), 10 + k))
            .unwrap();
        n.replace_segments(
            &id,
            SegmentSource::Live,
            &[seg(
                &format!("s{k}"),
                0,
                &format!(
                    "We went over the quarterly plan and the marketing budget, among a few other topics {k}."
                ),
            )],
        )
        .unwrap();
    }
    let mut ids: Vec<_> = n
        .search("budget", 50)
        .unwrap()
        .into_iter()
        .map(|h| h.meeting_id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["allhands", "m0", "m1", "m2", "m3", "m4"]);
    let top2 = n.search("budget", 2).unwrap();
    assert_eq!(top2.len(), 2);
    assert_eq!(top2[0].meeting_id, "allhands");
    assert!(top2
        .iter()
        .all(|h| h.snippet.contains("[budget]") && h.start_ms.is_some()));
}

#[test]
fn import_that_hits_disk_full_mid_meeting_leaves_nothing_and_reimport_repairs() {
    let dir = tempfile::tempdir().unwrap();
    let src = Db::open(dir.path().join("a.db")).unwrap();
    src.notes()
        .upsert_meeting(&meeting("long", "Quarterly planning", 1))
        .unwrap();
    let lines: Vec<_> = (0..1_500)
        .map(|i| {
            seg(
                &format!("s{i}"),
                i * 4_000,
                &format!("line {i}: we talked through the roadmap and the hiring plan in some detail"),
            )
        })
        .collect();
    src.notes()
        .replace_segments("long", SegmentSource::Live, &lines)
        .unwrap();
    let snap = dir.path().join("snap.db");
    src.snapshot_to(&snap).unwrap();

    let dst = Db::open_in_memory().unwrap();
    let pages = page_count(&dst);
    dst.connection()
        .execute_batch(&format!("PRAGMA max_page_count = {}", pages + 4))
        .unwrap();
    let err = dst.import_from(&snap, &HashMap::new()).unwrap_err();
    assert!(err.to_string().contains("full"), "{err}");
    assert!(dst.notes().get_meeting("long").unwrap().is_none());

    dst.connection()
        .execute_batch("PRAGMA max_page_count = 1073741823")
        .unwrap();
    let r = dst.import_from(&snap, &HashMap::new()).unwrap();
    assert_eq!((r.imported, r.skipped), (1, 0));
    assert_eq!(dst.notes().get_segments("long").unwrap().len(), 1_500);
}

#[test]
fn disk_full_is_retryable_unavailable_like_ts() {
    let db = Db::open_in_memory().unwrap();
    db.notes().upsert_meeting(&meeting("m", "M", 1)).unwrap();
    let pages = page_count(&db);
    db.connection()
        .execute_batch(&format!("PRAGMA max_page_count = {pages}"))
        .unwrap();
    let lines: Vec<_> = (0..500)
        .map(|i| seg(&format!("s{i}"), i, &"words ".repeat(40)))
        .collect();
    let e = db
        .notes()
        .replace_segments("m", SegmentSource::Live, &lines)
        .unwrap_err();
    assert!(e.to_string().contains("full"), "{e}");
    assert_eq!(e.code(), "E_UNAVAILABLE");
    assert!(e.retryable());
}

#[test]
fn sql_errors_stay_internal() {
    let db = Db::open_in_memory().unwrap();
    let e: qrtz_storage::StorageError = db
        .connection()
        .execute_batch("SELECT * FROM no_such_table")
        .unwrap_err()
        .into();
    assert_eq!(e.code(), "E_INTERNAL");
    assert!(!e.retryable());
}
