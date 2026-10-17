//! Hardening cycle 28: stress scenarios over the real `NoteRuntime` (SQLite in a temp dir, real files), each a
//! failure Jared can hit on a Mac:
//!
//! - **N141 (ADR 0088)** the wall clock jumps (NTP after a sleep, a manual change, a VM resume) during a meeting
//!   whose helper then can't confirm the stop (crashed, or wedged and aborted). The stop's fallback duration was
//!   `ended_at − started_at` on the wall clock: an hour back gave a negative duration (and `endedAt` before
//!   `startedAt`), an hour forward an extra hour of meeting. Now the stop stamps `endedAt` in meeting time
//!   (`startedAt` + monotonic elapsed, like N45's reconnect offsets).
//! - a helper crash mid-meeting that never comes back, then the stop (reconnecting → processing, nothing lost);
//! - a quit right after the stop of a ~3 h meeting (6,000 lines), relaunch: recovery finishes it, search finds it;
//! - a recording whose WAV vanished before finalize (disk ejected, file removed): done from the transcript.
//!
//! Every test runs in its own `tempfile` dir, removed when the rig drops.
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use qrtz_lifecycle::runtime::*;
use qrtz_model::*;
use qrtz_storage::Db;
use serde_json::json;

struct Quiet;
impl EventSink for Quiet {
    fn emit(&self, _: CoreEvent) {}
}

/// A helper whose sends start failing once `dead` is set (the process is gone: the supervisor can't write to it).
#[derive(Default)]
struct Helper {
    dead: AtomicBool,
}
impl HelperPort for Helper {
    fn send(&self, _: &HelperCommand) -> bool {
        !self.dead.load(Ordering::SeqCst)
    }
}

/// Real time for both clocks, with an injectable wall-clock jump (monotonic time never jumps).
struct SkewClock {
    t0: Instant,
    epoch0: i64,
    jump: AtomicI64,
}
impl SkewClock {
    fn new() -> Self {
        Self {
            t0: Instant::now(),
            epoch0: 1_760_000_000_000,
            jump: AtomicI64::new(0),
        }
    }
    fn mono(&self) -> i64 {
        self.t0.elapsed().as_millis() as i64
    }
}
impl Clock for SkewClock {
    fn now_ms(&self) -> EpochMs {
        self.epoch0 + self.mono() + self.jump.load(Ordering::SeqCst)
    }
    fn mono_ms(&self) -> EpochMs {
        self.mono()
    }
}

struct Rig {
    rt: NoteRuntime,
    db: InlineDb,
    rec: std::path::PathBuf,
    helper: Arc<Helper>,
    clock: Arc<SkewClock>,
    tmp: tempfile::TempDir,
}

