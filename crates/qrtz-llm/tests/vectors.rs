#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! The shared summary vectors (spec/vectors/llm, written by packages/core/src/note/llm.vectors.test.ts):
//! byte-identical prompts and the summary reply parser. No network (ADR 0076).
use std::path::Path;

use qrtz_llm::prompt::{capture_gaps_for_prompt, format_transcript_for_prompt};
use qrtz_llm::*;
use qrtz_model::{LifecycleError, Segment, SilentSpan};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn vectors(name: &str) -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/vectors/llm")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// `pin` of the TS side: long strings as {length (UTF-16), sha256 (UTF-8), head, tail (100 code points)}.
fn pin(s: &str) -> Value {
    let len = s.encode_utf16().count();
    if len <= 4000 {
        return json!(s);
    }
    let cs: Vec<char> = s.chars().collect();
    json!({
        "length": len,
        "sha256": format!("{:x}", Sha256::digest(s.as_bytes())),
        "head": cs[..100].iter().collect::<String>(),
        "tail": cs[cs.len() - 100..].iter().collect::<String>(),
    })
}

/// `genSegments` (see the `about` of prompts.json).
fn segs_of(c: &Value) -> Vec<Segment> {
    if let Some(s) = c.get("segments") {
        return serde_json::from_value(s.clone()).unwrap();
    }
    let g = &c["gen"];
    let (count, step) = (g["count"].as_i64().unwrap(), g["stepMs"].as_i64().unwrap());
    let sp = g["speakers"].as_array().unwrap();
    let tx = g["texts"].as_array().unwrap();
    let rep = g["repeat"].as_u64().unwrap_or(1) as usize;
    (0..count)
        .map(|i| {
            let mut s = json!({
                "id": format!("g{i}"), "startMs": i * step, "endMs": i * step + step - 100,
                "text": format!("{} #{i}", tx[i as usize % tx.len()].as_str().unwrap().repeat(rep)),
                "source": "live",
            });
            if let Some(n) = sp[i as usize % sp.len()].as_str() {
                s["speakerName"] = json!(n);
            }
            serde_json::from_value(s).unwrap()
        })
        .collect()
}

fn err_of(e: &LifecycleError) -> Value {
    let mut m = e.message.clone();
    for p in [
        "LLM reply was not JSON",
        "LLM reply did not match the summary shape",
    ] {
        if m.starts_with(&format!("{p}: ")) {
            m = format!("{p}: …");
        }
    }
    json!([e.code, e.retryable, m])
}

#[test]
fn prompts_match_ts() {
    let v = vectors("prompts.json");
    let prompts = v["prompts"].as_array().unwrap();
    assert!(prompts.len() >= 11);
    for c in prompts {
        let opts: SummaryOptions = serde_json::from_value(c["opts"].clone()).unwrap();
        let msgs = build_summary_prompt(c["title"].as_str().unwrap(), &segs_of(c), &opts);
        let got_msgs: Vec<Value> = msgs
            .iter()
            .map(|m| json!({"role": m.role, "content": pin(&m.content)}))
            .collect();
        let e = &c["expect"];
        assert_eq!(Value::Array(got_msgs), e["messages"], "{} messages", c["name"]);
    }
    for c in v["transcripts"].as_array().unwrap() {
        let got = format_transcript_for_prompt(&segs_of(c), c["maxChars"].as_u64().unwrap() as usize);
        assert_eq!(pin(&got), c["expect"], "transcript {}", c["name"]);
        assert!(
            got.encode_utf16().count() <= c["maxChars"].as_u64().unwrap() as usize || c["name"] == "fits"
        );
    }
    for c in v["gaps"].as_array().unwrap() {
        let spans: Vec<SilentSpan> = serde_json::from_value(c["spans"].clone()).unwrap();
        let lost: Vec<SilentSpan> =
            serde_json::from_value(c.get("lost").cloned().unwrap_or(json!([]))).unwrap();
        let got = capture_gaps_for_prompt(&spans, c["durationMs"].as_i64(), &lost);
        assert_eq!(json!(got), c["expect"], "gaps {}", c["name"]);
    }
}

#[test]
fn summary_reply_parser_matches_ts() {
    let v = vectors("summary-replies.json");
    for c in v["cases"].as_array().unwrap() {
        let got = match parse_summary_reply(c["text"].as_str().unwrap()) {
            Ok(r) => json!({"ok": r}),
            Err(e) => json!({"err": err_of(&e)}),
        };
        assert_eq!(got, c["expect"], "{}", c["name"]);
    }
}
