//! Hardening cycle 12 (N48, ADR 0039): a word inside a run of Han, kana or Thai is found by search and the meeting
//! list. The vectors in spec/stress/fts-substring.json are shared with packages/core/src/note/fts.hardening12.test.ts.

use qrtz_storage::model::{Meeting, MeetingSummary, Segment, SegmentSource};
use qrtz_storage::{search_plan, substring_snippet, to_fts_query, Db, ListQuery};
use serde_json::{json, Value};

fn spec() -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/stress/fts-substring.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

fn seeded(s: &Value) -> Db {
    let db = Db::open_in_memory().unwrap();
    for m in s["meetings"].as_array().unwrap() {
        let id = m["id"].as_str().unwrap();
        let meeting: Meeting = serde_json::from_value(json!({
            "id": id, "title": m["title"], "platform": "zoom", "startedAt": m["startedAt"],
            "status": "done", "participants": []
        }))
        .unwrap();
        db.notes().upsert_meeting(&meeting).unwrap();
        let segs: Vec<Segment> = m["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                serde_json::from_value(json!({
                    "id": g["id"], "startMs": g["startMs"], "endMs": g["startMs"].as_i64().unwrap() + 500,
                    "text": g["text"], "source": "live"
                }))
                .unwrap()
            })
            .collect();
        db.notes()
            .replace_segments(id, SegmentSource::Live, &segs)
            .unwrap();
        if let Some(summary) = m.get("summary") {
            let sum: MeetingSummary = serde_json::from_value(json!({
                "summary": summary, "actionItems": [], "qa": [], "provider": "test"
            }))
            .unwrap();
            db.notes().save_summary(id, &sum, 1).unwrap();
        }
    }
    db
}

#[test]
fn plans_every_spec_input_like_ts() {
    let s = spec();
    for c in s["plans"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        let got = search_plan(input);
        if c["substrings"].is_null() {
            assert!(got.is_none(), "{input}");
            continue;
        }
        let p = got.unwrap_or_else(|| panic!("{input}"));
        assert_eq!(p.fts.as_deref(), c["fts"].as_str(), "{input}");
        assert_eq!(p.substrings, strings(&c["substrings"]), "{input}");
        if p.substrings.is_empty() {
            assert_eq!(p.fts, to_fts_query(input), "{input}: unchanged indexed path");
        }
    }
}

#[test]
fn builds_the_spec_snippet() {
    let s = spec();
    for c in s["snippets"].as_array().unwrap() {
        let text = c["text"].as_str().unwrap();
        assert_eq!(
            substring_snippet(text, &strings(&c["terms"])),
            c["snippet"].as_str().unwrap(),
            "{text}"
        );
    }
}

#[test]
fn search_returns_the_spec_hits_end_to_end() {
    let s = spec();
    let db = seeded(&s);
    for c in s["searches"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        let got: Vec<Value> = db
            .notes()
            .search(input, 50)
            .unwrap()
            .into_iter()
            .map(|h| {
                let mut v = json!({ "meetingId": h.meeting_id, "snippet": h.snippet });
                if let Some(ms) = h.start_ms {
                    v["startMs"] = json!(ms);
                }
                v
            })
            .collect();
        assert_eq!(Value::Array(got), c["hits"], "{input}");
    }
}

#[test]
fn list_query_and_delete_follow_the_substring_path() {
    let s = spec();
    let db = seeded(&s);
    let list = |q: &str| {
        db.notes()
            .list(&ListQuery {
                query: Some(q.into()),
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .map(|i| i.meeting.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(list("会议"), vec!["zh"]);
    assert_eq!(list("ประชุม"), vec!["th"]);
    db.notes().delete_meeting("zh").unwrap();
    assert!(list("会议").is_empty());
    assert!(db.notes().search("纪要", 50).unwrap().is_empty());
}
