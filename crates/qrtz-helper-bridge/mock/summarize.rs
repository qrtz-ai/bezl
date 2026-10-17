//! ADR 0075: the mocks' on-device model, shared by `qrtz-mock-helper` and `qrtz-app/examples/e2e_helper.rs`
//! (both pull it in with `#[path]`). Knob (env):
//!   QRTZ_MOCK_SUMMARIZE=ok|unavailable:<reason>|context>N|guardrail|hang|hang-once:<path>|invalid|busy-while:<path>
//!     ok (default): `summarize.result` built from the fenced lines (each line with " will " is an action item,
//!     owner = speaker); unavailable:apple-intelligence-off etc.: `summarize.error` unavailable; context>N:
//!     `context` for a prompt over N chars; guardrail: `guardrail`; hang: never answers (until
//!     `summarize.cancel`); hang-once:<path>: the first request (no `path` yet) writes this process's pid to
//!     `path` and hangs, every later one (also after a restart) is ok; invalid: a `summarize.result` that fails the
//!     contract (`summary` is a number); busy-while:<path>: `busy` (retryable) while `path` exists, then ok.
//!     `summarize.check` answers `summarize.status` to match.
//!   QRTZ_MOCK_SUMMARIZE_LOG=path  every `summarize` request line is appended to `path` (tests read the prompts).
use serde_json::{json, Value};
use std::io::Write;

fn knob(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// The answer to a `summarize` / `summarize.check` / `summarize.cancel` line (None: no answer).
pub fn handle(line: &str, msg: &Value) -> Option<Value> {
    if msg["type"] == "summarize" {
        if let Some(p) = knob("QRTZ_MOCK_SUMMARIZE_LOG") {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "{line}");
            }
        }
    }
    let id = msg["id"].clone();
    let mode = knob("QRTZ_MOCK_SUMMARIZE").unwrap_or_else(|| "ok".into());
    let unavailable = mode.strip_prefix("unavailable:");
    match msg["type"].as_str()? {
        "summarize.check" => Some(match unavailable {
            Some(r) => json!({"type": "summarize.status", "id": id, "available": false, "reason": r}),
            None => json!({"type": "summarize.status", "id": id, "available": true, "contextTokens": 4096}),
        }),
        "summarize.cancel" => {
            Some(json!({"type": "summarize.error", "id": id, "code": "cancelled", "message": "cancelled"}))
        }
        "summarize" => {
            let prompt = msg["prompt"].as_str().unwrap_or_default();
            let stage = msg["stage"].as_str().unwrap_or("single");
            if let Some(r) = unavailable {
                return Some(
                    json!({"type": "summarize.error", "id": id, "code": "unavailable", "reason": r,
                    "message": "unavailable"}),
                );
            }
            if let Some(path) = mode.strip_prefix("busy-while:") {
                if std::path::Path::new(path).exists() {
                    return Some(
                        json!({"type": "summarize.error", "id": id, "code": "busy", "message": "busy"}),
                    );
                }
            }
            if mode == "hang" {
                return None;
            }
            if let Some(path) = mode.strip_prefix("hang-once:") {
                if !std::path::Path::new(path).exists() {
                    let _ = std::fs::write(path, std::process::id().to_string());
                    return None;
                }
            }
            if mode == "invalid" {
                return Some(
                    json!({"type": "summarize.result", "id": id, "summary": 7, "actionItems": [], "qa": []}),
                );
            }
            if mode == "guardrail" {
                return Some(
                    json!({"type": "summarize.error", "id": id, "code": "guardrail", "message": "refused"}),
                );
            }
            if let Some(n) = mode
                .strip_prefix("context>")
                .and_then(|n| n.parse::<usize>().ok())
            {
                if prompt.chars().count() > n {
                    return Some(json!({"type": "summarize.error", "id": id, "code": "context",
                        "message": "over the context window"}));
                }
            }
            // The fenced body: lines between `<tag-nonce>` and `</tag-nonce>`.
            let mut inside = false;
            let mut fenced = Vec::new();
            for l in prompt.lines() {
                if l.starts_with("</") && l.ends_with('>') {
                    inside = false;
                } else if l.starts_with('<') && l.ends_with('>') && !l.contains(' ') {
                    inside = true;
                } else if inside {
                    fenced.push(l);
                }
            }
            let mut items = Vec::new();
            for l in &fenced {
                if let Some((who, text)) = l.split_once(": ") {
                    if text.contains(" will ") && items.len() < 12 {
                        let who = who.rsplit("] ").next().unwrap_or(who);
                        items.push(json!({"text": text.chars().take(400).collect::<String>(),
                            "owner": who.chars().take(80).collect::<String>()}));
                    }
                }
            }
            Some(json!({
                "type": "summarize.result",
                "id": id,
                "summary": format!("mock on-device {stage} summary of {} fenced lines.", fenced.len()),
                "actionItems": items,
                "qa": [],
            }))
        }
        _ => None,
    }
}
