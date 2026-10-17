//! Hardening cycle 26: the host side of the helper's N121 diarizer bound (ADR 0079 amended, `16e60ad`).
//! A channel whose diarizer fell 30 s behind stands down for the rest of the recording (turns already sent stay,
//! its open turn is not sent); at stop a diarizer still busy is closed out (its open turns sent before
//! `recording.stopped`); a cancelled diarizer task must never reach the host after the ack. The host has no
//! "stood down" message (only a helper log line), so these pin what finalize does with turns that simply end:
//! later lines of that channel keep no speaker (never the last speaker carried forward), the other channel is
//! untouched, close-out turns sent before the ack count, and turns after the stop completed never reach that
//! meeting or the next one.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use qrtz_lifecycle::runtime::*;
use qrtz_model::*;
use qrtz_storage::Db;
use serde_json::{json, Value};

struct Quiet;
impl EventSink for Quiet {
    fn emit(&self, _: CoreEvent) {}
}
impl HelperPort for Quiet {
    fn send(&self, _: &HelperCommand) -> bool {
        true
    }
}

struct Rig {
    rt: NoteRuntime,
    db: InlineDb,
    rec: std::path::PathBuf,
    _tmp: tempfile::TempDir,
}

fn rig() -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let db = InlineDb::new(Db::open(tmp.path().join("qrtz.db")).unwrap());
    let rec = tmp.path().join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let n = Arc::new(AtomicU64::new(0));
    let rt = NoteRuntime::new(RuntimeDeps {
        store: Arc::new(Store(InlineDb(db.0.clone()))),
        steps: Arc::new(CoreSteps {
            db: InlineDb(db.0.clone()),
        }),
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
    });
    Rig {
        rt,
        db,
        rec,
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

/// Starts a recording whose WAV is `<id>.wav`; returns the WAV path.
async fn start(r: &Rig, id: &str) -> String {
    let wav = r.rec.join(format!("{id}.wav")).to_string_lossy().to_string();
    let r2 = r.rt.clone();
    let start = tokio::spawn(async move { r2.start(Some("Weekly".into())).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    r.rt.helper_json(&json!({"type": "recording.started", "audioPath": wav}));
    tokio::time::timeout(Duration::from_secs(5), start)
        .await
        .expect("start acked")
        .unwrap()
        .unwrap();
    wav
}

/// Sends `before_ack` while the stop waits for the helper (stopping), then the ack, then `after_ack`.
async fn stop(r: &Rig, wav: &str, before_ack: &[Value], after_ack: &[Value]) {
    let r2 = r.rt.clone();
    let stop = tokio::spawn(async move { r2.stop(StopReason::Manual).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    for m in before_ack {
        r.rt.helper_json(m);
    }
    r.rt.helper_json(&json!({"type": "recording.stopped", "audioPath": wav, "durationMs": 60_000}));
    for m in after_ack {
        r.rt.helper_json(m);
    }
    tokio::time::timeout(Duration::from_secs(5), stop)
        .await
        .expect("stop acked")
        .unwrap()
        .unwrap();
}

fn line(id: &str, a: i64, b: i64) -> Value {
    json!({"type": "asr", "event": {"kind": "final", "utteranceId": id, "startMs": a, "endMs": b,
        "text": format!("words of {id}")}})
}

fn turn(source: &str, speaker: u32, a: i64, b: i64) -> Value {
    json!({"type": "diarization.segment", "source": source, "speaker": speaker, "startMs": a, "endMs": b})
}

async fn finalized(r: &Rig, id: &str) -> Vec<Segment> {
    for _ in 0..500 {
        if let Ok(Some(p)) = r.rt.pipeline(id).await {
            if p.steps.finalize.status == StepStatus::Done {
                return r.db.0.lock().unwrap().notes().get_segments(id).unwrap();
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} never finalized");
}

/// By the segment's text (ids are rewritten at finalize): (text tag, speaker).
fn speakers(segs: &[Segment]) -> Vec<(String, Option<String>)> {
    segs.iter()
        .map(|s| {
            (
                s.text.trim_start_matches("words of ").to_string(),
                s.speaker.clone(),
            )
        })
        .collect()
}

fn speaker_of(segs: &[Segment], tag: &str) -> Option<String> {
    speakers(segs)
        .into_iter()
        .find(|(t, _)| t == tag)
        .unwrap_or_else(|| panic!("no line {tag} in {:?}", speakers(segs)))
        .1
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n121_a_channel_that_stood_down_leaves_its_later_lines_without_a_speaker() {
    let r = rig();
    ready(&r).await;
    let wav = start(&r, "id1").await;
    // The system diarizer keeps up for 25 s, then falls 30 s behind and stands down: no more system turns, and its
    // open turn (system:1 from 20 s) is never sent. The mic diarizer keeps up all along.
    for m in [
        turn("system", 0, 0, 20_000),
        line("system-u1", 1_000, 5_000),
        turn("mic", 0, 6_000, 9_000),
        line("mic-u2", 6_500, 8_500),
        line("system-u3", 21_000, 24_000),
        line("system-u4", 40_000, 45_000),
        turn("mic", 0, 50_000, 54_000),
        line("mic-u5", 50_500, 53_000),
        line("system-u6", 55_000, 58_000),
    ] {
        r.rt.helper_json(&m);
    }
    stop(&r, &wav, &[], &[]).await;
    let segs = finalized(&r, "id1").await;
    assert_eq!(speaker_of(&segs, "system-u1").as_deref(), Some("system:0"));
    assert_eq!(speaker_of(&segs, "mic-u2").as_deref(), Some("mic:0"));
    assert_eq!(
        speaker_of(&segs, "mic-u5").as_deref(),
        Some("mic:0"),
        "the mic channel is untouched"
    );
    for tag in ["system-u3", "system-u4", "system-u6"] {
        assert_eq!(
            speaker_of(&segs, tag),
            None,
            "{tag}: no turn after the stand-down, so no speaker: {segs:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n121_close_out_turns_before_the_ack_count_and_turns_after_the_stop_completed_do_not() {
    let r = rig();
    ready(&r).await;
    let wav = start(&r, "id1").await;
    for m in [
        turn("system", 0, 0, 10_000),
        line("system-u1", 1_000, 4_000),
        line("system-u2", 30_000, 34_000),
        line("system-u3", 50_000, 54_000),
    ] {
        r.rt.helper_json(&m);
    }
    // Stop: the diarizer is still busy after 3 s, so it is closed out: its tracker's open turn (system:1, up to the
    // audio it processed) is sent before `recording.stopped`. The host takes turns until the stop completes
    // (phase stopping, ADR 0081), so close-out turns always count.
    stop(&r, &wav, &[turn("system", 1, 28_000, 36_000)], &[]).await;
    // The cancelled model call returns later; if the helper's run guard ever let it through, the stop is over and
    // the meeting's turns are already in its pipeline: dropped.
    r.rt.helper_json(&turn("system", 2, 48_000, 60_000));
    r.rt.helper_json(&turn("mic", 0, 0, 60_000));
    let segs = finalized(&r, "id1").await;
    assert_eq!(speaker_of(&segs, "system-u1").as_deref(), Some("system:0"));
    assert_eq!(
        speaker_of(&segs, "system-u2").as_deref(),
        Some("system:1"),
        "close-out turn kept: {segs:?}"
    );
    assert_eq!(
        speaker_of(&segs, "system-u3"),
        None,
        "a turn after the stop is dropped: {segs:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n121_back_to_back_meetings_never_share_turns() {
    let r = rig();
    ready(&r).await;
    let wav1 = start(&r, "id1").await;
    for m in [turn("system", 0, 0, 10_000), line("system-u1", 1_000, 4_000)] {
        r.rt.helper_json(&m);
    }
    stop(&r, &wav1, &[], &[]).await;
    // A late turn from the first recording's cancelled diarizer arrives after its stop, before the next start.
    r.rt.helper_json(&turn("system", 3, 0, 60_000));
    let wav2 = start(&r, "id2").await;
    for m in [
        line("system-u2", 1_000, 4_000),
        turn("mic", 1, 1_000, 4_000),
        line("mic-u3", 1_500, 3_500),
    ] {
        r.rt.helper_json(&m);
    }
    stop(&r, &wav2, &[], &[]).await;
    let s1 = finalized(&r, "id1").await;
    let s2 = finalized(&r, "id2").await;
    assert_eq!(speaker_of(&s1, "system-u1").as_deref(), Some("system:0"));
    assert!(
        s1.iter()
            .chain(&s2)
            .all(|s| s.speaker.as_deref() != Some("system:3")),
        "{s1:?} {s2:?}"
    );
    assert_eq!(
        speaker_of(&s2, "system-u2"),
        None,
        "the first meeting's turns never reach the second: {s2:?}"
    );
    assert_eq!(speaker_of(&s2, "mic-u3").as_deref(), Some("mic:1"));
}
