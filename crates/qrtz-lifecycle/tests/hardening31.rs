#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Hardening cycle 31: the order of `note:session` views (N155, N156, ADR 0098) over the real `NoteRuntime`.
//!
//! A window subscribes to `note:session`, then asks for a snapshot, and keeps whichever view has the higher `seq`
//! (`sessionIsNewer`). That only works if (1) every view carries a seq, including the event each lifecycle
//! transition broadcasts, and (2) a higher seq is never an older state. Before N156 the transition events (built in
//! `send`) carried none, so the "idle" a window got as a meeting stopped was unordered and a slower "recording"
//! snapshot still replaced it; and a snapshot drew its seq after releasing the state lock, so a view built before a
//! transition could still get the later number.
//!
//! Every test runs in its own `tempfile` dir, removed when the rig drops.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qrtz_lifecycle::runtime::*;
use qrtz_model::*;
use qrtz_storage::Db;
use serde_json::json;

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
    views: Arc<Views>,
    _tmp: tempfile::TempDir,
}

fn rig() -> Rig {
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
        calendar: None,
    });
    Rig {
        rt,
        rec,
        views,
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

async fn start(r: &Rig) {
    let wav = r.rec.join("id1.wav").to_string_lossy().to_string();
    let r2 = r.rt.clone();
    let start = tokio::spawn(async move { r2.start(Some("Weekly".into())).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    r.rt.helper_json(&json!({"type": "recording.started", "audioPath": wav}));
    tokio::time::timeout(Duration::from_secs(5), start)
        .await
        .expect("start acked")
        .unwrap()
        .unwrap();
}

async fn stop(r: &Rig) {
    let r2 = r.rt.clone();
    let stop = tokio::spawn(async move { r2.stop(StopReason::Manual).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let wav = r.rec.join("id1.wav").to_string_lossy().to_string();
    r.rt.helper_json(&json!({"type": "recording.stopped", "audioPath": wav, "durationMs": 1000}));
    tokio::time::timeout(Duration::from_secs(10), stop)
        .await
        .expect("stop returns")
        .unwrap()
        .unwrap();
}

/// The pipeline finishes (`done`), then the window dismisses it (`idle`).
async fn idle_again(r: &Rig) {
    for _ in 0..1000 {
        if r.rt.session().phase == RecordingPhase::Done {
            let v = r.rt.dismiss();
            assert_eq!(v.phase, RecordingPhase::Idle);
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never done: {:?}", r.rt.session().phase);
}

/// How far a view is through one meeting: never lower for a later state.
fn rank(v: &SessionView, seen_meeting: &mut bool) -> u8 {
    let r = match v.phase {
        RecordingPhase::Idle | RecordingPhase::Detected => {
            if *seen_meeting {
                6
            } else {
                0
            }
        }
        RecordingPhase::Starting => 1,
        RecordingPhase::Recording | RecordingPhase::Reconnecting => 2,
        RecordingPhase::Stopping => 3,
        RecordingPhase::Processing => 4,
        RecordingPhase::Done | RecordingPhase::Error => 5,
    };
    if r > 0 {
        *seen_meeting = true;
    }
    r
}

/// N156 (the N155 case end to end): a window takes its snapshot while recording, the meeting stops before the
/// snapshot is applied. Every transition event carries a seq, the stop's events are all newer than the snapshot,
/// so `sessionIsNewer(idle event, recording snapshot)` keeps idle.
#[tokio::test]
async fn every_transition_event_is_ordered_after_an_earlier_snapshot() {
    let r = rig();
    ready(&r).await;
    start(&r).await;
    let snapshot = r.rt.session();
    assert_eq!(snapshot.phase, RecordingPhase::Recording);
    assert!(snapshot.seq > 0, "a snapshot carries a seq");
    stop(&r).await;
    idle_again(&r).await;
    let views = r.views.0.lock().unwrap().clone();
    assert!(!views.is_empty());
    for v in &views {
        assert!(
            v.seq > 0,
            "a note:session event without seq (unordered): {:?}",
            v.phase
        );
    }
    for w in views.windows(2) {
        assert!(
            w[1].seq > w[0].seq,
            "events out of seq: {} then {}",
            w[0].seq,
            w[1].seq
        );
    }
    let last = views.last().unwrap();
    assert_eq!(last.phase, RecordingPhase::Idle);
    assert!(
        last.seq > snapshot.seq,
        "the idle event ({}) must beat the older recording snapshot ({})",
        last.seq,
        snapshot.seq
    );
    // The JSON a window gets has it too (omitted only when 0).
    let j = serde_json::to_value(last).unwrap();
    assert!(j["seq"].as_u64().unwrap() > 0);
}

/// N156: snapshots taken from other threads while the meeting starts and stops. Sorted by seq, every view (events
/// and snapshots together) is a forward path through the meeting: a higher seq is never an older state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_snapshots_never_get_a_later_seq_than_a_newer_state() {
    let r = rig();
    ready(&r).await;
    let done = Arc::new(AtomicBool::new(false));
    let snaps = Arc::new(Mutex::new(Vec::<SessionView>::new()));
    let mut pollers = Vec::new();
    for _ in 0..3 {
        let (rt, done, snaps) = (r.rt.clone(), done.clone(), snaps.clone());
        pollers.push(std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let v = rt.session();
                snaps.lock().unwrap().push(v);
            }
        }));
    }
    start(&r).await;
    r.rt.helper_json(
        &json!({"type": "asr", "event": {"kind": "final", "utteranceId": "u1", "startMs": 0,
        "endMs": 400, "text": "the budget line"}}),
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop(&r).await;
    idle_again(&r).await;
    done.store(true, Ordering::SeqCst);
    for p in pollers {
        p.join().unwrap();
    }
    let mut all = r.views.0.lock().unwrap().clone();
    all.extend(snaps.lock().unwrap().drain(..));
    assert!(all.len() > 10, "the pollers ran");
    all.sort_by_key(|v| v.seq);
    let mut seen = false;
    let mut prev = (0u8, 0u64);
    for v in &all {
        assert!(v.seq > 0, "a view without seq: {:?}", v.phase);
        let k = rank(v, &mut seen);
        assert!(
            k >= prev.0,
            "seq {} is {:?} (rank {k}) after seq {} at rank {}",
            v.seq,
            v.phase,
            prev.1,
            prev.0
        );
        prev = (k, v.seq);
    }
    let mut seqs: Vec<u64> = all.iter().map(|v| v.seq).collect();
    seqs.dedup();
    assert_eq!(seqs.len(), all.len(), "every view gets its own seq");
}
