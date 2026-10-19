//! Hardening cycle 27, N126 host side (ADR 0085). Since `a4ea467` the helper says when a channel's diarizer stood
//! down (`backlog`) or never loaded (`model-unavailable`): `diarization.unavailable {source, atMs, reason}`, after the
//! open turn it could still send. The host ignored the line (no `HelperInput` arm), so a stood-down channel still
//! looked like a quiet one. Now: the meeting's capture record keeps it once per channel per part, in meeting time
//! (`CaptureReport.speakersUnavailable`, IPC contract regenerated); a `diarization.segment` on that channel after it,
//! in the same part, is dropped; nothing is kept outside a recording or reaches the next meeting.
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
        calendar: None,
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

fn down(source: &str, at: i64, reason: &str) -> Value {
    json!({"type": "diarization.unavailable", "source": source, "atMs": at, "reason": reason})
}

async fn finalized(r: &Rig, id: &str) -> (PipelineView, Vec<Segment>) {
    for _ in 0..500 {
        if let Ok(Some(p)) = r.rt.pipeline(id).await {
            if p.steps.finalize.status == StepStatus::Done {
                let segs = r.db.0.lock().unwrap().notes().get_segments(id).unwrap();
                return (p, segs);
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} never finalized");
}

fn speaker_of(segs: &[Segment], tag: &str) -> Option<String> {
    segs.iter()
        .find(|s| s.text == format!("words of {tag}"))
        .unwrap_or_else(|| panic!("no segment {tag}"))
        .speaker
        .clone()
}

/// The line is the contract's (the production parser takes it) and the runtime's.
#[test]
fn the_line_parses_through_the_contract_and_into_the_runtime() {
    for (src, reason) in [("system", "backlog"), ("mic", "model-unavailable")] {
        let line = serde_json::to_string(&down(src, 1200, reason)).unwrap();
        let v = qrtz_helper_bridge::parse_helper_line(&line).expect("contract-valid");
        let m = HelperInput::from_json(&v).expect("the runtime consumes it");
        assert_eq!(m.type_name(), "diarization.unavailable");
    }
    for bad in [
        json!({"type": "diarization.unavailable", "source": "both", "atMs": 0, "reason": "backlog"}),
        json!({"type": "diarization.unavailable", "source": "mic", "atMs": -1, "reason": "backlog"}),
        json!({"type": "diarization.unavailable", "source": "mic", "atMs": 0, "reason": "tired"}),
    ] {
        let line = serde_json::to_string(&bad).unwrap();
        assert!(qrtz_helper_bridge::parse_helper_line(&line).is_err(), "{line}");
    }
}

/// System audio's diarizer stands down at 25 s, the mic's model never loaded. The record says both, once each, in
/// meeting time; a system turn after the stand-down (a helper bug) is dropped; the mic line keeps no speaker; the
/// pipeline still matches the IPC contract and its JSON is the camelCase the renderer reads.
#[tokio::test]
async fn a_stand_down_reaches_the_meeting_record_and_later_turns_are_dropped() {
    let r = rig();
    ready(&r).await;
    let wav = start(&r, "id1").await;
    for m in [
        down("mic", 0, "model-unavailable"),
        line("system-u1", 1_000, 5_000),
        turn("system", 0, 1_000, 5_000),
        line("mic-u1", 6_000, 8_000),
        // Close-out turn of the stand-down, then the line itself (helper order, `a4ea467`).
        turn("system", 1, 20_000, 25_000),
        down("system", 25_000, "backlog"),
        down("system", 26_000, "backlog"), // once per channel per part
        line("system-u2", 30_000, 34_000),
        turn("system", 0, 30_000, 34_000), // after its stand-down: dropped
        down("mic", 3_000, "backlog"),     // already down this part
    ] {
        r.rt.helper_json(&m);
    }
    stop(&r, &wav, &[], &[]).await;
    let (p, segs) = finalized(&r, "id1").await;
    let cap = p.capture.clone().expect("capture record");
    assert_eq!(
        cap.speakers_unavailable,
        vec![
            SpeakersUnavailable {
                source: CaptureSource::Mic,
                at_ms: 0,
                reason: DiarizationUnavailableReason::ModelUnavailable,
            },
            SpeakersUnavailable {
                source: CaptureSource::System,
                at_ms: 25_000,
                reason: DiarizationUnavailableReason::Backlog,
            },
        ]
    );
    assert_eq!(speaker_of(&segs, "system-u1").as_deref(), Some("system:0"));
    assert_eq!(
        speaker_of(&segs, "system-u2"),
        None,
        "a turn after the stand-down was used"
    );
    assert_eq!(speaker_of(&segs, "mic-u1"), None);
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(
        v["capture"]["speakersUnavailable"][1],
        json!({"source": "system", "atMs": 25_000, "reason": "backlog"})
    );
    qrtz_ipc::validate_event("note:pipeline", &v).expect("note:pipeline payload is contract-valid");
}

/// A line outside a recording, or after its stop completed, is kept nowhere and never reaches the next meeting; a
/// meeting without one has no field at all (older records and the TS side unchanged).
#[tokio::test]
async fn nothing_is_kept_outside_a_recording_or_carried_to_the_next() {
    let r = rig();
    ready(&r).await;
    r.rt.helper_json(&down("system", 1_000, "backlog")); // idle
    let wav = start(&r, "id1").await;
    r.rt.helper_json(&line("system-u1", 1_000, 5_000));
    r.rt.helper_json(&down("system", 4_000, "backlog"));
    stop(&r, &wav, &[], &[down("mic", 2_000, "backlog")]).await;
    let (p1, _) = finalized(&r, "id1").await;
    r.rt.helper_json(&down("mic", 9_000, "backlog")); // after the stop returned
    assert_eq!(
        p1.capture.unwrap().speakers_unavailable.len(),
        2,
        "the stop takes lines until it completes (ADR 0082), so the mic line before it ended counts"
    );
    let wav2 = start(&r, "id2").await;
    r.rt.helper_json(&line("system-u1", 1_000, 5_000));
    r.rt.helper_json(&turn("system", 0, 1_000, 5_000));
    stop(&r, &wav2, &[], &[]).await;
    let (p2, segs2) = finalized(&r, "id2").await;
    assert!(
        p2.capture
            .as_ref()
            .is_none_or(|c| c.speakers_unavailable.is_empty()),
        "the first meeting's stand-down reached the second: {:?}",
        p2.capture
    );
    assert_eq!(
        speaker_of(&segs2, "system-u1").as_deref(),
        Some("system:0"),
        "a new recording's diarizer counts again"
    );
    let v = serde_json::to_value(&p2).unwrap();
    assert!(v
        .get("capture")
        .is_none_or(|c| c.get("speakersUnavailable").is_none()));
    // An older record (no field) still reads.
    let old: CaptureReport = serde_json::from_value(json!({"lostSpans": []})).unwrap();
    assert!(old.speakers_unavailable.is_empty());
}

/// Bogus times are bounded like turns (N122): a negative or > 48 h `atMs` is ignored, never overflows the offset.
#[tokio::test]
async fn bogus_times_are_ignored() {
    let r = rig();
    ready(&r).await;
    let wav = start(&r, "id1").await;
    r.rt.helper_json(
        &json!({"type": "diarization.unavailable", "source": "mic", "atMs": 1e300, "reason": "backlog"}),
    );
    r.rt.helper_json(&down("system", 49 * 3_600_000, "backlog"));
    stop(&r, &wav, &[], &[]).await;
    let (p, _) = finalized(&r, "id1").await;
    assert!(p.capture.is_none_or(|c| c.speakers_unavailable.is_empty()));
}
