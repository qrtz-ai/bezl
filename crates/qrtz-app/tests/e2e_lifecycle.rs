//! Phase 5 port of e2e/lifecycle.spec.ts (docs/e2e-parity.md rows 14-17): live transcript partials → finals and
//! after restart, permission denied, LLM failure → retries → fallback → manual retry, restart mid post-processing.
//! The scripted helper plays the standup (examples/e2e_helper.rs); the provider is a local mock OpenRouter.
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn stub_ai() -> String {
    completion(&json!({
        "summary": "STUB-AI: the team agreed to ship the beta on Friday.",
        "actionItems": [],
        "qa": []
    }))
}

async fn detected(a: &App) {
    a.until_session("detected", |s| s["phase"] == "detected").await;
}

/// lifecycle: a partial appears before its final and is replaced in place by it; the transcript grows; after stop
/// the persisted transcript (8 lines) is what the meeting opens on, also after a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_transcript_partials_then_finals_and_the_persisted_transcript_after_stop_and_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let a = App::launch(&data, Opts::default().standup("0.1")).await;
    let mut bus = a.app.service.events.subscribe();
    a.helper_ready().await;
    detected(&a).await;
    let s = a.note("note:start", json!({})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();

    // A partial, then the final with the same id replaces it.
    let (pid, first) = wait("a partial", || async {
        let t = a.note("note:live-transcript", json!({})).await;
        t["partial"]["id"].as_str().map(|p| {
            (
                p.to_owned(),
                t["partial"]["text"].as_str().unwrap_or("").to_owned(),
            )
        })
    })
    .await;
    let fin = wait("the partial finalized in place", || async {
        let t = a.note("note:live-transcript", json!({})).await;
        let seg = t["segments"]
            .as_array()
            .and_then(|s| s.iter().find(|x| x["id"] == pid.as_str()).cloned());
        (seg.is_some() && t["partial"]["id"] != pid.as_str()).then(|| seg.unwrap())
    })
    .await;
    assert!(
        fin["text"].as_str().unwrap().len() + 20 >= first.len(),
        "{fin} vs {first}"
    );
    // Text arrives incrementally: at least 3 distinct transcript sizes observed.
    let counts = std::sync::Mutex::new(BTreeSet::new());
    wait("growing transcript", || async {
        let n = segments(&a.note("note:live-transcript", json!({})).await);
        let mut c = counts.lock().unwrap();
        c.insert(n);
        (c.len() >= 3).then_some(())
    })
    .await;
    // The whole script plays; stop; the persisted transcript has 8 lines.
    wait("8 live lines", || async {
        (segments(&a.note("note:live-transcript", json!({})).await) == 8).then_some(())
    })
    .await;
    a.note("note:stop", Value::Null).await;
    a.until_phase("done").await;
    let d = a.note("note:get", json!({"id": id})).await;
    assert_eq!(d["segments"].as_array().unwrap().len(), 8, "{d}");
    let mut phases = vec![];
    while let Ok((name, p)) = bus.try_recv() {
        if name == "note:session" {
            phases.push(p["phase"].as_str().unwrap_or("").to_owned());
        }
    }
    for want in [
        "detected",
        "starting",
        "recording",
        "stopping",
        "processing",
        "done",
    ] {
        assert!(phases.iter().any(|p| p == want), "{want} in {phases:?}");
    }
    a.quit().await;

    // After a restart the full transcript is still what the meeting opens on.
    let b = App::launch(&data, Opts::default()).await;
    let list = b.note("note:list", json!({})).await;
    let first = list[0]["meeting"]["id"].as_str().unwrap().to_owned();
    assert_eq!(first, id);
    let d = b.note("note:get", json!({"id": first})).await;
    assert_eq!(d["segments"].as_array().unwrap().len(), 8, "{d}");
    b.quit().await;
}

/// lifecycle: screen recording + mic denied → the start is refused as a lifecycle error naming the permissions;
/// the phase is unchanged and no meeting is created; the error can be dismissed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permission_denied_refuses_the_start_as_lifecycle_state_and_creates_no_meeting() {
    let tmp = tempfile::tempdir().unwrap();
    let a = App::launch(
        &tmp.path().join("data"),
        Opts::default().standup("0.05").env(
            "QRTZ_MOCK_PERMISSIONS",
            "screen-recording=denied,microphone=denied",
        ),
    )
    .await;
    a.helper_ready().await;
    detected(&a).await;
    let r = a.raw("note", "note:start", json!({})).await;
    let s = a
        .until_session("lifecycle error", |s| s["error"].is_string())
        .await;
    let err = s["error"].as_str().unwrap();
    assert!(
        err.contains("Screen & System Audio Recording or Microphone"),
        "{err} / {r}"
    );
    assert_eq!(s["phase"], "detected", "{s}");
    assert_eq!(a.note("note:list", json!({})).await, json!([]));
    a.note("note:dismiss", Value::Null).await;
    a.until_session("error dismissed", |s| s["error"].is_null()).await;
    a.quit().await;
}

