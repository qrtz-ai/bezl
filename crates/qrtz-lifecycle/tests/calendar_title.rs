#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0217: a recording is named after the calendar event it overlaps, and linked to it.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qrtz_lifecycle::runtime::*;
use qrtz_model::*;
use qrtz_storage::Db;
use serde_json::json;

#[derive(Default)]
struct Cal {
    asked: Mutex<Vec<Option<String>>>,
    links: Mutex<Vec<(String, String)>>,
    none: bool,
}
impl CalendarLookup for Cal {
    fn event_at(&self, _now: EpochMs, platform: Option<&str>) -> Option<(String, String)> {
        self.asked.lock().unwrap().push(platform.map(str::to_owned));
        (!self.none).then(|| ("ev1".into(), "weekly sync".into()))
    }
    fn link(&self, meeting_id: &str, event_id: &str) {
        self.links
            .lock()
            .unwrap()
            .push((meeting_id.into(), event_id.into()));
    }
}

#[derive(Default)]
struct Views(Mutex<Vec<SessionView>>);
impl EventSink for Views {
    fn emit(&self, e: CoreEvent) {
        if let CoreEvent::Session(v) = e {
            self.0.lock().unwrap().push(v);
        }
    }
}

struct Helper;
impl HelperPort for Helper {
    fn send(&self, _: &HelperCommand) -> bool {
        true
    }
}

struct Rig {
    rt: NoteRuntime,
    rec: std::path::PathBuf,
    _views: Arc<Views>,
    _tmp: tempfile::TempDir,
}

fn rig(cal: Arc<Cal>) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let db = InlineDb::new(Db::open(tmp.path().join("qrtz.db")).unwrap());
    let rec = tmp.path().join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let n = Arc::new(AtomicU64::new(0));
    let views = Arc::new(Views::default());
    let rt = NoteRuntime::new(RuntimeDeps {
        store: Arc::new(Store(InlineDb(db.0.clone()))),
        steps: Arc::new(CoreSteps { db: InlineDb(db.0) }),
        helper: Arc::new(Helper),
        events: views.clone(),
        config: Arc::new(RuntimeConfig::default),
        clock: Arc::new(SystemClock),
        ids: Arc::new(move || format!("id{}", n.fetch_add(1, Ordering::SeqCst) + 1)),
        random: Arc::new(|| 0.5),
        recordings_dir: rec.clone(),
        retry_scale: 0.01,
        fault_step: None,
        default_title: Arc::new(utc_recording_title),
        pending_delete: None,
        calendar: Some(cal),
    });
    Rig {
        rt,
        rec,
        _views: views,
        _tmp: tmp,
    }
}

async fn ready(r: &Rig) {
    r.rt.recover().await.unwrap();
    r.rt.helper_json(
        &json!({"type": "hello", "protocol": 1, "helperVersion": "test", "os": "macos", "features": []}),
    );
    r.rt.helper_json(&json!({"type": "permissions.status", "status": {
        "accessibility": "granted", "screen-recording": "granted", "microphone": "granted"}}));
}

async fn start(r: &Rig, title: Option<&str>) {
    let wav = r.rec.join("id1.wav").to_string_lossy().to_string();
    let title = title.map(str::to_owned);
    let r2 = r.rt.clone();
    let start = tokio::spawn(async move { r2.start(title).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    r.rt.helper_json(&json!({"type": "recording.started", "audioPath": wav}));
    tokio::time::timeout(Duration::from_secs(5), start)
        .await
        .expect("start acked")
        .unwrap()
        .unwrap();
}

fn title(r: &Rig) -> String {
    let db = Db::open(r._tmp.path().join("qrtz.db")).unwrap();
    let m = db.notes().get_meeting("id1").unwrap().expect("meeting id1");
    m.title
}

#[tokio::test]
async fn an_untitled_recording_takes_the_events_title_and_links_it() {
    let cal = Arc::new(Cal::default());
    let r = rig(cal.clone());
    ready(&r).await;
    start(&r, None).await;
    assert_eq!(title(&r), "weekly sync");
    assert_eq!(
        *cal.links.lock().unwrap(),
        [("id1".to_string(), "ev1".to_string())]
    );
    assert_eq!(*cal.asked.lock().unwrap(), [None]);
}

#[tokio::test]
async fn an_explicit_title_wins_and_nothing_is_linked() {
    let cal = Arc::new(Cal::default());
    let r = rig(cal.clone());
    ready(&r).await;
    start(&r, Some("mine")).await;
    assert_eq!(title(&r), "mine");
    assert!(cal.asked.lock().unwrap().is_empty());
    assert!(cal.links.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_event_beats_the_ax_window_title_and_gets_the_platform() {
    let cal = Arc::new(Cal::default());
    let r = rig(cal.clone());
    ready(&r).await;
    r.rt.helper_json(&json!({"type": "meeting.detected", "key": "k", "title": "Zoom Meeting", "platform": "zoom", "app": "zoom.us"}));
    tokio::time::sleep(Duration::from_millis(50)).await;
    start(&r, None).await;
    assert_eq!(title(&r), "weekly sync");
    // Once at detection (the prompt's meeting identity), once at start (the title).
    assert_eq!(
        *cal.asked.lock().unwrap(),
        [Some("zoom".to_string()), Some("zoom".to_string())]
    );
}

#[tokio::test]
async fn no_event_keeps_the_old_fallbacks() {
    let cal = Arc::new(Cal {
        none: true,
        ..Default::default()
    });
    let r = rig(cal.clone());
    ready(&r).await;
    start(&r, None).await;
    assert!(title(&r).starts_with("Recording "), "{}", title(&r));
    assert!(cal.links.lock().unwrap().is_empty());
}
