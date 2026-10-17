//! N123: a meeting's cached part summaries (N114) never outlive its delete. A delete (at once or scheduled with an
//! undo window) forgets them, a meeting pending delete never reads or writes them, and an attempt still running
//! across a delete doesn't store them again. Over a fake model that fails the 2nd of 2 map calls: after the
//! delete, summarizing the same id again must call the model for both parts (a cache hit would make it one).
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use qrtz_app::ports::{NoShell, RuntimePort};
use qrtz_app::runtime_port::{HelperAvailability, NoteRuntimePort};
use qrtz_app::summary::{OnDeviceError, OnDeviceModel, OnDeviceSummaries, Stage};
use qrtz_app::wiring::{boot, BootOptions, DevKnobs};
use qrtz_app::{ConfigStore, Events, SharedConfig};
use qrtz_lifecycle::runtime::{HelperCommand, HelperPort};
use qrtz_llm::prompt::{SummaryOptions, SummaryReply};
use qrtz_llm::steps::Loaded;
use qrtz_model::{Segment, SegmentSource};
use serde_json::json;

const ID: &str = "m1";

fn meeting() -> Loaded {
    meeting_of(2)
}

fn meeting_of(lines: i64) -> Loaded {
    Loaded {
        id: ID.into(),
        title: "Launch sync".into(),
        segments: (0..lines)
            .map(|i| Segment {
                id: format!("s{i}"),
                start_ms: i * 5_000,
                end_ms: i * 5_000 + 4_000,
                text: format!("utterance{i} about the launch plan in some detail"),
                speaker: None,
                speaker_name: Some("Ada".into()),
                source: SegmentSource::Live,
                confidence: None,
            })
            .collect(),
        opts: SummaryOptions::default(),
    }
}

