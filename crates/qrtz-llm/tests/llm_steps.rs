//! The summary / resummarize step against a mock OpenRouter (never the network): strict classification, the
//! deadline with late-reply discard (N1), after-200 failures (N8), capture notes from the pipeline row (N29/N39),
//! local-only and missing keys (on-device), key redaction.
mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Mock, Reply};
use qrtz_lifecycle::runtime::{CoreSteps, InlineDb, StepExecutor, SummaryRequest};
use qrtz_llm::{LlmConfig, LlmSteps};
use qrtz_model::*;
use qrtz_storage::Db;
use serde_json::json;

const KEY: &str = "sk-or-v1-secretsecret";
const REPLY: &str =
    r#"{"summary":"We ship Friday.","actionItems":[{"text":"Send notes","owner":"Ana"}],"qa":[]}"#;

fn seeded() -> InlineDb {
    let db = Db::open_in_memory().unwrap();
    let m: Meeting = serde_json::from_value(json!({
        "id": "m1", "title": "Standup", "platform": "zoom", "startedAt": 1, "status": "processing",
        "participants": [], "durationMs": 300000
    }))
    .unwrap();
    db.notes().upsert_meeting(&m).unwrap();
    let segs: Vec<Segment> = serde_json::from_value(json!([
        {"id": "s0", "startMs": 0, "endMs": 900, "text": "We ship on Friday after the review.", "speakerName": "Ana", "source": "live"},
        {"id": "s1", "startMs": 200000, "endMs": 201000, "text": "I'll send the notes by tomorrow.", "speakerName": "Ben", "source": "live"}
    ]))
    .unwrap();
    db.notes()
        .replace_segments("m1", SegmentSource::Live, &segs)
        .unwrap();
    let step = json!({"status": "done", "attempts": 1, "maxAttempts": 3});
    let p: PipelineState = serde_json::from_value(json!({
        "meetingId": "m1", "stopReason": "manual",
        "steps": {"finalize": step, "summary": {"status": "pending", "attempts": 0, "maxAttempts": 3}, "index": {"status": "pending", "attempts": 0, "maxAttempts": 3}},
        "ctx": {"startedAt": 1, "endedAt": 300001, "durationMs": 300000, "audioParts": [], "active": [], "helperCrashed": false},
        "updatedAt": 1,
        "capture": {
            "silentSpans": [{"source": "system", "startMs": 0, "endMs": 130000}],
            "lostSpans": [{"source": "mic", "startMs": 60000}]
        }
    }))
    .unwrap();
    db.lifecycle().save_pipeline(&p, false).unwrap();
    InlineDb::new(db)
}

struct Fx {
    db: InlineDb,
    mock: Mock,
    cfg: Arc<Mutex<LlmConfig>>,
    steps: LlmSteps<InlineDb>,
}

async fn fx(key: Option<&'static str>) -> Fx {
    let db = seeded();
    let mock = Mock::start().await;
    let cfg = Arc::new(Mutex::new(LlmConfig {
        base_url: Some(mock.base_url.clone()),
        ..Default::default()
    }));
    let c2 = cfg.clone();
    let secrets = move |n: &str| (n == "llm.openrouter").then(|| key.map(str::to_string)).flatten();
    let mut steps = LlmSteps::new(
        db.clone(),
        Arc::new(CoreSteps { db: db.clone() }),
        Arc::new(|| 7),
        Arc::new(move || c2.lock().unwrap().clone()),
        Arc::new(secrets),
    );
    steps.deadline = Duration::from_millis(300);
    Fx { db, mock, cfg, steps }
}

fn stored(db: &InlineDb) -> (Option<MeetingSummary>, MeetingStatus) {
    let g = db.0.lock().unwrap();
    (
        g.notes().get_summary("m1").unwrap(),
        g.notes().need("m1").unwrap().status,
    )
}

#[tokio::test]
async fn openrouter_summary_is_saved_with_capture_notes_in_the_prompt() {
    let f = fx(Some(KEY)).await;
    f.cfg.lock().unwrap().model = "anthropic/claude-haiku-5.5-custom".into();
    f.mock.push(Reply::content(REPLY));
    let s = f
        .steps
        .summary("m1".into(), SummaryRequest::Llm)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (s.provider.as_str(), s.template.as_deref()),
        ("openrouter", Some("general"))
    );
    assert_eq!(s.summary, "We ship Friday.");
    assert_eq!(stored(&f.db), (Some(s), MeetingStatus::Done));
    let req = &f.mock.seen()[0];
    assert_eq!(req.path, "/chat/completions");
    assert_eq!(req.headers["authorization"], format!("Bearer {KEY}"));
    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    assert_eq!(body["model"], "anthropic/claude-haiku-5.5-custom");
    let user = body["messages"][1]["content"].as_str().unwrap();
    // System silent 0:00–2:10, mic lost 1:00 to the end (5:00): both 1:00–2:10, then each side's rest.
    assert!(user.contains("Capture gaps:\n- 0:00–1:00: no audio from the other participants (the computer's sound)\n- 1:00–2:10: no audio at all (nothing was captured)\n- 2:10–5:00: no audio from this user's microphone\n"), "{user}");
}

