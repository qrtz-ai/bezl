//! The shared LLM vectors (spec/vectors/llm, written by packages/core/src/note/llm.vectors.test.ts): byte-identical
//! prompts and request bodies, the provider reply classification (N8) and the summary reply parser. Requests go to
//! a local mock server, never the network.
mod common;

use std::path::Path;

use common::{Mock, Reply};
use qrtz_llm::prompt::{capture_gaps_for_prompt, format_transcript_for_prompt, ChatMessage, Role};
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

fn openrouter(mock: &Mock) -> Box<dyn LlmProvider> {
    create_provider(
        &reqwest::Client::new(),
        "openrouter",
        Some(Secret::new("sk-or-test")),
        None,
        Some(&mock.base_url),
    )
    .unwrap()
}

#[tokio::test]
async fn prompts_match_ts() {
    assert_eq!(OPENROUTER_DEFAULT_MODEL, "anthropic/claude-haiku-5.5");
    let v = vectors("prompts.json");
    let mock = Mock::start().await;
    let prompts = v["prompts"].as_array().unwrap();
    assert!(prompts.len() >= 11);
    for c in prompts {
        let opts: SummaryOptions = serde_json::from_value(c["opts"].clone()).unwrap();
        let msgs = build_summary_prompt(c["title"].as_str().unwrap(), &segs_of(c), &opts);
        mock.push(Reply::content("{}"));
        openrouter(&mock).complete(&msgs, true).await.unwrap();
        let req = mock.seen().pop().unwrap();
        let got_msgs: Vec<Value> = msgs
            .iter()
            .map(|m| json!({"role": m.role, "content": pin(&m.content)}))
            .collect();
        let e = &c["expect"];
        assert_eq!(Value::Array(got_msgs), e["messages"], "{} messages", c["name"]);
        assert_eq!(pin(&req.body), e["request"]["body"], "{} body", c["name"]);
        let want_url = e["request"]["url"].as_str().unwrap();
        assert_eq!(
            req.path,
            want_url.strip_prefix("https://openrouter.ai/api/v1").unwrap()
        );
        assert_eq!(req.method, "POST");
        for (k, val) in e["request"]["headers"].as_object().unwrap() {
            assert_eq!(
                req.headers.get(&k.to_lowercase()).map(String::as_str),
                val.as_str(),
                "{} {k}",
                c["name"]
            );
        }
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

#[tokio::test]
async fn reply_classification_matches_ts() {
    let v = vectors("replies.json");
    let mock = Mock::start().await;
    let p = openrouter(&mock);
    let hi = [ChatMessage {
        role: Role::User,
        content: "hi".into(),
    }];
    for c in v["cases"].as_array().unwrap() {
        mock.push(Reply::Full {
            status: c["status"].as_u64().unwrap() as u16,
            content_type: c["contentType"].as_str().unwrap_or("application/json").into(),
            body: c["body"].as_str().unwrap().into(),
        });
        let got = match p.complete(&hi, false).await {
            Ok(t) => json!({"ok": t}),
            Err(e) => json!({"err": err_of(&e)}),
        };
        assert_eq!(got, c["expect"], "{}", c["name"]);
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