/// Counts map calls; the 2nd map call of the whole run fails. `on_first_map` runs inside the first map call.
#[derive(Default)]
struct Model {
    maps: AtomicUsize,
    on_first_map: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Model {
    fn maps(&self) -> usize {
        self.maps.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl OnDeviceModel for Model {
    async fn summarize(&self, stage: Stage, _: String) -> Result<SummaryReply, OnDeviceError> {
        if stage == Stage::Map {
            let n = self.maps.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 1 {
                if let Some(f) = self.on_first_map.lock().unwrap().take() {
                    f();
                }
            }
            if n == 2 {
                return Err(OnDeviceError::Failed("model failed".into()));
            }
        }
        Ok(SummaryReply {
            summary: format!("{} summary.", stage.as_str()),
            action_items: vec![],
            qa: vec![],
            template: None,
        })
    }
}

/// Two map parts for [`meeting`].
fn two_parts(mut e: OnDeviceSummaries) -> OnDeviceSummaries {
    e.part_chars = 80;
    assert_eq!(e.parts(&meeting()).len(), 2, "the meeting must be two parts");
    e
}

/// First attempt: part 1 done (cached), part 2 fails.
async fn first_attempt_fails(e: &OnDeviceSummaries, m: &Model) {
    first_attempt_of(e, m, &meeting()).await;
}

async fn first_attempt_of(e: &OnDeviceSummaries, m: &Model, l: &Loaded) {
    assert!(e.summarize(l).await.is_err());
    assert_eq!(m.maps(), 2);
}

#[tokio::test]
async fn without_a_delete_the_retry_resumes_from_the_cache() {
    // The control: the cache N114 added is real, so the tests below fail without the N123 fix.
    let m = Arc::new(Model::default());
    let e = two_parts(OnDeviceSummaries::new(m.clone()));
    first_attempt_fails(&e, &m).await;
    assert_eq!(e.cached_meetings(), 1);
    e.summarize(&meeting()).await.unwrap();
    assert_eq!(m.maps() - 2, 1, "only part 2 is asked again");
}

#[tokio::test]
async fn a_deleted_meeting_summarized_again_calls_the_model_for_every_part() {
    let m = Arc::new(Model::default());
    let e = two_parts(OnDeviceSummaries::new(m.clone()));
    first_attempt_fails(&e, &m).await;
    e.forget(ID);
    assert_eq!(e.cached_meetings(), 0);
    e.summarize(&meeting()).await.unwrap();
    assert_eq!(m.maps() - 2, 2, "no part comes from the deleted meeting's cache");
}

#[tokio::test]
async fn a_meeting_pending_delete_neither_reads_nor_writes_the_cache() {
    // Pending while the first attempt runs, then undone: nothing was stored, so both parts are asked again.
    let m = Arc::new(Model::default());
    let pending = Arc::new(AtomicBool::new(true));
    let p = pending.clone();
    let e = two_parts(
        OnDeviceSummaries::new(m.clone())
            .with_pending_delete(Arc::new(move |id: &str| id == ID && p.load(Ordering::SeqCst))),
    );
    first_attempt_fails(&e, &m).await;
    assert_eq!(e.cached_meetings(), 0);
    pending.store(false, Ordering::SeqCst);
    e.summarize(&meeting()).await.unwrap();
    assert_eq!(m.maps() - 2, 2);
}

#[tokio::test]
async fn a_delete_that_lands_after_the_cache_was_filled_is_never_read() {
    // Cached first, then the delete is scheduled (pending): the retry must not read what was stored before.
    let m = Arc::new(Model::default());
    let pending = Arc::new(AtomicBool::new(false));
    let p = pending.clone();
    let e = two_parts(
        OnDeviceSummaries::new(m.clone())
            .with_pending_delete(Arc::new(move |id: &str| id == ID && p.load(Ordering::SeqCst))),
    );
    first_attempt_fails(&e, &m).await;
    assert_eq!(e.cached_meetings(), 1);
    pending.store(true, Ordering::SeqCst);
    e.summarize(&meeting()).await.unwrap();
    assert_eq!(m.maps() - 2, 2);
    assert_eq!(e.cached_meetings(), 0);
}

#[tokio::test]
async fn an_attempt_running_across_a_delete_does_not_store_the_deleted_meeting_parts() {
    let m = Arc::new(Model::default());
    let slot: Arc<OnceLock<Arc<OnDeviceSummaries>>> = Arc::default();
    let s = slot.clone();
    *m.on_first_map.lock().unwrap() = Some(Box::new(move || s.get().unwrap().forget(ID)));
    let e = Arc::new(two_parts(OnDeviceSummaries::new(m.clone())));
    let _ = slot.set(e.clone());
    first_attempt_fails(&e, &m).await;
    assert_eq!(
        e.cached_meetings(),
        0,
        "part 1 finished after the delete: not stored"
    );
    e.summarize(&meeting()).await.unwrap();
    assert_eq!(m.maps() - 2, 2);
}

struct NoHelper;
impl HelperPort for NoHelper {
    fn send(&self, _: &HelperCommand) -> bool {
        true
    }
}

/// The app's wiring: `boot` builds the engine with the runtime's tombstones, and `note:delete` forgets.
async fn through_note_delete(defer_ms: u64) {
    let dir = tempfile::tempdir().unwrap();
    let m = Arc::new(Model::default());
    let cfg: SharedConfig = ConfigStore::open(dir.path().join("config.json")).into();
    let booted = boot(BootOptions {
        data_dir: dir.path().join("data"),
        events: Events::new(),
        shell: Arc::new(NoShell),
        config: cfg,
        helper: Arc::new(NoHelper),
        steps: None,
        pending_delete: Some(Arc::new(qrtz_storage::PendingDeletes::new(dir.path().join("pd")))),
        clock: None,
        summarizer: Some(m.clone()),
        knobs: DevKnobs::default(),
    })
    .unwrap();
    let engine = booted.summaries.clone().expect("the app's engine");
    // The app's part size: the shortest meeting of two parts.
    let lines = (2..).find(|&n| engine.parts(&meeting_of(n)).len() == 2).unwrap();
    let l = meeting_of(lines);
    first_attempt_of(&engine, &m, &l).await;
    assert_eq!(engine.cached_meetings(), 1);
    let port = NoteRuntimePort::new(booted.runtime.clone(), HelperAvailability::Native)
        .forget_summaries_on_delete(booted.summaries.clone());
    port.call("note:delete", json!({"id": ID, "deferMs": defer_ms}))
        .await
        .unwrap();
    assert_eq!(engine.cached_meetings(), 0);
    assert_eq!(
        booted.runtime.pending_deletes().contains(&ID.to_string()),
        defer_ms > 0
    );
    engine.summarize(&l).await.unwrap();
    assert_eq!(
        m.maps() - 2,
        2,
        "the id came back: every part is summarized again"
    );
    assert_eq!(engine.cached_meetings(), 0);
}

#[tokio::test]
async fn note_delete_forgets_the_part_summaries() {
    through_note_delete(0).await;
}

#[tokio::test]
async fn note_delete_with_an_undo_window_forgets_them_and_the_tombstone_keeps_them_out() {
    through_note_delete(60_000).await;
}