#[tokio::test]
async fn strict_failures_keep_their_retry_class_and_save_nothing() {
    let f = fx(Some(KEY)).await;
    for (reply, code, retryable) in [
        (
            Reply::json(503, r#"{"error":{"message":"overloaded"}}"#),
            "E_PROVIDER",
            true,
        ),
        (
            Reply::json(401, r#"{"error":{"message":"bad key"}}"#),
            "E_PROVIDER",
            false,
        ),
        (
            Reply::json(200, r#"{"error":{"message":"upstream","code":502}}"#),
            "E_PROVIDER",
            true,
        ),
        (
            Reply::Full {
                status: 200,
                content_type: "text/html".into(),
                body: "<html>portal</html>".into(),
            },
            "E_PROVIDER",
            true,
        ),
        (
            Reply::Partial {
                body: format!(
                    "{{\"choices\":[{{\"message\":{{\"content\":{}}}}}]}}",
                    json!(REPLY)
                ),
                sent: 20,
            },
            "E_UNAVAILABLE",
            true,
        ),
        (Reply::content("not json at all"), "E_PROVIDER", true),
    ] {
        f.mock.push(reply);
        let e = f
            .steps
            .summary("m1".into(), SummaryRequest::Llm)
            .await
            .unwrap_err();
        assert_eq!((e.code.as_str(), e.retryable), (code, retryable), "{}", e.message);
        assert_eq!(stored(&f.db), (None, MeetingStatus::Processing));
    }
    // Retries exhausted: the runtime asks for the fallback, which saves the extractive summary with the error.
    let s = f
        .steps
        .summary(
            "m1".into(),
            SummaryRequest::Fallback {
                error: "LLM HTTP 503: overloaded".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (s.provider.as_str(), s.llm_error.as_deref()),
        ("extractive", Some("LLM HTTP 503: overloaded"))
    );
    assert_eq!(stored(&f.db).1, MeetingStatus::Done);
}

#[tokio::test]
async fn past_the_deadline_the_late_reply_is_discarded() {
    let f = fx(Some(KEY)).await;
    f.mock.push(Reply::Delay(
        Duration::from_millis(600),
        Box::new(Reply::content(REPLY)),
    ));
    let e = f
        .steps
        .summary("m1".into(), SummaryRequest::Llm)
        .await
        .unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.retryable, e.message.as_str()),
        ("E_TIMEOUT", true, "LLM summary deadline exceeded")
    );
    tokio::time::sleep(Duration::from_millis(500)).await; // the reply would have landed by now
    assert_eq!(stored(&f.db), (None, MeetingStatus::Processing));
    // resummarize: same deadline, the old summary stays.
    f.mock.push(Reply::content(REPLY));
    let first = f.steps.resummarize("m1".into(), None).await.unwrap();
    f.mock.push(Reply::Hang);
    let e = f
        .steps
        .resummarize("m1".into(), Some("sales".into()))
        .await
        .unwrap_err();
    assert_eq!(e.code, "E_TIMEOUT");
    assert_eq!(stored(&f.db).0, Some(first));
}

#[tokio::test]
async fn resummarize_is_not_strict_and_takes_the_template_through() {
    let f = fx(Some(KEY)).await;
    f.mock.push(Reply::json(500, "oops"));
    let s = f
        .steps
        .resummarize("m1".into(), Some("sales".into()))
        .await
        .unwrap();
    assert_eq!(s.provider, "extractive");
    assert_eq!(s.template.as_deref(), Some("sales"));
    assert_eq!(s.llm_error.as_deref(), Some("LLM HTTP 500: oops"));
    assert_eq!(stored(&f.db), (Some(s), MeetingStatus::Processing));
    f.mock.push(Reply::content(REPLY));
    let s = f.steps.resummarize("m1".into(), None).await.unwrap();
    assert_eq!(
        (s.provider.as_str(), s.template.as_deref()),
        ("openrouter", Some("sales"))
    );
    let sys = serde_json::from_str::<serde_json::Value>(&f.mock.seen()[1].body).unwrap()["messages"][0]
        ["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        sys.contains(qrtz_llm::template_by_id(Some("sales")).guidance),
        "{sys}"
    );
}

#[tokio::test]
async fn local_only_or_no_key_stays_on_device_without_a_request() {
    let f = fx(None).await;
    let s = f
        .steps
        .summary("m1".into(), SummaryRequest::Llm)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (s.provider.as_str(), s.llm_error.as_deref()),
        ("extractive", None)
    );
    let f = fx(Some(KEY)).await;
    f.cfg.lock().unwrap().local_only = true;
    let s = f
        .steps
        .summary("m1".into(), SummaryRequest::Llm)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.provider, "extractive");
    assert!(f.mock.seen().is_empty());
}

#[tokio::test]
async fn the_key_never_shows_in_an_error() {
    let f = fx(Some(KEY)).await;
    f.mock.push(Reply::json(
        400,
        format!(r#"{{"error":{{"message":"bad header Bearer {KEY}"}}}}"#),
    ));
    let e = f
        .steps
        .summary("m1".into(), SummaryRequest::Llm)
        .await
        .unwrap_err();
    assert!(
        !e.message.contains(KEY) && e.message.contains("[redacted]"),
        "{}",
        e.message
    );
    f.mock.push(Reply::json(
        400,
        format!(r#"{{"error":{{"message":"bad header Bearer {KEY}"}}}}"#),
    ));
    let s = f.steps.resummarize("m1".into(), None).await.unwrap();
    assert!(!s.llm_error.unwrap().contains(KEY));
    assert!(!format!("{:?}", qrtz_llm::Secret::new(KEY)).contains(KEY));
}