fn rig_in(tmp: tempfile::TempDir) -> Rig {
    let db = InlineDb::new(Db::open(tmp.path().join("qrtz.db")).unwrap());
    let rec = tmp.path().join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let n = Arc::new(AtomicU64::new(0));
    let helper = Arc::new(Helper::default());
    let clock = Arc::new(SkewClock::new());
    let rt = NoteRuntime::new(RuntimeDeps {
        store: Arc::new(Store(InlineDb(db.0.clone()))),
        steps: Arc::new(CoreSteps {
            db: InlineDb(db.0.clone()),
        }),
        helper: helper.clone(),
        events: Arc::new(Quiet),
        config: Arc::new(RuntimeConfig::default),
        clock: clock.clone(),
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
        helper,
        clock,
        tmp,
    }
}

fn rig() -> Rig {
    rig_in(tempfile::tempdir().unwrap())
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

fn line(n: usize, a: i64, b: i64) -> serde_json::Value {
    json!({"type": "asr", "event": {"kind": "final", "utteranceId": format!("u{n}"), "startMs": a, "endMs": b,
        "text": format!("line {n} of the meeting about budgets")}})
}

/// A 16 kHz mono 16-bit WAV of `ms` of silence (what the helper's WavWriter leaves after a crash: a header and PCM).
fn write_wav(path: &str, ms: u32) {
    let data = ms * 32;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&16_000u32.to_le_bytes());
    b.extend_from_slice(&32_000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    b.resize(44 + data as usize, 0);
    std::fs::write(path, b).unwrap();
}

async fn settled(r: &Rig, id: &str) -> Meeting {
    for _ in 0..1000 {
        if let Some(m) = r.db.0.lock().unwrap().notes().get_meeting(id).unwrap() {
            if m.status == MeetingStatus::Done {
                return m;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let m = r.db.0.lock().unwrap().notes().get_meeting(id).unwrap();
    panic!("{id} never done: {m:?}");
}

async fn stop(r: &Rig) {
    tokio::time::timeout(Duration::from_secs(10), r.rt.stop(StopReason::Manual))
        .await
        .expect("stop returns")
        .unwrap();
}

/// N141: an hour back on the wall clock mid-meeting, then the helper dies before the stop (no ack, no audio):
/// the duration is the meeting's real length, never negative, and `endedAt` is not before `startedAt`.
#[tokio::test]
async fn wall_clock_back_an_hour_then_a_dead_helper_at_the_stop() {
    let r = rig();
    ready(&r).await;
    start(&r, "id1").await;
    r.rt.helper_json(&line(1, 0, 400));
    tokio::time::sleep(Duration::from_millis(300)).await;
    r.clock.jump.store(-3_600_000, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    r.helper.dead.store(true, Ordering::SeqCst);
    stop(&r).await;
    let m = settled(&r, "id1").await;
    let (started, ended) = (m.started_at, m.ended_at.expect("endedAt"));
    assert!(ended >= started, "endedAt {ended} before startedAt {started}");
    let d = m.duration_ms.expect("duration");
    assert!(
        (400..60_000).contains(&d),
        "duration {d} ms for a ~0.5 s meeting (wall clock went back 1 h)"
    );
}

/// N141: an hour forward, and a helper that is alive but never acks the stop (wedged; the abort isn't acked
/// either): no extra hour of meeting.
#[tokio::test]
async fn wall_clock_forward_an_hour_then_an_unacked_stop() {
    let r = rig();
    ready(&r).await;
    start(&r, "id1").await;
    r.rt.helper_json(&line(1, 0, 400));
    tokio::time::sleep(Duration::from_millis(300)).await;
    r.clock.jump.store(3_600_000, Ordering::SeqCst);
    stop(&r).await;
    let m = settled(&r, "id1").await;
    let d = m.duration_ms.expect("duration");
    assert!(
        d < 60_000,
        "duration {d} ms for a meeting of a few seconds (wall clock went forward 1 h)"
    );
    assert!(m.ended_at.unwrap() - m.started_at < 60_000);
}

/// The helper crashes mid-meeting and never comes back; the user stops while it is reconnecting. The meeting is
/// finalized from what was captured (its WAV and the lines before the crash), with no wait on a helper that's gone.
#[tokio::test]
async fn helper_crash_mid_meeting_then_stop_while_reconnecting() {
    let r = rig();
    ready(&r).await;
    let wav = start(&r, "id1").await;
    for n in 0..20 {
        r.rt.helper_json(&line(n, n as i64 * 1000, n as i64 * 1000 + 900));
    }
    write_wav(&wav, 2_000);
    r.helper.dead.store(true, Ordering::SeqCst);
    r.rt.helper_down("crashed");
    for _ in 0..200 {
        if r.rt.session().phase == RecordingPhase::Reconnecting {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(r.rt.session().phase, RecordingPhase::Reconnecting);
    let t0 = Instant::now();
    stop(&r).await;
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "the stop waited on a dead helper: {:?}",
        t0.elapsed()
    );
    let m = settled(&r, "id1").await;
    let segs = r.db.0.lock().unwrap().notes().get_segments("id1").unwrap();
    assert_eq!(segs.len(), 20, "every line before the crash is kept");
    assert_eq!(
        m.duration_ms,
        Some(2_000),
        "the crash path takes the audio's length"
    );
    assert!(std::path::Path::new(&wav).exists());
}

/// A ~3 h meeting (6,000 lines) is stopped and the app quits at once, before the pipeline runs; on relaunch the
/// recovery finishes it (done, every line, the WAV kept) and full-text search finds a line from its last minute.
#[tokio::test]
async fn quit_right_after_stopping_a_three_hour_meeting_then_relaunch() {
    let r = rig();
    ready(&r).await;
    let wav = start(&r, "id1").await;
    const N: usize = 6_000;
    for n in 0..N {
        let a = n as i64 * 1_800;
        r.rt.helper_json(&line(n, a, a + 1_700));
    }
    r.rt.helper_json(
        &json!({"type": "asr", "event": {"kind": "final", "utteranceId": "last", "startMs": 10_799_000,
        "endMs": 10_799_900, "text": "zanzibar is the codeword"}}),
    );
    write_wav(&wav, 1_000);
    // Live appends are asynchronous: wait for them to land before the quit, like a real meeting's last minute.
    for _ in 0..1000 {
        if r.db.0.lock().unwrap().notes().get_segments("id1").unwrap().len() == N + 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let r2 = r.rt.clone();
    let stopping = tokio::spawn(async move { r2.stop(StopReason::Manual).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    r.rt.helper_json(&json!({"type": "recording.stopped", "audioPath": wav, "durationMs": 10_800_000}));
    let _ = tokio::time::timeout(Duration::from_secs(10), stopping).await;
    r.rt.kill(); // quit: nothing more runs in this process
    let Rig { tmp, db, .. } = r;
    drop(db);
    let r = rig_in(tmp);
    let t0 = Instant::now();
    r.rt.recover().await.unwrap();
    let m = settled(&r, "id1").await;
    assert!(
        t0.elapsed() < Duration::from_secs(10),
        "recovery took {:?}",
        t0.elapsed()
    );
    assert_eq!(m.status, MeetingStatus::Done);
    let segs = r.db.0.lock().unwrap().notes().get_segments("id1").unwrap();
    assert_eq!(segs.len(), N + 1, "no line lost across the quit");
    assert!(m
        .audio_path
        .as_deref()
        .is_some_and(|p| std::path::Path::new(p).exists()));
    let t1 = Instant::now();
    let hits = r.db.0.lock().unwrap().notes().search("zanzibar", 20).unwrap();
    assert!(
        t1.elapsed() < Duration::from_millis(500),
        "search took {:?}",
        t1.elapsed()
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
}

/// The recording's WAV is gone before finalize (an external disk ejected, the file removed): the meeting still
/// ends done from its transcript, with the transcript's length, and no audio path that points at nothing.
#[tokio::test]
async fn a_recording_whose_wav_vanished_finalizes_from_the_transcript() {
    let r = rig();
    ready(&r).await;
    let wav = start(&r, "id1").await;
    for n in 0..5 {
        r.rt.helper_json(&line(n, n as i64 * 1000, n as i64 * 1000 + 900));
    }
    let r2 = r.rt.clone();
    let stopping = tokio::spawn(async move { r2.stop(StopReason::Manual).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    // The helper acked with no length (it never wrote a byte) and the file isn't there.
    r.rt.helper_json(&json!({"type": "recording.stopped", "audioPath": wav, "durationMs": 0}));
    tokio::time::timeout(Duration::from_secs(10), stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let m = settled(&r, "id1").await;
    assert!(!std::path::Path::new(&wav).exists());
    let d = m.duration_ms.unwrap();
    assert!(d > 0, "duration {d}");
    let segs = r.db.0.lock().unwrap().notes().get_segments("id1").unwrap();
    assert_eq!(segs.len(), 5);
}
