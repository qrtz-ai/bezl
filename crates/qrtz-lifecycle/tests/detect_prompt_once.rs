//! Jared: the "meeting detected, record?" prompt fired on every tab flip back to the meeting. Once per meeting.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qrtz_lifecycle::runtime::*;
use qrtz_storage::Db;
use serde_json::json;

#[derive(Default)]
struct Prompts(Mutex<Vec<String>>);
impl EventSink for Prompts {
    fn emit(&self, e: CoreEvent) {
        if let CoreEvent::DetectedPrompt { title, .. } = e {
            self.0.lock().unwrap().push(title);
        }
    }
}

struct Helper;
impl HelperPort for Helper {
    fn send(&self, _: &HelperCommand) -> bool {
        true
    }
}

async fn rig() -> (NoteRuntime, Arc<Prompts>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db = InlineDb::new(Db::open(tmp.path().join("qrtz.db")).unwrap());
    let rec = tmp.path().join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let n = Arc::new(AtomicU64::new(0));
    let prompts = Arc::new(Prompts::default());
    let rt = NoteRuntime::new(RuntimeDeps {
        store: Arc::new(Store(InlineDb(db.0.clone()))),
        steps: Arc::new(CoreSteps {
            db: InlineDb(db.0.clone()),
        }),
        helper: Arc::new(Helper),
        events: prompts.clone(),
        config: Arc::new(RuntimeConfig::default),
        clock: Arc::new(SystemClock),
        ids: Arc::new(move || format!("id{}", n.fetch_add(1, Ordering::SeqCst) + 1)),
        random: Arc::new(|| 0.5),
        recordings_dir: rec,
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
    (rt, prompts, tmp)
}

fn detected(rt: &NoteRuntime, key: &str, title: &str) {
    rt.helper_json(&json!({"type": "meeting.detected", "key": key, "title": title,
        "platform": "google-meet", "app": "Google Meet (Chrome)"}));
}

#[tokio::test]
async fn tab_flips_on_one_meeting_prompt_once_and_another_meeting_prompts_again() {
    let (rt, prompts, _tmp) = rig().await;
    for _ in 0..5 {
        detected(&rt, "meet:abc-defg-hij", "Weekly Product Sync");
        tokio::time::sleep(Duration::from_millis(20)).await;
        rt.helper_json(&json!({"type": "meeting.ended", "key": "meet:abc-defg-hij"}));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(*prompts.0.lock().unwrap(), ["Weekly Product Sync"]);
    detected(&rt, "meet:xyz-wxyz-xyz", "1:1");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(*prompts.0.lock().unwrap(), ["Weekly Product Sync", "1:1"]);
}
