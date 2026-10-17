//! The ranker against the TS oracle: spec/vectors/search/rank.json (queryTerms, rankSources, hybridRank) and the
//! `ranks` of spec/stress/chat-cjk.json (N49).
use std::path::Path;

use qrtz_search::*;
use serde_json::Value;

fn load(rel: &str) -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn src(v: &Value) -> ChatSource {
    ChatSource {
        meeting_id: v["meetingId"].as_str().unwrap().into(),
        title: v["title"].as_str().unwrap().into(),
        started_at: v["startedAt"].as_i64().unwrap(),
        start_ms: v["startMs"].as_i64().unwrap(),
        speaker: v["speaker"].as_str().map(Into::into),
        text: v["text"].as_str().unwrap().into(),
    }
}

fn vecf(v: &Value) -> Vec<f32> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap() as f32)
        .collect()
}

#[test]
fn rank_vectors_match_ts() {
    let v = load("spec/vectors/search/rank.json");
    let pool: Vec<VecSource> = v["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| VecSource {
            source: src(s),
            vec: s.get("vec").map(vecf),
        })
        .collect();
    let plain: Vec<ChatSource> = pool.iter().map(|p| p.source.clone()).collect();
    let id = |s: &ChatSource| format!("{}@{}", s.meeting_id, s.start_ms);
    let cases = v["cases"].as_array().unwrap();
    assert!(cases.len() >= 13);
    for c in cases {
        let (q, k) = (c["question"].as_str().unwrap(), c["k"].as_u64().unwrap() as usize);
        let terms: Vec<String> = serde_json::from_value(c["terms"].clone()).unwrap();
        assert_eq!(query_terms(q), terms, "{q}");
        let lex: Vec<String> = rank_sources(q, &plain, k).iter().map(id).collect();
        assert_eq!(serde_json::to_value(lex).unwrap(), c["lexical"], "lexical {q}");
        let qv = c.get("qvec").map(vecf);
        let hy: Vec<String> = hybrid_rank(q, &pool, qv.as_deref(), k, 0.25)
            .iter()
            .map(id)
            .collect();
        assert_eq!(serde_json::to_value(hy).unwrap(), c["hybrid"], "hybrid {q}");
    }
}

#[test]
fn chat_cjk_ranks_match_ts() {
    let v = load("spec/stress/chat-cjk.json");
    let all: Vec<ChatSource> = v["meetings"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| {
            m["segments"].as_array().unwrap().iter().map(move |s| ChatSource {
                meeting_id: m["id"].as_str().unwrap().into(),
                title: m["title"].as_str().unwrap().into(),
                started_at: m["startedAt"].as_i64().unwrap(),
                start_ms: s["startMs"].as_i64().unwrap(),
                speaker: s["speaker"].as_str().map(Into::into),
                text: s["text"].as_str().unwrap().into(),
            })
        })
        .collect();
    let ranks = v["ranks"].as_array().unwrap();
    assert!(!ranks.is_empty());
    for c in ranks {
        let q = c["question"].as_str().unwrap();
        let got: Vec<String> = rank_sources(q, &all, c["k"].as_u64().unwrap() as usize)
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert_eq!(serde_json::to_value(got).unwrap(), c["texts"], "{q}");
    }
}
