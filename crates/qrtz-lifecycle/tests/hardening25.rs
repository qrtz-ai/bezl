#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Hardening cycle 25 (N120; ADR 0080): `note:set-template` while the meeting's summary attempt is already running.
//! That attempt read the old template before the new one was stored, and `retry_step` leaves a running step
//! alone, so set-template used to wait for it and return the old template's summary as the new one.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::*;
use qrtz_model::*;
use qrtz_storage::Db;
use serde_json::json;

struct Quiet;
impl EventSink for Quiet {
    fn emit(&self, _: CoreEvent) {}
}
impl HelperPort for Quiet {
    fn send(&self, _: &HelperCommand) -> bool {
        true
    }
}

/// The real finalize/index; a summary that records the template it read at its start, and whose first attempt
/// waits for `release` (an on-device summary that takes a while).
struct Steps {
    core: CoreSteps<InlineDb>,
    db: InlineDb,
    seen: Mutex<Vec<Option<String>>>,
    release: tokio::sync::Semaphore,
}

#[async_trait]
impl StepExecutor for Steps {
    async fn finalize(&self, id: String, ctx: PipelineContext) -> Result<FinalizeOutcome, LifecycleError> {
        self.core.finalize(id, ctx).await
    }
    async fn summary(
        &self,
        id: String,
        req: SummaryRequest,
    ) -> Result<Option<MeetingSummary>, LifecycleError> {
        if req != SummaryRequest::Llm {
            return self.core.summary(id, req).await;
        }
        let template = self.db.0.lock().unwrap().notes().need(&id).unwrap().template;
        let first = {
            let mut s = self.seen.lock().unwrap();
            s.push(template.clone());
            s.len() == 1
        };
        if first {
            self.release.acquire().await.unwrap().forget();
        }
        let s = MeetingSummary {
            summary: format!("written for {}", template.as_deref().unwrap_or("general")),
            action_items: vec![],
            qa: vec![],
            provider: "test".into(),
            template,
            llm_error: None,
        };
        let db = self.db.0.lock().unwrap();
        db.notes().save_summary(&id, &s, 1).unwrap();
        db.notes().set_status(&id, MeetingStatus::Done).unwrap();
        Ok(Some(s))
    }
    async fn index(&self, id: String) -> Result<IndexOutcome, LifecycleError> {
        self.core.index(id).await
    }
    async fn resummarize(&self, id: String, t: Option<String>) -> Result<MeetingSummary, LifecycleError> {
        self.core.resummarize(id, t).await
    }
}

struct Rig {
    rt: NoteRuntime,
    steps: Arc<Steps>,
    _tmp: tempfile::TempDir,
}

