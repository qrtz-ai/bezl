//! Phase 5 port of e2e/states.spec.ts "LLM failure falls back to on-device summary, Retry gets the AI summary"
//! (docs/e2e-parity.md row 32; "helper missing" is in e2e_shell.rs, "hundreds of meetings" in e2e_scale.rs).
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn llm_failure_falls_back_on_device_and_retry_gets_the_ai_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let llm = MockHttp::start().await;
    llm.always(overloaded());
    let a = App::launch(
        &tmp.path().join("data"),
        Opts::default().standup("0.02").llm(&llm.base),
    )
    .await;
    a.settings(
        "secrets:set",
        json!({"name": "llm.openrouter", "value": "sk-test-stub"}),
    )
    .await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    let (id, _) = record_until(&a, None, |t| segments(t) > 3).await;
    let list = a.note("note:list", json!({})).await;
    let st = list[0]["meeting"]["status"].as_str().unwrap();
    assert!(st != "recording" && st != "processing", "{list}");
    let d = a.note("note:get", json!({"id": id})).await;
    assert!(
        d["summary"]["llmError"].as_str().unwrap_or("").contains("503"),
        "{d}"
    );
    assert!(d["summary"]["summary"].is_string(), "on-device summary: {d}");

    llm.always(completion(&json!({
        "summary": "STUB-AI: the team agreed to ship the beta on Friday.",
        "actionItems": [{"text": "Ship the beta", "owner": "Ada"}],
        "qa": []
    })));
    a.note("note:resummarize", json!({"id": id})).await;
    let d = wait("AI summary", || async {
        let d = a.note("note:get", json!({"id": id})).await;
        d["summary"]["summary"]
            .as_str()
            .is_some_and(|s| s.contains("STUB-AI"))
            .then_some(d)
    })
    .await;
    assert!(d["summary"].get("llmError").is_none_or(Value::is_null), "{d}");
    assert_eq!(d["summary"]["actionItems"][0]["text"], "Ship the beta");
    a.quit().await;
}
