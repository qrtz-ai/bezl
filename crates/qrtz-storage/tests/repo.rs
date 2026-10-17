//! Behaviour the cross-open fixture doesn't pin: lenient reads, corruption rules, snapshot/import, the handle.

use std::collections::HashMap;

use qrtz_storage::model::{Meeting, MeetingPlatform, MeetingStatus, RecordingPhase, RecordingState, Segment};
use qrtz_storage::{to_chat_fts_query, to_fts_query, Db, ListQuery, StorageError, StorageHandle};
use rusqlite::params;

fn meeting(id: &str, title: &str, started_at: i64) -> Meeting {
    serde_json::from_value(serde_json::json!({
        "id": id, "title": title, "platform": "zoom", "startedAt": started_at, "status": "done", "participants": []
    }))
    .unwrap()
}

fn seg(id: &str, text: &str) -> Segment {
    serde_json::from_value(
        serde_json::json!({"id": id, "startMs": 0, "endMs": 1, "text": text, "source": "live"}),
    )
    .unwrap()
}

#[test]
fn fts_query_builders() {
    assert_eq!(
        to_fts_query("Hello, World!").as_deref(),
        Some("\"hello\"* \"world\"*")
    );
    assert_eq!(to_fts_query("O'Reilly").as_deref(), Some("\"o'reilly\"*")); // N46
    assert_eq!(to_fts_query("\"*()"), None);
    assert_eq!(to_fts_query("   "), None);
    assert_eq!(
        to_fts_query("a b c d e f g h i j").unwrap().matches('*').count(),
        8
    );
    assert_eq!(
        to_chat_fts_query("a ship it, Friday").as_deref(),
        Some("\"ship\"* OR \"it\"* OR \"friday\"*")
    );
    assert_eq!(to_chat_fts_query("a ? !"), None);
}

#[test]
fn reads_float_ms_columns_written_by_js() {
    // node:sqlite binds JS numbers; a non-integral ms value lands as REAL. Rust reads it rounded, never fails.
    let db = Db::open_in_memory().unwrap();
    db.notes().upsert_meeting(&meeting("m", "T", 1)).unwrap();
    db.connection()
        .execute(
            "UPDATE meetings SET started_at=?, duration_ms=? WHERE id='m'",
            params![1000.6_f64, 12.4_f64],
        )
        .unwrap();
    let m = db.notes().get_meeting("m").unwrap().unwrap();
    assert_eq!((m.started_at, m.duration_ms), (1001, Some(12)));
}

#[test]
fn missing_meeting_is_not_found_with_the_ts_message() {
    let db = Db::open_in_memory().unwrap();
    let e = db.notes().detail("nope").unwrap_err();
    assert_eq!(e.to_string(), "meeting nope not found");
    assert_eq!(e.code(), "E_NOT_FOUND");
}

