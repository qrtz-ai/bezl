#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Jared: flipping away from the meeting tab stopped a running recording (the helper ends a meeting ~4 s after its
//! tab loses focus). `meeting.ended` for the recorded meeting now only arms a long grace
//! (`AUTO_STOP_GRACE_MS` x `retry_scale`); the meeting coming back disarms it, and only a meeting that stays gone
//! for the whole grace stops the recording.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qrtz_lifecycle::runtime::*;
use qrtz_model::*;
use qrtz_storage::Db;
use serde_json::json;

/// `note:auto-stop` payloads, in order.
#[derive(Default)]
struct Sink(Mutex<Vec<serde_json::Value>>);
impl EventSink for Sink {
    fn emit(&self, e: CoreEvent) {
        if e.ipc_name() == Some("note:auto-stop") {
            self.0.lock().unwrap().push(e.payload());
        }
    }
}
impl Sink {
    fn states(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|v| v["state"].as_str().unwrap().to_string())
            .collect()
    }
}

/// The calendar event overlapping now (switchable).
#[derive(Default)]
struct Cal(Mutex<Option<String>>);
impl CalendarLookup for Cal {
    fn event_at(&self, _: EpochMs, _: Option<&str>) -> Option<(String, String)> {
        self.0
            .lock()
            .unwrap()
            .clone()
            .map(|id| (id, "Weekly sync".to_string()))
    }
    fn link(&self, _: &str, _: &str) {}
}

#[derive(Default)]
struct Helper(Mutex<Vec<String>>);
impl HelperPort for Helper {
    fn send(&self, c: &HelperCommand) -> bool {
        self.0.lock().unwrap().push(c.type_name().to_string());
        true
    }
}
impl Helper {
    fn stops(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|t| *t == "recording.stop")
            .count()
    }
}

/// Grace at this scale: 180 000 ms x 0.001 = 180 ms.
const SCALE: f64 = 0.001;
const GRACE: u64 = (AUTO_STOP_GRACE_MS * SCALE) as u64;

struct Rig {
    rt: NoteRuntime,
    helper: Arc<Helper>,
    sink: Arc<Sink>,
    cal: Arc<Cal>,
    _tmp: tempfile::TempDir,
}

async fn rig() -> (NoteRuntime, Arc<Helper>, tempfile::TempDir) {
    let r = rig_with(None).await;
    (r.rt, r.helper, r._tmp)
}

async fn rig_with(event: Option<&str>) -> Rig {
    let cal = Arc::new(Cal(Mutex::new(event.map(str::to_string))));
    let sink = Arc::new(Sink::default());
    let tmp = tempfile::tempdir().unwrap();
    let db = InlineDb::new(Db::open(tmp.path().join("qrtz.db")).unwrap());
    let rec = tmp.path().join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let n = Arc::new(AtomicU64::new(0));
    let helper = Arc::new(Helper::default());
    let rt = NoteRuntime::new(RuntimeDeps {
        store: Arc::new(Store(InlineDb(db.0.clone()))),
        steps: Arc::new(CoreSteps {
            db: InlineDb(db.0.clone()),
        }),
        helper: helper.clone(),
        events: sink.clone(),
        config: Arc::new(|| RuntimeConfig {
            auto_record: true,
            ..RuntimeConfig::default()
        }),
        clock: Arc::new(SystemClock),
        ids: Arc::new(move || format!("id{}", n.fetch_add(1, Ordering::SeqCst) + 1)),
        random: Arc::new(|| 0.5),
        recordings_dir: rec.clone(),
        retry_scale: SCALE,
        fault_step: None,
        default_title: Arc::new(utc_recording_title),
        pending_delete: None,
        calendar: event.map(|_| cal.clone() as Arc<dyn CalendarLookup>),
    });
    rt.recover().await.unwrap();
    rt.helper_json(
        &json!({"type": "hello", "protocol": 1, "helperVersion": "test", "os": "macos", "features": []}),
    );
    rt.helper_json(&json!({"type": "permissions.status", "status": {
        "accessibility": "granted", "screen-recording": "granted", "microphone": "granted"}}));
    detected(&rt);
    tokio::time::sleep(Duration::from_millis(30)).await;
    let path = rec.join("id1.wav");
    rt.helper_json(&json!({"type": "recording.started", "audioPath": path.to_string_lossy()}));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(rt.session().phase, RecordingPhase::Recording);
    Rig {
        rt,
        helper,
        sink,
        cal,
        _tmp: tmp,
    }
}

