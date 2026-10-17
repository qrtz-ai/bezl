//! Cycle 22: behaviours whose only end-to-end test was a TS test deleted with Electron (`44f9da6`,
//! apps/desktop/src/main/note-service.hardening3.test.ts), now pinned on the Rust executor:
//! N13 (a full disk while the LLM summary is saved is retryable and saves nothing half-way; the retry saves it once
//! space frees up), N3 (the summary deadline sits inside the runtime's RPC timeout) and the 402 path (no silent model
//! swap). Mock OpenRouter only.
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Mock, Reply};
use qrtz_lifecycle::runtime::{CoreSteps, InlineDb, StepExecutor, SummaryRequest, SUMMARY_TIMEOUT_MS};
use qrtz_llm::{LlmConfig, LlmSteps, CHAT_DEADLINE_MS, SUMMARY_DEADLINE_MS};
use qrtz_model::*;
use qrtz_storage::Db;
use serde_json::json;

const KEY: &str = "sk-or-v1-secretsecret";

fn seeded() -> InlineDb {
    let db = Db::open_in_memory().unwrap();
    let m: Meeting = serde_json::from_value(json!({
        "id": "m1", "title": "Standup", "platform": "zoom", "startedAt": 1, "status": "processing",
        "participants": [], "durationMs": 60000
    }))
    .unwrap();
    db.notes().upsert_meeting(&m).unwrap();
    let segs: Vec<Segment> = serde_json::from_value(json!([
        {"id": "s0", "startMs": 0, "endMs": 900, "text": "We ship on Friday after the review.", "speakerName": "Ana", "source": "live"},
        {"id": "s1", "startMs": 2000, "endMs": 2900, "text": "I'll send the notes by tomorrow.", "speakerName": "Ben", "source": "live"}
    ]))
    .unwrap();
    db.notes()
        .replace_segments("m1", SegmentSource::Live, &segs)
        .unwrap();
    InlineDb::new(db)
}

async fn steps(db: &InlineDb) -> (LlmSteps<InlineDb>, Mock) {
    let mock = Mock::start().await;
    let base = mock.base_url.clone();
    let mut s = LlmSteps::new(
        db.clone(),
        Arc::new(CoreSteps { db: db.clone() }),
        Arc::new(|| 7),
        Arc::new(move || LlmConfig {
            base_url: Some(base.clone()),
            ..Default::default()
        }),
        Arc::new(|n: &str| (n == "llm.openrouter").then(|| KEY.to_string())),
    );
    s.deadline = Duration::from_secs(5);
    (s, mock)
}

/// A summary too big for the pages left (40 kB: overflow pages).
fn big_reply() -> Reply {
    let summary = "We ship Friday. ".repeat(2_500);
    Reply::content(&json!({"summary": summary, "actionItems": [], "qa": []}).to_string())
}

fn set_max_pages(db: &InlineDb, full: bool) {
    let g = db.0.lock().unwrap();
    let c = g.connection();
    let n: i64 = if full {
        c.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap()
    } else {
        1_073_741_823
    };
    c.execute_batch(&format!("PRAGMA max_page_count = {n}")).unwrap();
}

fn stored(db: &InlineDb) -> (Option<MeetingSummary>, MeetingStatus) {
    let g = db.0.lock().unwrap();
    (
        g.notes().get_summary("m1").unwrap(),
        g.notes().need("m1").unwrap().status,
    )
}

/// N13: the disk fills up while the LLM summary is saved. The step fails retryable (E_UNAVAILABLE, so the runtime
/// backs off and retries instead of failing the meeting), nothing is half-saved and the meeting is still processing;
/// once space frees up the retry saves the summary and finishes the meeting.
#[tokio::test]
async fn n13_a_full_disk_while_the_summary_is_saved_is_retryable_and_the_retry_saves_it() {
    let db = seeded();
    let (s, mock) = steps(&db).await;
    set_max_pages(&db, true);
    mock.push(big_reply());
    let e = s.summary("m1".into(), SummaryRequest::Llm).await.unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.retryable),
        ("E_UNAVAILABLE", true),
        "{}",
        e.message
    );
    assert!(e.message.contains("full"), "{}", e.message);
    assert!(!e.message.contains(KEY));
    assert_eq!(stored(&db), (None, MeetingStatus::Processing));
    // Space frees up; the runtime's retry is the same request again.
    set_max_pages(&db, false);
    mock.push(big_reply());
    let saved = s
        .summary("m1".into(), SummaryRequest::Llm)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.provider, "openrouter");
    assert_eq!(stored(&db), (Some(saved), MeetingStatus::Done));
    assert_eq!(mock.seen().len(), 2);
}