/// lifecycle: the provider answers 503 → the summary retries (maxAttempts = 3 requests), fails with the on-device
/// fallback and the 503; finalize is done; `note:retry-step summary` then gets the AI summary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn llm_failure_retries_falls_back_on_device_and_a_manual_step_retry_succeeds() {
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
    detected(&a).await;
    let s = a.note("note:start", json!({})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    wait("> 3 live lines", || async {
        (segments(&a.note("note:live-transcript", json!({})).await) > 3).then_some(())
    })
    .await;
    a.note("note:stop", Value::Null).await;
    let p = a.until_step(&id, "summary", "failed").await;
    assert_eq!(llm.hits(), 3, "maxAttempts");
    let step = &p["steps"]["summary"];
    assert_eq!(step["fallback"], true, "{p}");
    assert!(
        step["error"]["message"].as_str().unwrap_or("").contains("503"),
        "{p}"
    );
    assert_eq!(p["steps"]["finalize"]["status"], "done", "{p}");
    // The meeting still has the on-device summary.
    let d = a.note("note:get", json!({"id": id})).await;
    assert!(d["summary"]["summary"].is_string(), "{d}");

    llm.always(stub_ai());
    a.note("note:retry-step", json!({"id": id, "step": "summary"}))
        .await;
    a.until_step(&id, "summary", "done").await;
    let p = a.until_settled(&id).await;
    assert!(
        ["finalize", "summary", "index"]
            .iter()
            .all(|s| matches!(p["steps"][s]["status"].as_str(), Some("done" | "skipped"))),
        "{p}"
    );
    let d = a.note("note:get", json!({"id": id})).await;
    assert!(
        d["summary"]["summary"].as_str().unwrap().contains("STUB-AI"),
        "{d}"
    );
    assert!(d["summary"].get("llmError").is_none_or(Value::is_null), "{d}");
    a.quit().await;
}

/// lifecycle: quit while the summary waits between attempts (real backoff) → the next launch resumes the summary
/// from the persisted step: one more request, finalize not redone, the meeting ends done.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_restart_mid_post_processing_resumes_the_pipeline_from_the_persisted_step() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let llm = MockHttp::start().await;
    llm.always(overloaded());
    let a = App::launch(
        &data,
        Opts::default().standup("0.02").llm(&llm.base).retry_scale(1.0),
    )
    .await;
    a.settings(
        "secrets:set",
        json!({"name": "llm.openrouter", "value": "sk-test-stub"}),
    )
    .await;
    a.helper_ready().await;
    detected(&a).await;
    let s = a.note("note:start", json!({})).await;
    let id = s["meetingId"].as_str().unwrap().to_owned();
    wait("> 3 live lines", || async {
        (segments(&a.note("note:live-transcript", json!({})).await) > 3).then_some(())
    })
    .await;
    a.note("note:stop", Value::Null).await;
    let p = a.until_step(&id, "summary", "retrying").await;
    assert_eq!(p["steps"]["finalize"]["status"], "done", "{p}");
    let finalized_at = p["steps"]["finalize"]["finishedAt"].clone();
    a.quit().await; // between attempts
    let before = llm.hits();

    llm.always(stub_ai());
    let b = App::launch(&data, Opts::default().llm(&llm.base)).await;
    let p = b.until_settled(&id).await;
    assert_eq!(p["steps"]["summary"]["status"], "done", "{p}");
    assert_eq!(
        p["steps"]["finalize"]["finishedAt"], finalized_at,
        "finalize redone: {p}"
    );
    let d = b.note("note:get", json!({"id": id})).await;
    assert!(
        d["summary"]["summary"].as_str().unwrap().contains("STUB-AI"),
        "{d}"
    );
    assert_eq!(llm.hits(), before + 1, "resumed once");
    let list = b.note("note:list", json!({})).await;
    assert_eq!(list[0]["meeting"]["status"], "done", "{list}");
    b.quit().await;
}
