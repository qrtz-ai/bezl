//! The on-device fallback against the TS oracle (spec/vectors/llm/extractive.json, written by
//! packages/core/src/note/llm.vectors.test.ts), and the summary step that saves it (N53: a Tauri meeting always
//! ends with a summary).
use std::path::Path;
use std::sync::Arc;

use qrtz_lifecycle::runtime::{CoreSteps, InlineDb, StepExecutor, SummaryRequest};
use qrtz_llm::{extractive_summary, LlmSteps};
use qrtz_model::*;
use qrtz_storage::Db;
use serde_json::{json, Value};

fn vectors(name: &str) -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/vectors/llm")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn extractive_matches_ts() {
    let v = vectors("extractive.json");
    let cases = v["cases"].as_array().unwrap();
    assert!(cases.len() >= 7);
    for c in cases {
        let segs: Vec<Segment> = serde_json::from_value(c["segments"].clone()).unwrap();
        let o = &c["opts"];
        let notes: Vec<MeetingNote> =
            serde_json::from_value(o.get("notes").cloned().unwrap_or(json!([]))).unwrap();
        let got = extractive_summary(
            &segs,
            o["maxSentences"].as_u64().map(|n| n as usize),
            &notes,
            o["template"].as_str(),
        );
        assert_eq!(serde_json::to_value(&got).unwrap(), c["expect"], "{}", c["name"]);
    }
}

fn seeded() -> InlineDb {
    let db = Db::open_in_memory().unwrap();
    let m: Meeting = serde_json::from_value(json!({
        "id": "m1", "title": "Standup", "platform": "zoom", "startedAt": 1, "status": "processing",
        "participants": [], "template": "standup"
    }))
    .unwrap();
    db.notes().upsert_meeting(&m).unwrap();
    db.notes().set_template("m1", "standup").unwrap();
    let segs: Vec<Segment> = serde_json::from_value(json!([
        {"id": "s0", "startMs": 0, "endMs": 900, "text": "Yesterday I finished the parser and the tests. Today I am working on the importer.", "speakerName": "Ana", "source": "live"},
        {"id": "s1", "startMs": 1000, "endMs": 1900, "text": "I'll review the importer by tomorrow.", "speakerName": "Ben", "source": "live"}
    ]))
    .unwrap();
    db.notes()
        .replace_segments("m1", SegmentSource::Live, &segs)
        .unwrap();
    InlineDb::new(db)
}

fn steps(db: &InlineDb) -> LlmSteps<InlineDb> {
    let inner = Arc::new(CoreSteps { db: db.clone() });
    LlmSteps::on_device(db.clone(), inner, Arc::new(|| 42))
}

fn stored(db: &InlineDb) -> (Option<MeetingSummary>, MeetingStatus) {
    let g = db.0.lock().unwrap();
    (
        g.notes().get_summary("m1").unwrap(),
        g.notes().need("m1").unwrap().status,
    )
}

#[tokio::test]
async fn summary_step_saves_the_on_device_summary_and_finishes_the_meeting() {
    let db = seeded();
    let s = steps(&db)
        .summary("m1".into(), SummaryRequest::Llm)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.provider, "extractive");
    assert_eq!(s.template.as_deref(), Some("standup"));
    assert!(s.llm_error.is_none());
    assert_eq!(s.action_items[0].owner.as_deref(), Some("Ben"));
    let (saved, status) = stored(&db);
    assert_eq!(saved.unwrap(), s);
    assert_eq!(status, MeetingStatus::Done);
}

#[tokio::test]
async fn fallback_carries_the_llm_error_cut_to_500_utf16_units() {
    let db = seeded();
    let error = format!("{}😀tail", "e".repeat(499));
    let s = steps(&db)
        .summary("m1".into(), SummaryRequest::Fallback { error })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.llm_error.as_deref(), Some("e".repeat(499).as_str()));
    assert_eq!(stored(&db).0.unwrap().llm_error, s.llm_error);
}

#[tokio::test]
async fn skip_finishes_without_a_summary_and_resummarize_keeps_the_status() {
    let db = seeded();
    let st = steps(&db);
    assert!(st
        .summary("m1".into(), SummaryRequest::Skip)
        .await
        .unwrap()
        .is_none());
    assert_eq!(stored(&db), (None, MeetingStatus::Done));
    let s = st.resummarize("m1".into(), Some("sales".into())).await.unwrap();
    assert_eq!(s.template.as_deref(), Some("sales"));
    let g = db.0.lock().unwrap();
    assert_eq!(g.notes().need("m1").unwrap().template.as_deref(), Some("sales"));
}

#[tokio::test]
async fn a_deleted_meeting_is_not_found() {
    let db = seeded();
    let e = steps(&db)
        .summary("nope".into(), SummaryRequest::Llm)
        .await
        .unwrap_err();
    assert_eq!(e.code, "E_NOT_FOUND");
}