/// N13 for "summarize again": a full disk keeps the old summary (nothing half-written) and is retryable.
#[tokio::test]
async fn n13_resummarize_on_a_full_disk_keeps_the_old_summary() {
    let db = seeded();
    let (s, mock) = steps(&db).await;
    mock.push(Reply::content(r#"{"summary":"First.","actionItems":[],"qa":[]}"#));
    let first = s.resummarize("m1".into(), None).await.unwrap();
    set_max_pages(&db, true);
    mock.push(big_reply());
    let e = s.resummarize("m1".into(), None).await.unwrap_err();
    assert_eq!((e.code.as_str(), e.retryable), ("E_UNAVAILABLE", true));
    assert_eq!(stored(&db).0, Some(first));
}

/// N3: each executor deadline ends before the runtime's RPC timeout for that step, with room to save, so a slow
/// provider is always the executor's retryable E_TIMEOUT with nothing saved late, never an RPC timeout that leaves
/// a reply free to land afterwards. Chat uses the same deadline.
const _: () = assert!(SUMMARY_DEADLINE_MS + 5_000 <= SUMMARY_TIMEOUT_MS);
const _: () = assert!(CHAT_DEADLINE_MS == SUMMARY_DEADLINE_MS);

#[test]
fn n3_summary_and_chat_deadlines_sit_inside_the_rpc_timeout() {
    // Checked at compile time above; this names the behaviour in the test list.
    let [deadline, rpc] = std::hint::black_box([SUMMARY_DEADLINE_MS, SUMMARY_TIMEOUT_MS]);
    assert!(deadline < rpc, "{deadline} vs {rpc}");
}

/// No free-model fallback (decisions-parity gap 5; the TS "on 402" test went with Electron): a 402 is one
/// request to the configured model and a non-retryable provider error that says to add credits. The lifecycle's
/// fallback then saves the on-device summary carrying that error (the UI's "add credits" hint keys off
/// `needs_credits`), and "summarize again" once credits are added is a normal LLM summary.
#[tokio::test]
async fn a_402_is_an_error_with_retry_never_a_silent_model_swap() {
    let db = seeded();
    let (s, mock) = steps(&db).await;
    mock.push(Reply::json(
        402,
        r#"{"error":{"message":"Insufficient credits"}}"#,
    ));
    let e = s.summary("m1".into(), SummaryRequest::Llm).await.unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.retryable),
        ("E_PROVIDER", false),
        "{}",
        e.message
    );
    assert!(qrtz_llm::needs_credits(&e.message), "{}", e.message);
    let seen = mock.seen();
    assert_eq!(seen.len(), 1, "one request, no second model tried");
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(body["model"], qrtz_llm::OPENROUTER_DEFAULT_MODEL);
    assert_eq!(stored(&db), (None, MeetingStatus::Processing));

    let fallback = s
        .summary(
            "m1".into(),
            SummaryRequest::Fallback {
                error: e.message.clone(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_ne!(fallback.provider, "openrouter");
    assert!(qrtz_llm::needs_credits(
        fallback.llm_error.as_deref().unwrap_or("")
    ));
    assert_eq!(mock.seen().len(), 1, "the fallback is on-device");
    assert_eq!(stored(&db), (Some(fallback), MeetingStatus::Done));

    mock.push(Reply::content(
        r#"{"summary":"We ship Friday.","actionItems":[],"qa":[]}"#,
    ));
    let again = s.resummarize("m1".into(), None).await.unwrap();
    assert_eq!(again.provider, "openrouter");
    assert_eq!(again.llm_error, None);
    assert_eq!(stored(&db).0, Some(again));
}
