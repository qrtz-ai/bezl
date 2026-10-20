#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Hardening cycle 11 (N46, ADR 0038): search terms with an apostrophe or combining marks. The vectors in
//! spec/stress/fts-queries.json are shared with packages/core/src/note/fts.hardening11.test.ts.

use qrtz_storage::model::{Meeting, Segment, SegmentSource};
use qrtz_storage::{to_fts_query, Db, ListQuery};
use rusqlite::{params, Connection};

fn spec() -> serde_json::Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/stress/fts-queries.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn query_builder_matches_the_spec() {
    let s = spec();
    for c in s["cases"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        assert_eq!(to_fts_query(input).as_deref(), c["query"].as_str(), "{input}");
    }
}

#[test]
fn real_fts5_table_returns_the_spec_rows() {
    let s = spec();
    let c = Connection::open_in_memory().unwrap();
    let tok = s["tokenize"].as_str().unwrap();
    c.execute_batch(&format!(
        "CREATE VIRTUAL TABLE f USING fts5(text, tokenize='{tok}')"
    ))
    .unwrap();
    for (i, t) in s["corpus"].as_array().unwrap().iter().enumerate() {
        c.execute(
            "INSERT INTO f(rowid, text) VALUES (?1, ?2)",
            params![i as i64 + 1, t.as_str().unwrap()],
        )
        .unwrap();
    }
    for case in s["cases"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        let want: Vec<i64> = case["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        let got: Vec<i64> = match to_fts_query(input) {
            None => vec![],
            Some(q) => {
                let mut st = c
                    .prepare("SELECT rowid FROM f WHERE f MATCH ?1 ORDER BY rowid")
                    .unwrap();
                st.query_map([q], |r| r.get::<_, i64>(0))
                    .unwrap()
                    .map(|r| r.unwrap() - 1)
                    .collect()
            }
        };
        assert_eq!(got, want, "{input}");
    }
}

#[test]
fn end_to_end_search_finds_contractions_and_possessives() {
    let db = Db::open_in_memory().unwrap();
    let m: Meeting = serde_json::from_value(serde_json::json!({
        "id": "m", "title": "Vendor sync", "platform": "zoom", "startedAt": 1, "status": "done", "participants": []
    }))
    .unwrap();
    db.notes().upsert_meeting(&m).unwrap();
    let seg: Segment = serde_json::from_value(serde_json::json!({
        "id": "s", "startMs": 0, "endMs": 1, "text": "Don't forget the O'Reilly contract.", "source": "live"
    }))
    .unwrap();
    db.notes()
        .replace_segments("m", SegmentSource::Live, &[seg])
        .unwrap();
    for q in [
        "don't",
        "Don't forget",
        "O'Reilly",
        "O'Reilly's contract",
        "o\u{2019}reilly",
    ] {
        let hits = db.notes().search(q, 50).unwrap();
        assert_eq!(
            hits.iter().map(|h| h.meeting_id.as_str()).collect::<Vec<_>>(),
            vec!["m"],
            "{q}"
        );
    }
    let listed = db
        .notes()
        .list(&ListQuery {
            query: Some("don't".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(listed.len(), 1);
}
