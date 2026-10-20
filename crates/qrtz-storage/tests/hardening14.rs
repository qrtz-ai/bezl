#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Hardening cycle 14 (N49, ADR 0047): chat retrieval for a question in Chinese, Japanese or Thai. The vectors in
//! spec/stress/chat-cjk.json are shared with packages/core/src/note/chat.hardening14.test.ts (the TS oracle).

use std::time::Instant;

use qrtz_storage::model::{Meeting, Segment, SegmentSource};
use qrtz_storage::{chat_plan, to_chat_fts_query, Db, CHAT_MAX_BIGRAMS};
use serde_json::{json, Value};

fn spec() -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/stress/chat-cjk.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn meeting(id: &str, title: &Value, started_at: &Value) -> Meeting {
    serde_json::from_value(json!({
        "id": id, "title": title, "platform": "zoom", "startedAt": started_at, "status": "done", "participants": []
    }))
    .unwrap()
}

fn seeded(s: &Value) -> Db {
    let db = Db::open_in_memory().unwrap();
    for m in s["meetings"].as_array().unwrap() {
        let id = m["id"].as_str().unwrap();
        db.notes()
            .upsert_meeting(&meeting(id, &m["title"], &m["startedAt"]))
            .unwrap();
        let segs: Vec<Segment> = m["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                let mut v = json!({
                    "id": g["id"], "startMs": g["startMs"], "endMs": g["startMs"].as_i64().unwrap() + 500,
                    "text": g["text"], "source": "live"
                });
                if let Some(sp) = g.get("speaker") {
                    v["speakerName"] = sp.clone();
                }
                serde_json::from_value(v).unwrap()
            })
            .collect();
        db.notes()
            .replace_segments(id, SegmentSource::Live, &segs)
            .unwrap();
    }
    db
}

#[test]
fn plans_every_spec_question_like_ts() {
    let s = spec();
    for c in s["plans"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        let got = chat_plan(input);
        if c["substrings"].is_null() {
            assert!(got.is_none(), "{input}");
            continue;
        }
        let p = got.unwrap_or_else(|| panic!("{input}"));
        assert_eq!(p.fts.as_deref(), c["fts"].as_str(), "{input}");
        let want: Vec<String> = serde_json::from_value(c["substrings"].clone()).unwrap();
        assert_eq!(p.substrings, want, "{input}");
        assert!(p.substrings.len() <= CHAT_MAX_BIGRAMS);
    }
    // No unsegmented word and no mark: the query retrieval always ran.
    assert_eq!(
        chat_plan("a ship it, Friday").unwrap().fts,
        to_chat_fts_query("a ship it, Friday")
    );
}

#[test]
fn retrieves_the_spec_lines_like_ts() {
    let s = spec();
    let db = seeded(&s);
    for c in s["retrievals"].as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        let got: Vec<Value> = db
            .notes()
            .search_segments(input, c["limit"].as_u64().unwrap() as u32)
            .unwrap()
            .into_iter()
            .map(|h| json!([h.meeting_id, h.start_ms]))
            .collect();
        assert_eq!(Value::Array(got), c["hits"], "{input}");
    }
}

#[test]
fn speaker_comes_back_with_a_bigram_hit() {
    let s = spec();
    let db = seeded(&s);
    let hits = db.notes().search_segments("我们决定了什么", 10).unwrap();
    assert_eq!(hits[0].text, "我们决定推迟发布。");
    assert_eq!(hits[0].speaker.as_deref(), Some("Li"));
}

#[test]
fn scans_200k_lines_in_bounded_time() {
    let db = Db::open_in_memory().unwrap();
    db.notes()
        .upsert_meeting(&meeting("big", &json!("big"), &json!(1)))
        .unwrap();
    let lines = [
        "我们讨论了一下进度",
        "the roadmap is on track",
        "เราจะคุยกันพรุ่งนี้",
        "来週また話しましょう",
    ];
    let segs: Vec<Segment> = (0..200_000)
        .map(|i| {
            let text = if i == 123_456 { "下周五之前把预算定下来" } else { lines[i % lines.len()] };
            serde_json::from_value(json!({
                "id": format!("s{i}"), "startMs": i * 1000, "endMs": i * 1000 + 900, "text": text, "source": "live"
            }))
            .unwrap()
        })
        .collect();
    db.notes()
        .replace_segments("big", SegmentSource::Live, &segs)
        .unwrap();
    let t0 = Instant::now();
    let hits = db
        .notes()
        .search_segments("我们下周五之前必须把预算和发布计划都定下来吧", 80)
        .unwrap();
    let ms = t0.elapsed().as_millis();
    eprintln!("N49 scan 200k lines: {ms} ms");
    assert_eq!(hits[0].text, "下周五之前把预算定下来");
    assert!(ms < 5_000, "{ms} ms");
}
