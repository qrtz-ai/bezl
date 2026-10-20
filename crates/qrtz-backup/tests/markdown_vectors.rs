#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! spec/vectors/export/markdown.json (TS oracle: packages/core/src/note/markdown.vectors.test.ts): same filename
//! and byte-identical Markdown, plus `export_meeting` through a real database.
use std::path::Path;

use qrtz_backup::{export_filename, export_meeting, meeting_to_markdown};
use qrtz_storage::model::{Meeting, MeetingNote, MeetingSummary, Segment, SegmentSource};
use qrtz_storage::Db;
use serde_json::Value;

fn cases() -> Vec<Value> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/export/markdown.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    v["cases"].as_array().unwrap().clone()
}

fn de<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn markdown_matches_the_ts_oracle_byte_for_byte() {
    let all = cases();
    assert!(all.len() >= 9);
    for c in all {
        let name = c["name"].as_str().unwrap();
        let m: Meeting = de(&c["meeting"]);
        let segs: Vec<Segment> = de(&c["segments"]);
        let summary: Option<MeetingSummary> = c.get("summary").map(de);
        let notes: Vec<MeetingNote> = de(&c["notes"]);
        assert_eq!(
            export_filename(&m),
            c["expected"]["filename"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            meeting_to_markdown(&m, &segs, summary.as_ref(), &notes),
            c["expected"]["markdown"].as_str().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn export_meeting_reads_everything_from_the_database() {
    let c = cases()
        .into_iter()
        .find(|c| c["name"].as_str().unwrap().starts_with("full"))
        .unwrap();
    let db = Db::open_in_memory().unwrap();
    let n = db.notes();
    let m: Meeting = de(&c["meeting"]);
    n.upsert_meeting(&m).unwrap();
    let segs: Vec<Segment> = de(&c["segments"]);
    n.replace_segments(&m.id, SegmentSource::Live, &segs).unwrap();
    let s: MeetingSummary = de(&c["summary"]);
    n.save_summary(&m.id, &s, 1).unwrap();
    for note in de::<Vec<MeetingNote>>(&c["notes"]) {
        n.add_note(&m.id, &note).unwrap();
    }
    let ex = export_meeting(&db, &m.id).unwrap();
    assert_eq!(ex.filename, c["expected"]["filename"].as_str().unwrap());
    assert_eq!(ex.markdown, c["expected"]["markdown"].as_str().unwrap());
    let err = export_meeting(&db, "nope").unwrap_err();
    assert_eq!(err.to_string(), "meeting nope not found");
}
