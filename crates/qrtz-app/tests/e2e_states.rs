//! Phase 5 port of e2e/states.spec.ts "LLM failure falls back to on-device summary, Retry gets the AI summary"
//! (docs/e2e-parity.md row 32; "helper missing" is in e2e_shell.rs, "hundreds of meetings" in e2e_scale.rs).
//! ADR 0075: the model is on-device (Apple Intelligence through the helper; here the scripted helper's mock).
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};

/// The on-device model can't answer (busy through every attempt) → the meeting ends with the extractive summary
/// and the error; once the model answers, "summarize again" gets the on-device summary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_unavailable_falls_back_to_extractive_and_summarize_again_gets_the_on_device_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let off = tmp.path().join("off");
    std::fs::write(&off, b"").unwrap();
    let a = App::launch(
        &tmp.path().join("data"),
        Opts::default()
            .standup("0.02")
            .env("QRTZ_MOCK_SUMMARIZE", &format!("busy-while:{}", off.display())),
    )
    .await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    let (id, _) = record_until(&a, None, |t| segments(t) > 3).await;
    let d = wait("fallback", || async {
        let d = a.note("note:get", json!({"id": id})).await;
        d["summary"]["llmError"].is_string().then_some(d)
    })
    .await;
    let list = a.note("note:list", json!({})).await;
    let st = list[0]["meeting"]["status"].as_str().unwrap();
    assert!(st != "recording" && st != "processing", "{list}");
    assert!(
        d["summary"]["llmError"]
            .as_str()
            .unwrap()
            .starts_with("summary unavailable"),
        "{d}"
    );
    assert!(d["summary"]["summary"].is_string(), "extractive summary: {d}");

    std::fs::remove_file(&off).unwrap();
    a.note("note:resummarize", json!({"id": id})).await;
    let d = wait("on-device summary", || async {
        let d = a.note("note:get", json!({"id": id})).await;
        (d["summary"]["provider"] == "apple-intelligence").then_some(d)
    })
    .await;
    assert!(d["summary"].get("llmError").is_none_or(Value::is_null), "{d}");
    assert!(
        d["summary"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("mock on-device"),
        "{d}"
    );
    a.quit().await;
}
