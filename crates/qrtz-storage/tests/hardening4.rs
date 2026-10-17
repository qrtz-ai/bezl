//! Hardening cycle 4, storage: the files a deleted meeting leaves behind (N15). Mirrors
//! apps/desktop/src/main/note-service.hardening4.test.ts.

use qrtz_storage::model::Meeting;
use qrtz_storage::Db;

fn meeting(id: &str, audio: Option<&str>) -> Meeting {
    let mut v = serde_json::json!({
        "id": id, "title": "Weekly", "platform": "zoom", "startedAt": 1, "status": "processing", "participants": []
    });
    if let Some(a) = audio {
        v["audioPath"] = a.into();
    }
    serde_json::from_value(v).unwrap()
}

fn pipeline_row(db: &Db, id: &str, json: &str) {
    db.connection()
        .execute(
            "INSERT INTO pipeline_state (meeting_id,json,settled,updated_at) VALUES (?,?,0,1)",
            [id, json],
        )
        .unwrap();
}

#[test]
fn meeting_files_lists_recording_and_parts_like_ts() {
    let db = Db::open_in_memory().unwrap();
    db.notes()
        .upsert_meeting(&meeting("m4", Some("/r/m4.wav")))
        .unwrap();
    pipeline_row(
        &db,
        "m4",
        r#"{"ctx":{"audioParts":["/r/m4.wav","/r/m4.p1.wav","",7]}}"#,
    );
    assert_eq!(
        db.notes().meeting_files("m4").unwrap(),
        vec!["/r/m4.wav".to_string(), "/r/m4.p1.wav".to_string()]
    );
    db.notes().delete_meeting("m4").unwrap();
    assert!(db.notes().meeting_files("m4").unwrap().is_empty());
}

#[test]
fn meeting_files_survives_a_corrupt_pipeline_record() {
    let db = Db::open_in_memory().unwrap();
    db.notes()
        .upsert_meeting(&meeting("m5", Some("/r/m5.wav")))
        .unwrap();
    pipeline_row(&db, "m5", "{nope");
    assert_eq!(
        db.notes().meeting_files("m5").unwrap(),
        vec!["/r/m5.wav".to_string()]
    );
    db.notes().upsert_meeting(&meeting("m6", None)).unwrap();
    assert!(db.notes().meeting_files("m6").unwrap().is_empty());
}
