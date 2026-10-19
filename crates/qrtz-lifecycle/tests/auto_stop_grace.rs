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

struct Quiet;
impl EventSink for Quiet {
    fn emit(&self, _: CoreEvent) {}
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

async fn rig() -> (NoteRuntime, Arc<Helper>, tempfile::TempDir) {
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
        events: Arc::new(Quiet),
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
        calendar: None,
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
    (rt, helper, tmp)
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

const _: () = assert!(AUTO_STOP_GRACE_MS >= 120_000.0, "the grace is at least two minutes");

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