#[test]
fn list_pages_return_limit_plus_one() {
    let db = Db::open_in_memory().unwrap();
    for i in 0..5 {
        db.notes()
            .upsert_meeting(&meeting(&format!("m{i}"), "standup", i))
            .unwrap();
    }
    let n = |q: ListQuery| db.notes().list(&q).unwrap().len();
    assert_eq!(
        n(ListQuery {
            limit: Some(2),
            ..Default::default()
        }),
        3
    );
    assert_eq!(
        n(ListQuery {
            limit: Some(2),
            offset: Some(4),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        n(ListQuery {
            query: Some("standup".into()),
            limit: Some(2),
            ..Default::default()
        }),
        3
    );
}

#[test]
fn lifecycle_corruption_rules_match_ts() {
    let db = Db::open_in_memory().unwrap();
    let life = db.lifecycle();
    assert!(life.load_session().unwrap().is_none());
    let s = RecordingState {
        phase: RecordingPhase::Idle,
        ..Default::default()
    };
    life.save_session(&s, 5).unwrap();
    assert_eq!(life.load_session().unwrap(), Some(s));
    let put = |j: &str| {
        db.connection()
            .execute("UPDATE lifecycle_state SET json=? WHERE key='session'", [j])
            .unwrap();
    };
    put(r#"{"phase":"warp-speed"}"#); // valid JSON, wrong shape: safeParse fails -> null
    assert!(life.load_session().unwrap().is_none());
    put("{not json"); // JSON.parse throws
    assert!(matches!(life.load_session(), Err(StorageError::Corrupt(_))));
}

#[test]
fn orphans_and_deleted_meeting_pipelines() {
    let db = Db::open_in_memory().unwrap();
    let mut m = meeting("r", "rec", 1);
    m.status = MeetingStatus::Recording;
    m.platform = MeetingPlatform::Teams;
    db.notes().upsert_meeting(&m).unwrap();
    assert_eq!(db.lifecycle().orphans().unwrap().len(), 1);
    assert!(db.lifecycle().list_unsettled().unwrap().is_empty());
}

#[test]
fn snapshot_and_import_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let src = Db::open(dir.path().join("src.db")).unwrap();
    let mut m = meeting("a", "Imported café", 10);
    m.audio_path = Some("/old/place/a.wav".into());
    src.notes().upsert_meeting(&m).unwrap();
    src.notes().set_template("a", "general").unwrap();
    src.notes()
        .replace_segments(
            "a",
            qrtz_storage::model::SegmentSource::Live,
            &[seg("s", "hello café")],
        )
        .unwrap();
    let snap = dir.path().join("snap.db");
    src.snapshot_to(&snap).unwrap();
    src.snapshot_to(&snap).unwrap(); // overwrites

    let dst = Db::open(dir.path().join("dst.db")).unwrap();
    dst.notes().upsert_meeting(&meeting("b", "local", 20)).unwrap();
    let audio = HashMap::from([("a.wav".to_string(), dir.path().join("restored/a.wav"))]);
    let r = dst.import_from(&snap, &audio).unwrap();
    assert_eq!((r.imported, r.skipped), (1, 0));
    assert_eq!(dst.import_from(&snap, &audio).unwrap().skipped, 1);
    let got = dst.notes().get_meeting("a").unwrap().unwrap();
    assert_eq!(got.template.as_deref(), Some("general"));
    assert!(got.audio_path.unwrap().ends_with("restored/a.wav"));
    let hits = dst
        .notes()
        .list(&ListQuery {
            query: Some("cafe".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].meeting.id, "a");

    std::fs::write(dir.path().join("junk.db"), b"definitely not sqlite").unwrap();
    assert!(Db::open_readonly(dir.path().join("junk.db")).is_err());
}

#[tokio::test]
async fn handle_serializes_jobs_and_survives_panics() {
    let h = StorageHandle::spawn_in_memory().unwrap();
    let mut waits = Vec::new();
    for i in 0..50 {
        let h2 = h.clone();
        waits.push(tokio::spawn(async move {
            h2.call(move |db| db.notes().upsert_meeting(&meeting(&format!("m{i:02}"), "x", i)))
                .await
        }));
    }
    for w in waits {
        w.await.unwrap().unwrap();
    }
    let e = h
        .call(|db| -> qrtz_storage::Result<()> {
            db.connection().execute_batch("BEGIN").unwrap();
            db.notes().upsert_meeting(&meeting("ghost", "x", 0)).unwrap();
            panic!("boom")
        })
        .await
        .unwrap_err();
    assert!(matches!(&e, StorageError::Panicked(m) if m == "boom"));
    assert!(e.retryable());
    let n = h
        .call(|db| Ok(db.notes().list_meetings(1000, 0)?.len()))
        .await
        .unwrap();
    assert_eq!(
        n, 50,
        "the panicked job's transaction was rolled back and the thread kept serving"
    );
    let blocking = h.clone();
    let v = tokio::task::spawn_blocking(move || blocking.call_blocking(|db| db.schema_version()))
        .await
        .unwrap();
    assert_eq!(v.unwrap(), 5);
}

#[test]
fn handle_reports_open_errors() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("new.db");
    {
        let c = rusqlite::Connection::open(&p).unwrap();
        c.execute_batch("PRAGMA user_version = 99").unwrap();
    }
    assert!(matches!(
        StorageHandle::spawn(p),
        Err(StorageError::SchemaTooNew { db: 99, .. })
    ));
}