async fn recording() -> (Rig, String) {
    let tmp = tempfile::tempdir().unwrap();
    let db = InlineDb::new(Db::open(tmp.path().join("qrtz.db")).unwrap());
    let steps = Arc::new(Steps {
        core: CoreSteps {
            db: InlineDb(db.0.clone()),
        },
        db: InlineDb(db.0.clone()),
        seen: Mutex::new(vec![]),
        release: tokio::sync::Semaphore::new(0),
    });
    let rec = tmp.path().join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let n = Arc::new(AtomicU64::new(0));
    let rt = NoteRuntime::new(RuntimeDeps {
        store: Arc::new(Store(InlineDb(db.0.clone()))),
        steps: steps.clone(),
        helper: Arc::new(Quiet),
        events: Arc::new(Quiet),
        config: Arc::new(RuntimeConfig::default),
        clock: Arc::new(SystemClock),
        ids: Arc::new(move || format!("id{}", n.fetch_add(1, Ordering::SeqCst) + 1)),
        random: Arc::new(|| 0.5),
        recordings_dir: rec.clone(),
        retry_scale: 0.01,
        fault_step: None,
        default_title: Arc::new(utc_recording_title),
        pending_delete: None,
        calendar: None,
    });
    rt.recover().await.unwrap();
    rt.helper_json(
        &json!({"type": "hello", "protocol": 1, "helperVersion": "test", "os": "macos", "features": []}),
    );
    rt.helper_json(&json!({"type": "permissions.status", "status": {
        "accessibility": "granted", "screen-recording": "granted", "microphone": "granted"}}));
    let wav = rec.join("id1.wav").to_string_lossy().to_string();
    let r2 = rt.clone();
    let start = tokio::spawn(async move { r2.start(Some("Weekly".into())).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    rt.helper_json(&json!({"type": "recording.started", "audioPath": wav}));
    tokio::time::timeout(Duration::from_secs(5), start)
        .await
        .expect("start acked")
        .unwrap()
        .unwrap();
    (Rig { rt, steps, _tmp: tmp }, wav)
}

async fn stop(r: &Rig, wav: &str) {
    let rt = &r.rt;
    let r2 = rt.clone();
    let stop = tokio::spawn(async move { r2.stop(StopReason::Manual).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    rt.helper_json(&json!({"type": "recording.stopped", "audioPath": wav, "durationMs": 4000}));
    tokio::time::timeout(Duration::from_secs(5), stop)
        .await
        .expect("stop acked")
        .unwrap()
        .unwrap();
}

async fn rig() -> Rig {
    let (r, wav) = recording().await;
    stop(&r, &wav).await;
    r
}

async fn until_summary(rt: &NoteRuntime, status: StepStatus) {
    for _ in 0..400 {
        if let Ok(Some(p)) = rt.pipeline("id1").await {
            if p.steps.summary.status == status {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("summary never reached {status:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n120_a_template_change_during_a_running_summary_returns_the_new_templates_summary() {
    let r = rig().await;
    until_summary(&r.rt, StepStatus::Running).await;
    let rt = r.rt.clone();
    let set = tokio::spawn(async move { rt.set_template("id1", "standup").await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!set.is_finished(), "set-template waits for the attempt in flight");
    r.steps.release.add_permits(1);
    let s = tokio::time::timeout(Duration::from_secs(10), set)
        .await
        .expect("set-template returns")
        .unwrap()
        .unwrap();
    assert_eq!(s.template.as_deref(), Some("standup"), "{s:?}");
    assert_eq!(s.summary, "written for standup");
    let seen = r.steps.seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![Some("general".into()), Some("standup".into())],
        "the old attempt, then one re-run"
    );
    // The stored summary is the new one too.
    let stored = r.rt.pipeline("id1").await.unwrap().unwrap();
    assert_eq!(stored.steps.summary.status, StepStatus::Done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n120_a_template_change_after_the_summary_settled_re_runs_once() {
    let r = rig().await;
    until_summary(&r.rt, StepStatus::Running).await;
    r.steps.release.add_permits(1);
    until_summary(&r.rt, StepStatus::Done).await;
    let s = tokio::time::timeout(Duration::from_secs(10), r.rt.set_template("id1", "one-on-one"))
        .await
        .expect("set-template returns")
        .unwrap();
    assert_eq!(s.template.as_deref(), Some("one-on-one"), "{s:?}");
    let seen = r.steps.seen.lock().unwrap().clone();
    assert_eq!(seen, vec![Some("general".into()), Some("one-on-one".into())]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n122_a_diarization_segment_with_a_huge_time_cannot_overflow_the_part_offset() {
    let (r, wav) = recording().await;
    // The contract only bounds these below; `de_ms` saturates 1e300 to i64::MAX, then the part offset is added.
    r.rt.helper_json(&json!({"type": "ax.activeSpeaker", "atMs": 1e300, "name": "Ada"}));
    for (start, end) in [(0.0, 1e300), (1e300, 1e301), (10.0, 900.0)] {
        r.rt.helper_json(
            &json!({"type": "diarization.segment", "source": "mic", "speaker": 1,
            "startMs": start, "endMs": end}),
        );
    }
    stop(&r, &wav).await;
    until_summary(&r.rt, StepStatus::Running).await;
    r.steps.release.add_permits(1);
    until_summary(&r.rt, StepStatus::Done).await;
}

#[test]
fn n122_default_names_never_overflow_on_huge_turns() {
    let t = |s: &str, a: i64, b: i64| SpeakerTurn {
        speaker: s.into(),
        start_ms: a,
        end_ms: b,
    };
    let turns = [
        t("mic:1", 0, i64::MAX),
        t("mic:1", 1, i64::MAX),
        t("mic:2", 0, 10),
    ];
    let names = qrtz_lifecycle::speakers::default_names(&turns);
    assert_eq!(names.get("mic:1").map(String::as_str), Some("me"));
}
