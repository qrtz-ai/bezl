//! The extractive summarizer against the TS oracle (spec/vectors/llm/extractive.json, written by
//! packages/core/src/note/llm.vectors.test.ts), and `load_sync` + `Loaded::extractive`, which qrtz-app's
//! `SummarySteps` falls back to when the on-device model fails (ADR 0075).
use std::path::Path;

use qrtz_llm::{extractive_summary, load_sync};
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

fn seeded() -> Db {
    let db = Db::open_in_memory().unwrap();
    let m: Meeting = serde_json::from_value(json!({
        "id": "m1", "title": "Standup", "platform": "zoom", "startedAt": 1, "status": "processing",
        "participants": [], "template": "general"
    }))
    .unwrap();
    db.notes().upsert_meeting(&m).unwrap();
    let segs: Vec<Segment> = serde_json::from_value(json!([
        {"id": "s0", "startMs": 0, "endMs": 900, "text": "Yesterday I finished the parser and the tests. Today I am working on the importer.", "speakerName": "Ana", "source": "live"},
        {"id": "s1", "startMs": 1000, "endMs": 1900, "text": "I'll review the importer by tomorrow.", "speakerName": "Ben", "source": "live"}
    ]))
    .unwrap();
    db.notes()
        .replace_segments("m1", SegmentSource::Live, &segs)
        .unwrap();
    db
}

#[tokio::test]
async fn load_stores_the_template_and_the_extractive_summary_uses_it() {
    let db = seeded();
    let m = load_sync(&db, "m1", Some("standup")).unwrap();
    assert_eq!(m.title, "Standup");
    assert_eq!(m.segments.len(), 2);
    assert_eq!(m.opts.template.as_deref(), Some("standup"));
    assert_eq!(
        db.notes().need("m1").unwrap().template.as_deref(),
        Some("standup")
    );
    let s = m.extractive().await;
    assert_eq!(s.provider, "extractive");
    assert_eq!(s.template.as_deref(), Some("standup"));
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        serde_json::to_value(m.extractive_sync()).unwrap()
    );
}

#[test]
fn a_deleted_meeting_is_not_found() {
    let db = seeded();
    assert!(load_sync(&db, "nope", None).is_err());
}