fn detected(rt: &NoteRuntime) {
    rt.helper_json(
        &json!({"type": "meeting.detected", "key": "meet:abc-defg-hij", "title": "Weekly sync",
        "platform": "google-meet", "app": "Google Meet (Chrome)"}),
    );
}
fn ended(rt: &NoteRuntime) {
    rt.helper_json(&json!({"type": "meeting.ended", "key": "meet:abc-defg-hij"}));
}
async fn ms(n: u64) {
    tokio::time::sleep(Duration::from_millis(n)).await;
}

const _: () = assert!(
    AUTO_STOP_GRACE_MS >= 120_000.0,
    "the grace is at least two minutes"
);

#[tokio::test]
async fn tab_flips_never_stop_a_recording() {
    let (rt, helper, _tmp) = rig().await;
    for _ in 0..6 {
        ended(&rt);
        ms(GRACE / 3).await;
        detected(&rt); // back on the tab before the grace ran out
        ms(GRACE / 3).await;
    }
    ms(GRACE * 2).await;
    assert_eq!(helper.stops(), 0);
    assert_eq!(rt.session().phase, RecordingPhase::Recording);
}

#[tokio::test]
async fn a_meeting_gone_for_less_than_the_grace_keeps_recording_then_stops_after_it() {
    let (rt, helper, _tmp) = rig().await;
    ended(&rt);
    ms(GRACE / 2).await;
    ended(&rt); // a repeat end doesn't push the deadline or stop early
    assert_eq!(helper.stops(), 0);
    assert_eq!(rt.session().phase, RecordingPhase::Recording);
    ms(GRACE).await;
    assert_eq!(helper.stops(), 1, "gone for the whole grace: auto-stop");
}

#[tokio::test]
async fn a_manual_stop_disarms_the_grace() {
    let (rt, helper, _tmp) = rig().await;
    ended(&rt);
    let r = rt.clone();
    tokio::spawn(async move { r.stop(StopReason::Manual).await });
    ms(GRACE * 2).await;
    assert_eq!(helper.stops(), 1, "only the manual stop");
}

/// Bozniak H1: a new key under the same calendar event joins the set; the recording stops once it ends too.
#[tokio::test]
async fn a_key_change_then_the_new_key_ending_stops() {
    let r = rig_with(Some("ev1")).await;
    detected_key(&r.rt, "meet:new-tab"); // same event, another tab
    ms(10).await;
    ended(&r.rt); // the original tab closes: meet:new-tab is still live
    ms(GRACE * 2).await;
    assert_eq!(r.helper.stops(), 0);
    assert!(
        r.sink.states().is_empty(),
        "nothing armed while a key of the meeting is live"
    );
    ended_key(&r.rt, "meet:new-tab");
    ms(10).await;
    assert_eq!(r.sink.states(), ["armed"]);
    ms(GRACE * 2).await;
    assert_eq!(r.helper.stops(), 1, "the last live key ended: stop");
}

/// Bozniak H2: the same `zoom-app:<pid>`-style key now naming the next calendar event doesn't extend the grace.
#[tokio::test]
async fn a_different_meeting_under_the_same_key_does_not_extend_it() {
    let r = rig_with(Some("ev1")).await;
    ended(&r.rt);
    ms(GRACE / 3).await;
    *r.cal.0.lock().unwrap() = Some("ev2".into());
    detected(&r.rt); // same key, the next meeting
    ms(GRACE).await;
    assert_eq!(r.helper.stops(), 1);
}

/// The countdown is shown (armed {deadlineAt}, cleared) and "keep recording" disarms it.
#[tokio::test]
async fn armed_and_cleared_are_emitted_and_keep_recording_disarms() {
    let r = rig_with(None).await;
    ended(&r.rt);
    ms(10).await;
    let armed = r.sink.0.lock().unwrap()[0].clone();
    assert_eq!(armed["state"], "armed");
    assert!(armed["deadlineAt"].as_i64().unwrap() > 0);
    detected(&r.rt);
    ms(10).await;
    assert_eq!(r.sink.states(), ["armed", "cleared"]);
    ended(&r.rt);
    ms(10).await;
    r.rt.keep_recording();
    ms(GRACE * 2).await;
    assert_eq!(r.sink.states(), ["armed", "cleared", "armed", "cleared"]);
    assert_eq!(r.helper.stops(), 0);
    assert_eq!(r.rt.session().phase, RecordingPhase::Recording);
}

fn detected_key(rt: &NoteRuntime, key: &str) {
    rt.helper_json(
        &json!({"type": "meeting.detected", "key": key, "title": "Weekly sync",
        "platform": "google-meet", "app": "Google Meet (Chrome)"}),
    );
}
fn ended_key(rt: &NoteRuntime, key: &str) {
    rt.helper_json(&json!({"type": "meeting.ended", "key": key}));
}
