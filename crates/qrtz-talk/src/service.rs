//! The talk service: push-to-talk dictation. idle → listening → finalizing → inserting → idle (or
//! error). On release the final ASR text gets the on-device cleanup only (no LLM: ADR 0071) and goes straight in.
//! The helper opens the mic; the on-device engine (`qrtz_speech::SpeechEngine`, the qrtz speech module) is the only
//! engine, so talk never touches the network (ADR 0076): a missing, revoked or stale module makes talk unavailable
//! (`speech-unavailable`), never a cloud fallback. Every wait has a timeout so a dictation always ends.
//!
//! Port of `apps/desktop/src/main/talk-service.ts` (busy ids 8eddeab, T1 2ea8a53, H5, T3/T4); ADR 0058.
use crate::cleanup::cleanup_dictation;
use crate::config::TalkConfig;
use crate::contract::*;
use crate::speech::{
    bound_final, drive, fail_error, max_rejects, start_error, HelperAsrSpeech, SessionCtl, TalkValidator,
};
use crate::store::{self, LoadOutcome, Store, StoreIo, MAX_HISTORY};
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_speech::engine::StatusPhase;
use qrtz_speech::{Channel, FailReason, SessionKind, SessionSpec, SpeakerMode, SpeechEngine, SpeechEvent};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;

// ------------------------------------------------------------------------------------------------- injected deps

/// Host → helper lines (qrtz-app: encode + `Supervisor::try_send`). `false` = the helper isn't running.
pub trait HelperSink: Send + Sync {
    fn send(&self, msg: &Value) -> bool;
}
/// `talk:state` / `talk:history-changed` onto the validated IPC bus.
pub trait TalkEvents: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}
pub trait Clipboard: Send + Sync {
    fn write(&self, text: &str);
}
pub trait Clock: Send + Sync {
    /// Wall clock, epoch ms (`at`, `startedAt`, file names).
    fn now_ms(&self) -> f64;
    /// Monotonic ms (latencies, durations and timeout checks; immune to clock jumps).
    fn mono_ms(&self) -> f64;
}
pub trait IdSource: Send + Sync {
    fn next_id(&self) -> String;
}
pub struct SystemClock {
    origin: std::time::Instant,
}
impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}
impl Clock for SystemClock {
    fn now_ms(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0)
    }
    fn mono_ms(&self) -> f64 {
        self.origin.elapsed().as_millis() as f64
    }
}

/// Random v4-style ids from the OS RNG via `std` hashing (no extra dependency; uniqueness, not secrecy).
pub struct RandomIds;
impl IdSource for RandomIds {
    fn next_id(&self) -> String {
        use std::hash::{BuildHasher, Hasher};
        let mut b = [0u8; 16];
        for chunk in b.chunks_mut(8) {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            );
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    }
}

/// Timeouts (ms), the TS defaults; tests shrink them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timeouts {
    /// `dictation.start` → `dictation.started`.
    pub started: u64,
    /// `dictation.stop` → `dictation.stopped`: the floor. The helper keeps the mic ~50 ms past the release (the
    /// tail, then up to one more tap block), then drains what is buffered and flushes the recogniser's last chunk (N124, ADR 0081), so this must
    /// cover tail + ≥ 1.5 s of flush. The real wait is [`Timeouts::stopped_for`]: longer for a longer dictation.
    pub stopped: u64,
    /// Extra `dictation.stop` → `dictation.stopped` time per second of dictated audio (a recogniser that fell behind
    /// real time has that much backlog to work through before its final).
    pub stopped_per_audio_s: u64,
    /// Cap of the scaled wait (a helper that never answers still ends the dictation). Never below the helper's own
    /// worst case when `helper_tail` is set (N130).
    pub stopped_max: u64,
    /// N130 (ADR 0084): the helper's release path (`DictationTail` in `QrtzHelperCore/Dictation.swift`). The helper
    /// owns the stop deadline (it knows its backlog) and always answers inside it, with its own last partial at
    /// worst; talk never gives up first. `None`: the scaled wait alone (test rigs with a mock helper).
    pub helper_tail: Option<HelperTail>,
    /// `insert.text` → `insert.result`.
    pub insert: u64,
    /// On-device flush → its last finals.
    pub final_ms: u64,
}
impl Default for Timeouts {
    fn default() -> Self {
        Self {
            started: 3000,
            stopped: 3000,
            stopped_per_audio_s: 100,
            stopped_max: 20_000,
            helper_tail: Some(HelperTail::default()),
            insert: 2500,
            final_ms: 1500,
        }
    }
}

impl Timeouts {
    /// N124: how long to wait for the helper's real final after the release of a dictation of `audio_ms`. Never less
    /// than `stopped` (tail + flush), plus `stopped_per_audio_s` per second of audio for the recogniser's backlog,
    /// capped at `stopped_max` (but never below `stopped`).
    pub fn stopped_for(&self, audio_ms: f64) -> u64 {
        let secs = if audio_ms.is_finite() {
            (audio_ms.max(0.0) / 1000.0).ceil() as u64
        } else {
            0
        };
        let scaled = self
            .stopped
            .saturating_add(secs.saturating_mul(self.stopped_per_audio_s));
        let scaled = scaled.min(self.stopped_max.max(self.stopped));
        // N130: never shorter than the helper's own worst case for this dictation.
        match &self.helper_tail {
            Some(t) => scaled.max(t.host_wait_ms(audio_ms)),
            None => scaled,
        }
    }
    /// ADR 0118: the stop wait of a dictation on a segmented (batch) recogniser, which decoded what it cut during
    /// the hold: the helper's segmented worst case, never below `stopped`. No per-second-of-dictation term past one
    /// segment. Without a helper tail (test rigs) it's [`Timeouts::stopped_for`].
    pub fn stopped_for_segmented(&self, audio_ms: f64) -> u64 {
        match &self.helper_tail {
            Some(t) => t.segmented_host_wait_ms(audio_ms).max(self.stopped),
            None => self.stopped_for(audio_ms),
        }
    }
}

/// N130 (ADR 0084): the helper's `DictationTail` defaults and formulas (`QrtzHelperCore/Dictation.swift`; held equal
/// by `tests/hardening27.rs`, which reads the Swift source). After `dictation.stop` the helper keeps capturing
/// `tail_ms`, drains the mic blocks still queued (`drain_timeout_ms` + 2 ms per ms queued), pads `pad_ms` of
/// silence, then waits for the recogniser's final (`finalize_timeout_ms` + 100 ms per second of audio + 1 ms per
/// ms queued), drain + finalize within `max_wait_ms` (finalize never below its floor). What is queued at the release
/// is at most the audio captured, so the host takes queued = audio: its bound is the helper's worst case for that
/// dictation, not a guess that a backlogged recogniser outlasts (which pasted the last partial: the cut-off tail).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HelperTail {
    pub tail_ms: u64,
    pub drain_timeout_ms: u64,
    pub pad_ms: u64,
    pub finalize_timeout_ms: u64,
    pub max_wait_ms: u64,
    /// N166 (ADR 0107): the helper's `HelperController.armMaxMs`. Since ADR 0104 the press opens the mic, and the
    /// start takes everything heard since the press as its pre-roll, up to this much. That head is audio the
    /// helper decodes and counts (`samples`), so the host's wait must count it too.
    pub arm_max_ms: u64,
    /// Host-side slack on top: the pad's own append on a slow recogniser, the line's trip and scheduling.
    pub slack_ms: u64,
    /// ADR 0118 (`SegmentedTail` in the helper): a segmented (batch) recogniser decodes finished segments during
    /// the hold and only the open last segment after the release, read `segment_overlap_ms` back before its cut,
    /// with `pad_ms` after.
    pub segment_overlap_ms: u64,
    /// The cap of one segment, its pad included (the batch model's 15 s window): neither the release's last segment nor a
    /// segment still decoding at the release is longer than `segment_cap_ms - pad_ms` of audio.
    pub segment_cap_ms: u64,
    /// The release decode's floor, and its allowance per second of audio still to decode.
    pub segment_decode_floor_ms: u64,
    pub segment_decode_ms_per_audio_s: u64,
}

/// ADR 0118: what `dictation.stopped` says about the recogniser that heard the dictation (all optional on the
/// wire): its id, and for a segmented recogniser the segments decoded and the release's tail decode (ms).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AsrInfo {
    /// The recogniser's id, as the helper names it; logged, never text. Capped at 48 chars.
    pub asr: String,
    pub segments: Option<u64>,
    pub tail_decode_ms: Option<f64>,
    /// ADR 0131 / 0132 (N169): the release tail decode's confidence (`tailConf`, 0..=1; absent when the recogniser
    /// reported none or no tail was decoded) and whether the helper's tail confidence floor dropped that tail
    /// (`tailDropped`). Logged on the `talk latency` line beside `raw_chars`, so a dictation that lost its tail is
    /// visible next to what was inserted. Additive: an older helper sends neither.
    pub tail_conf: Option<f64>,
    pub tail_dropped: bool,
}

impl AsrInfo {
    /// From a `dictation.stopped` line; `None` without an `asr` (an older helper, or no recogniser ran).
    pub fn from_stopped(m: &Value) -> Option<Self> {
        let asr: String = m["asr"]
            .as_str()
            .filter(|a| !a.is_empty())?
            .chars()
            .take(48)
            .collect();
        Some(AsrInfo {
            asr,
            segments: m["segments"].as_u64(),
            tail_decode_ms: m["tailDecodeMs"].as_f64().filter(|v| v.is_finite() && *v >= 0.0),
            tail_conf: m["tailConf"]
                .as_f64()
                .filter(|v| (0.0..=1.0).contains(v))
                .map(|v| (v * 1000.0).round() / 1000.0),
            tail_dropped: m["tailDropped"].as_bool() == Some(true),
        })
    }
    /// A segmented recogniser (it reported segments).
    pub fn segmented(&self) -> bool {
        self.segments.is_some()
    }
}

impl Default for HelperTail {
    fn default() -> Self {
        HelperTail {
            tail_ms: 50,
            drain_timeout_ms: 300,
            pad_ms: 640,
            finalize_timeout_ms: 1500,
            max_wait_ms: 20_000,
            arm_max_ms: 10_000,
            slack_ms: 1500,
            segment_overlap_ms: SEGMENT_OVERLAP_MS,
            segment_cap_ms: 15_000,
            segment_decode_floor_ms: 1500,
            segment_decode_ms_per_audio_s: 100,
        }
    }
}

impl HelperTail {
    /// `DictationTail.drainLimitMs(queuedMs:)`.
    pub fn drain_limit_ms(&self, queued_ms: u64) -> u64 {
        self.max_wait_ms
            .min(self.drain_timeout_ms.saturating_add(queued_ms.saturating_mul(2)))
    }
    /// `DictationTail.finalizeLimitMs(audioMs:queuedMs:)`.
    pub fn finalize_limit_ms(&self, audio_ms: u64, queued_ms: u64) -> u64 {
        let want = self
            .finalize_timeout_ms
            .saturating_add(audio_ms / 10)
            .saturating_add(queued_ms);
        let left = self.max_wait_ms.saturating_sub(self.drain_limit_ms(queued_ms));
        want.min(self.finalize_timeout_ms.max(left))
    }
    /// `DictationTail.budgetMs(audioMs:queuedMs:)`: release to `dictation.stopped` at most, as the helper counts it.
    pub fn budget_ms(&self, audio_ms: u64, queued_ms: u64) -> u64 {
        self.tail_ms
            .saturating_add(self.drain_limit_ms(queued_ms))
            .saturating_add(self.finalize_limit_ms(audio_ms, queued_ms))
    }
    /// The host's wait for a dictation of `audio_ms`: the helper's budget with everything still queued, plus the
    /// pad and `slack_ms`.
    pub fn host_wait_ms(&self, audio_ms: f64) -> u64 {
        let a = if audio_ms.is_finite() {
            audio_ms.max(0.0).ceil() as u64
        } else {
            0
        };
        self.budget_ms(a, a)
            .saturating_add(self.pad_ms)
            .saturating_add(self.slack_ms)
    }
    /// ADR 0118, `SegmentedTail.budgetMs(audioMs:)`: the segmented helper's release budget for a dictation of
    /// `audio_ms`. Tail, the mic drain (nothing queued: a batch recogniser takes no audio during the release), then
    /// the decode of the last segment (at most one segment: `segment_cap_ms - pad_ms`) with its overlap and the
    /// pad, plus at most one more segment still decoding in the background, at `segment_decode_ms_per_audio_s`,
    /// never below the floor and within `max_wait_ms` (the cap is kept). Unlike [`HelperTail::budget_ms`] it stops
    /// growing past two segments of audio.
    pub fn segmented_budget_ms(&self, audio_ms: u64) -> u64 {
        let span = self.segment_cap_ms.saturating_sub(self.pad_ms).max(1);
        let last = audio_ms.min(span);
        let queued = audio_ms.saturating_sub(last).min(span);
        let work = last
            .saturating_add(self.segment_overlap_ms)
            .saturating_add(self.pad_ms)
            .saturating_add(queued);
        let want = self
            .segment_decode_floor_ms
            .saturating_add(work.saturating_mul(self.segment_decode_ms_per_audio_s) / 1000);
        let drain = self.drain_limit_ms(0);
        let left = self.max_wait_ms.saturating_sub(drain);
        self.tail_ms
            .saturating_add(drain)
            .saturating_add(want.min(self.segment_decode_floor_ms.max(left)))
    }
    /// The host's wait for a segmented dictation of `audio_ms`: the helper's segmented budget, plus the pad and
    /// `slack_ms`.
    pub fn segmented_host_wait_ms(&self, audio_ms: f64) -> u64 {
        let a = if audio_ms.is_finite() {
            audio_ms.max(0.0).ceil() as u64
        } else {
            0
        };
        self.segmented_budget_ms(a)
            .saturating_add(self.pad_ms)
            .saturating_add(self.slack_ms)
    }
}

/// ADR 0118: segments don't overlap (docs/echo/dictation-tuning.md §11: every cut lands in silence, and a segment
/// that starts inside a word is often decoded blank).
pub const SEGMENT_OVERLAP_MS: u64 = 0;

/// N124: a dictation whose `dictation.stopped` didn't come within [`Timeouts::stopped_for`]. It went ahead with what
/// the recogniser had shown; the helper's real final may still come, and is then kept, never dropped (ADR 0081).
#[derive(Debug, Clone, PartialEq)]
enum Late {
    /// `finish` (cleanup, insert, history) is still running.
    Pending,
    /// The final arrived while `finish` was running: applied when it ends.
    Text(String),
    /// `finish` is done: a final now is applied at once.
    Done,
}

/// At most this many dictations wait for a late final (each is one release that timed out).
const MAX_LATE: usize = 8;
/// What a history entry says when its pasted text was cut short and the full text came later.
pub const MSG_LATE_FINAL: &str =
    "the pasted text was cut short: the full text came after the wait and is saved here";

pub struct TalkDeps {
    pub helper: Arc<dyn HelperSink>,
    pub events: Arc<dyn TalkEvents>,
    pub clipboard: Arc<dyn Clipboard>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
    /// The on-device engine (`qrtz_speech::SpeechEngine`: the qrtz speech module). Talk never knows the model.
    pub local: Arc<dyn SpeechEngine>,
    /// Today's helper's `dictation.asr` lines as a `SpeechEngine` (transitional; pass the same instance as
    /// `local` until the module ships, or any instance once `local` is the module).
    pub helper_asr: Arc<HelperAsrSpeech>,
    /// The full app config JSON (read on every use).
    pub config: Arc<dyn Fn() -> Value + Send + Sync>,
    /// `<data dir>/talk.json` (the same file Electron used).
    pub store_path: PathBuf,
    pub store_io: Arc<dyn StoreIo>,
    pub timeouts: Timeouts,
}

// ------------------------------------------------------------------------------------------------- state

struct Attempt {
    id: String,
    /// The on-device session (pinned by the engine to the module version it started on); `None` once it ended.
    local: Option<SessionCtl>,
    local_val: TalkValidator,
    local_done: Option<oneshot::Sender<()>>,
    finals: Vec<String>,
    partial: String,
}

impl Attempt {
    /// (committed, partial) as heard so far.
    fn text(&self) -> (String, String) {
        (self.finals.join(" ").trim().to_owned(), self.partial.clone())
    }
    fn live(&self) -> String {
        let (c, p) = self.text();
        join(&c, &p)
    }
}

fn join(a: &str, b: &str) -> String {
    format!("{a} {b}").trim().to_owned()
}

struct Inner {
    view: TalkView,
    store: Store,
    hellos: u64,
    dictation_helper: u64,
    via_push_key: bool,
    /// N111: the push key is physically down (last `hotkey` was `down`).
    key_held: bool,
    key_down: f64,
    key_up: f64,
    /// N166: when the push key last went down (a press queued behind a finishing dictation, N110, starts later).
    press_at: f64,
    /// N166 (ADR 0107): audio the helper heard before this dictation's start (its armed or warm pre-roll): the
    /// larger of `dictation.started`'s `preRollMs` and, for a press, the time from the press to the start, both
    /// bounded by [`HelperTail::arm_max_ms`]. Counted in the stop wait, as the helper counts it.
    head_ms: f64,
    /// ADR 0112: how loud the mic was while this dictation listened (`dictation.level`), for the latency line.
    level: crate::level::LevelMeter,
    /// ADR 0118: this helper process's last answered dictation ran on a segmented recogniser (its stop wait then
    /// follows the last segment, not the whole dictation); cleared by a new helper process. And this dictation's
    /// recogniser.
    segmented_helper: bool,
    asr: Option<AsrInfo>,
    first_partial: Option<f64>,
    waiters: HashMap<String, oneshot::Sender<Option<Value>>>,
    attempt: Option<Attempt>,
    /// N124: dictations whose real final may still arrive after the stop wait ran out.
    late: HashMap<String, Late>,
}

struct Shared {
    deps: TalkDeps,
    inner: Mutex<Inner>,
    /// N110: the push key went down while the last dictation was still finalizing. The next dictation starts the
    /// moment the view takes a start (checked in `emit_view`, under the state lock), unless the key goes up first.
    pending_press: AtomicBool,
}

/// The service. Cheap to clone (one shared state); every method may be called from any tokio task.
#[derive(Clone)]
pub struct TalkService(Arc<Shared>);

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}

const MSG_BUSY: &str = "another dictation is still running";
/// N142: the insert could not be sent (the capture helper is gone, restarting, or refused the line).
const MSG_NO_HELPER_INSERT: &str = "the capture helper could not take the text, so it was not pasted";
/// N134: the helper refused the start because no recogniser could take it (all busy finishing, or none loads).
const MSG_ASR: &str = "speech recognition isn't ready yet; try again in a moment";
const MSG_HOTKEY: &str = "the push-to-talk key isn't available; allow qrtz in system settings → privacy & \
    security → input monitoring and accessibility";

impl TalkService {
    pub fn new(deps: TalkDeps) -> Self {
        let (st, outcome) = store::load(deps.store_io.as_ref(), &deps.store_path, deps.clock.now_ms());
        match &outcome {
            LoadOutcome::Corrupt { moved_to } => {
                tracing::warn!(
                    moved = moved_to.is_some(),
                    "talk store was not valid JSON; moved aside, starting empty"
                )
            }
            LoadOutcome::Unreadable(kind) => tracing::warn!(%kind, "talk store unreadable; starting empty"),
            LoadOutcome::Loaded { dropped } if *dropped > 0 => {
                tracing::warn!(dropped, "talk store: invalid entries dropped")
            }
            _ => {}
        }
        Self(Arc::new(Shared {
            deps,
            inner: Mutex::new(Inner {
                view: TalkView::idle(),
                store: st,
                hellos: 0,
                dictation_helper: 0,
                via_push_key: false,
                key_held: false,
                key_down: 0.0,
                key_up: 0.0,
                press_at: 0.0,
                head_ms: 0.0,
                level: Default::default(),
                segmented_helper: false,
                asr: None,
                first_partial: None,
                waiters: HashMap::new(),
                attempt: None,
                late: HashMap::new(),
            }),
            pending_press: AtomicBool::new(false),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn deps(&self) -> &TalkDeps {
        &self.0.deps
    }
    fn cfg(&self) -> TalkConfig {
        TalkConfig::from_app_config(&(self.deps().config)())
    }
    fn mono(&self) -> f64 {
        self.deps().clock.mono_ms()
    }
    /// N166: a head the helper may have heard before a start, in `[0, arm_max_ms]` (0 when not finite).
    fn head_bound(&self, ms: f64) -> f64 {
        let max = self
            .deps()
            .timeouts
            .helper_tail
            .map(|t| t.arm_max_ms)
            .unwrap_or(HelperTail::default().arm_max_ms) as f64;
        if ms.is_finite() {
            ms.clamp(0.0, max)
        } else {
            0.0
        }
    }
    fn send(&self, msg: Value) -> bool {
        self.deps().helper.send(&msg)
    }
    fn emit_view(&self, g: &Inner) {
        self.deps()
            .events
            .emit("talk:state", serde_json::to_value(&g.view).unwrap_or(Value::Null));
        // N110: a press held through the last dictation's finalizing starts the next one now.
        if g.view.takes_a_start() && self.0.pending_press.swap(false, Ordering::SeqCst) {
            let me = self.clone();
            tokio::spawn(async move { me.start_inner(true).await });
        }
    }
    fn save(&self, g: &Inner) {
        if let Err(e) = self
            .deps()
            .store_io
            .write_private_atomic(&self.deps().store_path, &store::serialize(&g.store))
        {
            // Never the content: only the error kind. The in-memory state is kept and the next write carries it.
            tracing::warn!(kind = %e.kind(), "talk store write failed");
        }
    }

    // --------------------------------------------------------------------------------------------- reads

    pub fn state(&self) -> TalkView {
        self.lock().view.clone()
    }
    pub fn history(&self) -> Vec<TalkEntry> {
        self.lock().store.history.clone()
    }
    pub fn dictionary(&self) -> Vec<DictionaryEntry> {
        self.lock().store.dictionary.clone()
    }
    pub fn set_dictionary(&self, entries: Vec<DictionaryEntry>) -> Vec<DictionaryEntry> {
        let mut g = self.lock();
        let mut seen = std::collections::HashSet::new();
        g.store.dictionary = entries
            .into_iter()
            .filter_map(|e| e.normalized())
            .filter(|e| seen.insert(e.term.to_lowercase()))
            .collect();
        self.save(&g);
        g.store.dictionary.clone()
    }
    pub fn delete_entry(&self, id: &str) -> bool {
        let mut g = self.lock();
        let n = g.store.history.len();
        g.store.history.retain(|h| h.id != id);
        self.save(&g);
        self.deps().events.emit("talk:history-changed", json!({"id": id}));
        g.store.history.len() != n
    }
    pub fn clear_history(&self) {
        let mut g = self.lock();
        g.store.history.clear();
        self.save(&g);
        self.deps()
            .events
            .emit("talk:history-changed", json!({"id": "*"}));
    }
    pub fn copy(&self, id: &str) -> bool {
        let g = self.lock();
        match g.store.history.iter().find(|h| h.id == id) {
            Some(e) => {
                self.deps().clipboard.write(&e.text);
                true
            }
            None => false,
        }
    }

    /// The `talk:*` channels (payloads already validated by qrtz-ipc against the contract).
    pub async fn call(&self, channel: &str, p: Value) -> Result<Value, AppError> {
        let view = |v: TalkView| Ok(serde_json::to_value(v).unwrap_or(Value::Null));
        match channel {
            "talk:state" => view(self.state()),
            "talk:start" => view(self.start().await),
            "talk:stop" => view(self.stop().await),
            "talk:cancel" => view(self.cancel()),
            "talk:history" => Ok(json!(self.history())),
            "talk:delete-entry" => Ok(json!({"ok": self.delete_entry(&s(&p, "id"))})),
            "talk:clear-history" => {
                self.clear_history();
                Ok(json!({"ok": true}))
            }
            "talk:copy" => Ok(json!({"ok": self.copy(&s(&p, "id"))})),
            "talk:dictionary" => Ok(json!(self.dictionary())),
            "talk:set-dictionary" => {
                let entries: Vec<DictionaryEntry> = serde_json::from_value(p["entries"].clone())
                    .map_err(|e| AppError::new(ErrorCode::Validation, format!("talk:set-dictionary: {e}")))?;
                Ok(json!(self.set_dictionary(entries)))
            }
            _ => Err(AppError::new(
                ErrorCode::UnknownChannel,
                format!("no talk handler for {channel}"),
            )),
        }
    }

    /// Toggle hotkey: listening → stop, otherwise start.
    pub async fn toggle(&self) -> TalkView {
        if self.state().phase == TalkPhase::Listening {
            self.stop().await
        } else {
            self.start().await
        }
    }

    // --------------------------------------------------------------------------------------------- engine

    /// Which engine a new dictation uses: always the on-device qrtz speech module (there is no cloud, ADR 0076).
    pub fn resolve_engine(&self) -> Engine {
        Engine::OnDevice
    }

    /// Push settings to the helper. Call on start, every helper hello and every config change.
    pub fn configure(&self) {
        let cfg = self.cfg();
        if !cfg.enabled {
            self.switched_off();
        }
        self.send(json!({
            "type": "dictation.configure",
            "warmMic": cfg.enabled && cfg.warm_mic,
            "preRollMs": cfg.pre_roll_ms,
            "streamAudio": false, // the mic audio is only ever for a host-side engine; there is none
            "pushKey": if cfg.enabled { cfg.push_key.clone() } else { String::new() },
            "consumeFn": cfg.block_globe_key,
            "pill": cfg.show_pill,
            "autoInsert": cfg.auto_insert,
            "echoGate": cfg.echo_gate,
        }));
    }

    /// N172 (ADR 0147): echo was switched off (`talk.enabled`, or ADR 0146's `modules.echo.enabled` /
    /// `modules.enabled`). The push key is about to be unset on the helper, so no release will ever arrive: a
    /// dictation still listening is cancelled now (mic closed, nothing inserted) instead of recording on until a
    /// timeout, and a press held for the next dictation is forgotten. One already finalizing or inserting was
    /// released by the user and finishes as usual.
    fn switched_off(&self) {
        self.0.pending_press.store(false, Ordering::SeqCst);
        let listening = {
            let mut g = self.lock();
            g.key_held = false;
            g.via_push_key = false;
            g.view.phase == TalkPhase::Listening
        };
        if listening {
            tracing::info!("echo switched off while a dictation was listening; cancelled, nothing inserted");
            self.cancel();
        }
    }

    /// ADR 0092: show a failure or notice only the host knows on the helper's native pill (`kind`: error|info).
    /// The text is sent lowercase and capped at the contract's 200 characters.
    pub fn pill_flash(&self, kind: &str, message: &str) {
        let mut m: String = message.trim().to_lowercase();
        if m.is_empty() {
            return;
        }
        if m.chars().count() > 200 {
            m = m.chars().take(199).collect::<String>() + "…";
        }
        self.send(json!({"type": "pill.flash", "kind": kind, "message": m}));
    }

    // --------------------------------------------------------------------------------------------- helper in

    /// A validated helper → host line: `hello`, `hotkey`, `dictation.*`, `insert.*` and `error{dictation.*}`.
    pub fn helper_line(&self, m: &Value) {
        // The speech module's lines (and today's helper's `dictation.asr`) are validated by the engine first.
        if self.deps().helper_asr.on_helper_line(m) {
            return;
        }
        let ty = m["type"].as_str().unwrap_or_default();
        match ty {
            "hello" => {
                // A fresh helper process: a dictation started under the previous one is gone (T1). Lines written
                // before a process's first hello reach that process, so a dictation before any hello is helper 1's.
                let lost = {
                    let mut g = self.lock();
                    g.hellos += 1;
                    // N124: a new helper process sends no final for the old one's dictations.
                    g.late.clear();
                    // ADR 0118: nor has it said what its recogniser is.
                    g.segmented_helper = false;
                    g.dictation_helper < g.hellos
                };
                if lost {
                    self.helper_lost("helper restarted");
                }
                self.configure();
            }
            "hotkey" => {
                let action = s(m, "action");
                // N172: a release always counts (it only ends what a press started), so a dictation started
                // before echo was switched off still stops on release; only a press needs echo on.
                if action != "up" && !self.cfg().enabled {
                    return;
                }
                tracing::info!(%action, "push key");
                let mut g = self.lock();
                if action == "down" {
                    g.key_held = true;
                    g.press_at = self.mono();
                    if g.view.takes_a_start() {
                        g.via_push_key = true;
                        drop(g);
                        let me = self.clone();
                        tokio::spawn(async move { me.start_inner(true).await });
                    } else if g.view.phase == TalkPhase::Finalizing {
                        // N110: the pill is already gone, so this press must not be dropped: start as soon as
                        // the last dictation is past finalizing (`emit_view`), if the key is still down then.
                        g.via_push_key = true;
                        self.0.pending_press.store(true, Ordering::SeqCst);
                    }
                } else if action == "up" {
                    g.key_held = false;
                    if self.0.pending_press.swap(false, Ordering::SeqCst) {
                        g.via_push_key = false;
                        tracing::info!(
                            "push key released before the last dictation finished; nothing started"
                        );
                    } else if g.via_push_key {
                        g.via_push_key = false;
                        drop(g);
                        let me = self.clone();
                        tokio::spawn(async move { me.stop().await });
                    }
                }
            }
            "dictation.started" => {
                let id = s(m, "id");
                {
                    // N166: the helper's own count of the head it fed (armed or warm pre-roll).
                    let mut g = self.lock();
                    if g.view.id.as_deref() == Some(id.as_str()) {
                        let pre = self.head_bound(m["preRollMs"].as_f64().unwrap_or(0.0));
                        g.head_ms = g.head_ms.max(pre);
                    }
                }
                self.resolve(&format!("started:{id}"), m.clone())
            }
            "dictation.level" => {
                let mut g = self.lock();
                if g.view.phase == TalkPhase::Listening {
                    g.view.level = m["rms"].as_f64().unwrap_or(0.0);
                    if let Some(r) = m["rms"].as_f64() {
                        g.level.add(r);
                    }
                    self.emit_view(&g);
                }
            }
            "dictation.stopped" => {
                let id = s(m, "id");
                let key = format!("stopped:{id}");
                let mut g = self.lock();
                if let Some(tx) = g.waiters.remove(&key) {
                    if let Err(Some(m)) = tx.send(Some(m.clone())) {
                        // N124: the stop wait ran out a moment ago and hasn't taken its lock yet: this is a late
                        // final, kept for when it goes ahead (it registers the dictation as late, keeping this).
                        g.late.insert(id.clone(), Late::Text(bound_final(&s(&m, "text"))));
                    }
                    return;
                }
                // N124: the real final of a dictation that already went ahead on what was shown. Kept, never dropped.
                if g.late.contains_key(&id) {
                    drop(g);
                    return self.late_final(&id, bound_final(&s(m, "text")));
                }
                // H5: the helper ended the dictation itself. Keep and insert what was heard, then say why.
                let reason = s(m, "reason");
                if !reason.is_empty()
                    && g.view.id.as_deref() == Some(id.as_str())
                    && g.view.phase == TalkPhase::Listening
                {
                    drop(g);
                    let (kind, msg) = if reason == "permission-revoked" {
                        (
                            TalkErrorKind::PermissionRevoked,
                            "microphone access was turned off, so dictation stopped",
                        )
                    } else {
                        (
                            TalkErrorKind::MicLost,
                            "the microphone went away, so dictation stopped",
                        )
                    };
                    let text = bound_final(&s(m, "text"));
                    let dur = m["durationMs"].as_f64().unwrap_or(0.0);
                    let me = self.clone();
                    tokio::spawn(async move { me.ended_by_helper(id, text, dur, kind, msg).await });
                }
            }
            "permission.revoked" => {
                if m["permission"] == "microphone" && self.lock().view.active() {
                    tracing::warn!("microphone revoked mid-dictation");
                }
            }
            "insert.result" => self.resolve(&format!("insert:{}", s(m, "id")), m.clone()),
            "error" => {
                let code = s(m, "code");
                if !code.starts_with("dictation.") {
                    return;
                }
                tracing::warn!(%code, "dictation error");
                if code == "dictation.mic" {
                    let mut g = self.lock();
                    if g.view.active() {
                        let msg = m["message"]
                            .as_str()
                            .unwrap_or("microphone access denied")
                            .to_owned();
                        self.fail(&mut g, TalkErrorKind::MicDenied, &msg, true);
                    }
                } else if code == "dictation.busy" {
                    self.on_busy(m);
                } else if code == "dictation.asr" {
                    // N134: the helper refused our start (it never ran): say so now instead of waiting it out.
                    let mut g = self.lock();
                    let ours = m["id"].as_str().is_none_or(|id| g.view.id.as_deref() == Some(id));
                    if g.view.active() && ours {
                        self.fail(&mut g, TalkErrorKind::SpeechUnavailable, MSG_ASR, false);
                    }
                } else if code == "dictation.hotkey" {
                    // The helper couldn't watch the push key: pressing it would do nothing, so say so (never
                    // silent). An active dictation keeps going; the next press is what's broken.
                    if !self.lock().view.active() {
                        self.refuse(TalkErrorKind::PermissionRevoked, MSG_HOTKEY, None);
                    }
                }
            }
            _ => {}
        }
    }

    /// Supervisor status: not running and not starting while a dictation is active → T1 helper-lost.
    pub fn helper_status(&self, running_or_starting: bool, why: &str) {
        if !running_or_starting {
            self.helper_lost(why);
        }
    }

    /// error{dictation.busy, id, activeId}: act only when `id` is our start in flight. `activeId == id` is a
    /// duplicate start of our running dictation (keep it); any other `activeId` is an orphan: cancel it by its
    /// own id, then fail this attempt without cancelling it (the helper never ran it). Never cancels by view id.
    fn on_busy(&self, m: &Value) {
        let (bid, active) = (s(m, "id"), s(m, "activeId"));
        let mut g = self.lock();
        let id = g.view.id.clone().unwrap_or_default();
        if bid.is_empty() || bid != id || g.view.phase != TalkPhase::Listening {
            tracing::warn!(id = %bid, current = %id, "dictation.busy for another start; ignored");
            return;
        }
        if active == id {
            if let Some(tx) = g.waiters.remove(&format!("started:{id}")) {
                let _ = tx.send(Some(
                    json!({"type": "dictation.started", "id": id, "preRollMs": 0}),
                ));
            }
            return;
        }
        if !active.is_empty() {
            tracing::warn!(active_id = %active, "cancelling orphaned dictation");
            self.send(json!({"type": "dictation.cancel", "id": active}));
        }
        self.fail(&mut g, TalkErrorKind::Busy, MSG_BUSY, false);
    }

    /// T1: the helper that served the dictation is gone (supervisor down, or a new hello). End it gracefully with
    /// what was heard, release every waiter for its id. Idempotent; a no-op when idle.
    pub fn helper_lost(&self, why: &str) {
        let mut g = self.lock();
        // ADR 0118: the next process says again what its recogniser is.
        g.segmented_helper = false;
        // No insert result will come from a helper that is gone: release every outstanding insert, including an
        // older dictation's still inserting behind a newer one (N110); its words go to the clipboard.
        let inserts: Vec<String> = g
            .waiters
            .keys()
            .filter(|k| k.starts_with("insert:"))
            .cloned()
            .collect();
        for k in inserts {
            if let Some(tx) = g.waiters.remove(&k) {
                let _ = tx.send(None);
            }
        }
        self.0.pending_press.store(false, Ordering::SeqCst);
        // No late final will come from a helper that is gone (N124). A `finish` still running ends as it is.
        g.late.clear();
        let Some(id) = g.view.id.clone().filter(|_| g.view.active()) else {
            return;
        };
        tracing::warn!(why, phase = ?g.view.phase, "helper lost mid-dictation");
        g.via_push_key = false;
        for k in ["started", "stopped"] {
            if let Some(tx) = g.waiters.remove(&format!("{k}:{id}")) {
                let _ = tx.send(None);
            }
        }
        if g.view.phase == TalkPhase::Listening {
            let dur = self.mono() - g.key_down;
            drop(g);
            let me = self.clone();
            tokio::spawn(async move {
                me.ended_by_helper(
                    id,
                    String::new(),
                    dur,
                    TalkErrorKind::HelperLost,
                    "the capture helper restarted, so dictation stopped",
                )
                .await
            });
        }
    }

    fn resolve(&self, key: &str, m: Value) {
        if let Some(tx) = self.lock().waiters.remove(key) {
            let _ = tx.send(Some(m));
        }
    }

    fn waiter(g: &mut Inner, key: String) -> oneshot::Receiver<Option<Value>> {
        let (tx, rx) = oneshot::channel();
        g.waiters.insert(key, tx);
        rx
    }

    async fn wait(&self, key: String, rx: oneshot::Receiver<Option<Value>>, ms: u64) -> Option<Value> {
        let r = tokio::time::timeout(Duration::from_millis(ms), rx).await;
        self.lock().waiters.remove(&key);
        r.ok().and_then(|r| r.ok()).flatten()
    }

    fn refresh(&self, g: &mut Inner) {
        let now = self.mono();
        let Some(a) = g.attempt.as_ref() else { return };
        let (c, p) = a.text();
        if (!c.is_empty() || !p.is_empty()) && g.first_partial.is_none() {
            g.first_partial = Some(now);
        }
        g.view.text = c;
        g.view.partial = p;
        g.view.engine = Some(Engine::OnDevice);
        g.view.failover = Some(false);
        self.emit_view(g);
    }

    // --------------------------------------------------------------------------------------------- speech

    fn on_event(&self, id: &str) -> Arc<dyn Fn(SpeechEvent) + Send + Sync> {
        let me = Arc::downgrade(&self.0);
        let id = id.to_owned();
        Arc::new(move |ev| {
            if let Some(sh) = me.upgrade() {
                TalkService(sh).on_local(&id, ev)
            }
        })
    }

    fn spec(id: &str) -> SessionSpec {
        SessionSpec {
            session_id: id.to_owned(),
            kind: SessionKind::Dictation,
            channels: vec![Channel::Mic],
            language_hint: None,
            speaker_mode: SpeakerMode::Fast,
        }
    }

    /// Validated on-device words, or the engine going away.
    fn on_local(&self, id: &str, ev: SpeechEvent) {
        let mut g = self.lock();
        let Some(a) = g.attempt.as_mut().filter(|a| a.id == id) else {
            return;
        };
        let failed = match &ev {
            SpeechEvent::Words { .. } => match a.local_val.words(&ev) {
                Ok(h) => {
                    if h.is_final {
                        a.finals.push(h.text);
                        a.partial.clear();
                    } else {
                        a.partial = h.text;
                    }
                    self.refresh(&mut g);
                    None
                }
                Err(r) => {
                    tracing::warn!(why = r.0, "rejected on-device speech output");
                    (a.local_val.rejected >= max_rejects()).then_some(FailReason::ProtocolViolations)
                }
            },
            SpeechEvent::Finished { .. } => {
                a.local = None;
                if let Some(tx) = a.local_done.take() {
                    let _ = tx.send(());
                }
                None
            }
            SpeechEvent::Status {
                phase: StatusPhase::Failed,
                error,
            } => {
                tracing::warn!(?error, "on-device speech status failed");
                Some(FailReason::StartFailed)
            }
            SpeechEvent::Failed { reason } => Some(*reason),
            // Speaker turns are diarization (note's); talk never names a speaker.
            SpeechEvent::Status { .. } | SpeechEvent::Speaker { .. } => None,
        };
        if let Some(reason) = failed {
            self.local_failed(g, id, reason);
        }
    }

    /// The on-device session is gone mid-dictation: it ends cleanly with what was heard (no cloud fallback): `speech-unavailable` when the module was revoked, went stale or isn't
    /// installed (the engine ends a revoked session at its next safe point, after the utterance in flight), else
    /// `speech-lost` (crash, protocol violations).
    fn local_failed(&self, mut g: MutexGuard<'_, Inner>, id: &str, reason: FailReason) {
        let now = self.mono();
        let Some(a) = g.attempt.as_mut().filter(|a| a.id == id) else {
            return;
        };
        tracing::warn!(?reason, "on-device speech ended");
        a.local = None;
        if let Some(tx) = a.local_done.take() {
            let _ = tx.send(());
        }
        let heard = a.live();
        if g.view.phase != TalkPhase::Listening {
            return;
        }
        let (id, dur) = (id.to_owned(), now - g.key_down);
        self.send(json!({"type": "dictation.cancel", "id": id}));
        drop(g);
        let me = self.clone();
        tokio::spawn(async move {
            let (kind, msg) = fail_error(reason);
            me.ended_by_helper(id, heard, dur, kind, msg).await
        });
    }

    // --------------------------------------------------------------------------------------------- lifecycle

    fn release_waiters(g: &mut Inner, id: &str) {
        for k in ["started", "stopped"] {
            if let Some(tx) = g.waiters.remove(&format!("{k}:{id}")) {
                let _ = tx.send(None);
            }
        }
    }

    /// A press refused before `dictation.start`. `attempt`: the id that press drew (N150, ADR 0096): each refused
    /// press is its own attempt, so two in a row are two failures (the shell's `PillGate` keys a flash by the view's
    /// id), while re-emitting the same view keeps its id and still flashes once.
    fn refuse(&self, kind: TalkErrorKind, error: &str, attempt: Option<&str>) -> TalkView {
        let mut g = self.lock();
        g.view = TalkView {
            phase: TalkPhase::Error,
            id: attempt.map(str::to_owned),
            error: Some(error.into()),
            error_kind: Some(kind),
            ..g.view.idle_keeping()
        };
        self.emit_view(&g);
        g.view.clone()
    }

    fn fail(&self, g: &mut Inner, kind: TalkErrorKind, error: &str, cancel: bool) {
        if let Some(id) = g.view.id.clone() {
            if cancel {
                self.send(json!({"type": "dictation.cancel", "id": id}));
            }
            Self::release_waiters(g, &id);
        }
        g.attempt = None;
        g.view.phase = TalkPhase::Error;
        g.view.error = Some(error.to_owned());
        g.view.error_kind = Some(kind);
        g.view.level = 0.0;
        self.emit_view(g);
    }

    pub async fn start(&self) -> TalkView {
        self.start_inner(false).await
    }

    /// `from_key`: started by the push key, so the key state decides when it stops (N111).
    async fn start_inner(&self, from_key: bool) -> TalkView {
        if !self.lock().view.takes_a_start() {
            return self.state();
        }
        let id = self.deps().ids.next_id();
        // The on-device session starts before the mic opens: `start()` is the engine's version handshake, so an
        // incompatible, missing, revoked or stale module is a typed refusal with the mic never touched (warm-mic
        // pre-roll covers the spawn). There is no cloud to fall back to (ADR 0076).
        let local = match self.deps().local.start(Self::spec(&id)).await {
            Ok(sess) => sess,
            Err(e) => {
                let (kind, msg) = start_error(&e);
                return self.refuse(kind, &msg, Some(&id));
            }
        };
        let now = self.deps().clock.now_ms();
        // N172: echo was switched off during the engine handshake: a push-key start never opens the mic.
        if from_key && !self.cfg().enabled {
            tracing::info!("echo switched off while a push-key dictation was starting; not started");
            return self.state();
        }
        let started_rx = {
            let mut g = self.lock();
            if !g.view.takes_a_start() {
                return g.view.clone();
            }
            g.dictation_helper = g.hellos.max(1);
            g.key_down = self.mono();
            // N166: a press held through the last dictation's finalizing (N110) was heard by the helper from the
            // press on (ADR 0104), not from this start.
            g.head_ms = if from_key {
                self.head_bound(g.key_down - g.press_at)
            } else {
                0.0
            };
            g.first_partial = None;
            g.level = Default::default();
            g.asr = None;
            g.view = TalkView {
                phase: TalkPhase::Listening,
                id: Some(id.clone()),
                started_at: Some(now),
                engine: Some(Engine::OnDevice),
                ..TalkView::idle()
            };
            if from_key {
                g.via_push_key = true;
            }
            g.attempt = Some(Attempt {
                id: id.clone(),
                local: Some(drive(local, Channel::Mic, self.on_event(&id))),
                local_val: TalkValidator::new(&[Channel::Mic]),
                local_done: None,
                finals: Vec::new(),
                partial: String::new(),
            });
            Self::waiter(&mut g, format!("started:{id}"))
        };
        if !self.send(json!({"type": "dictation.start", "id": id})) {
            let mut g = self.lock();
            self.fail(
                &mut g,
                TalkErrorKind::HelperUnavailable,
                "the capture helper isn't running, so the mic can't open",
                false,
            );
            return g.view.clone();
        }
        {
            let mut g = self.lock();
            // The helper can answer at once (busy / mic denied): this attempt is already over.
            if g.view.id.as_deref() == Some(id.as_str()) && g.view.phase == TalkPhase::Listening {
                self.emit_view(&g);
                // N111: the key went up while the engine was starting (a tap shorter than the module spawn and
                // handshake): its release found nothing listening yet, so stop now. After `dictation.start` was
                // sent, so the helper sees start then stop.
                let released = from_key && !g.key_held;
                if released {
                    g.via_push_key = false;
                }
                drop(g);
                if released {
                    tracing::info!("push key released while the dictation was starting; stopping it");
                    let me = self.clone();
                    tokio::spawn(async move { me.stop().await });
                }
            }
        }
        let started = self
            .wait(format!("started:{id}"), started_rx, self.deps().timeouts.started)
            .await;
        let mut g = self.lock();
        if started.is_none()
            && g.view.id.as_deref() == Some(id.as_str())
            && g.view.phase == TalkPhase::Listening
        {
            self.fail(
                &mut g,
                TalkErrorKind::MicDidNotStart,
                "Microphone did not start",
                true,
            );
        }
        g.view.clone()
    }

    pub async fn stop(&self) -> TalkView {
        let (id, rx) = {
            let mut g = self.lock();
            let Some(id) = g.view.id.clone().filter(|_| g.view.phase == TalkPhase::Listening) else {
                return g.view.clone();
            };
            g.key_up = self.mono();
            g.view.phase = TalkPhase::Finalizing;
            g.view.level = 0.0;
            self.emit_view(&g);
            let rx = Self::waiter(&mut g, format!("stopped:{id}"));
            (id, rx)
        };
        self.send(json!({"type": "dictation.stop", "id": id}));
        // N124: wait for the helper's real final (its tail, drain and recogniser flush), scaled to the dictation's
        // length; only past that bound does the dictation go ahead on what was shown so far.
        let (helper, wait_ms) = {
            let g = self.lock();
            // N166: the helper's audio is the head it heard before the start plus the held time.
            let audio = g.key_up - g.key_down + g.head_ms;
            let t = self.deps().timeouts;
            // ADR 0118: this helper's recogniser is segmented: what it cut was decoded during the hold, so wait
            // for its last segment only. Until a dictation says so (first one after a helper start), the
            // streaming bound, which is never shorter.
            let wait = if g.segmented_helper {
                t.stopped_for_segmented(audio)
            } else {
                t.stopped_for(audio)
            };
            (g.dictation_helper, wait)
        };
        let r = tokio::time::timeout(Duration::from_millis(wait_ms), rx).await;
        let (text, dur) = {
            let mut g = self.lock();
            // Under the same lock as the waiter's removal, so a final landing right at the bound is either the
            // waiter's or a late one, never neither.
            g.waiters.remove(&format!("stopped:{id}"));
            let timed_out = r.is_err();
            let stopped = r.ok().and_then(|r| r.ok()).flatten();
            if let Some(a) = stopped.as_ref().and_then(AsrInfo::from_stopped) {
                // ADR 0118: remembered for this helper's next stop wait (a fallback recogniser clears it).
                if g.dictation_helper == helper && g.hellos <= helper {
                    g.segmented_helper = a.segmented();
                }
                g.asr = Some(a);
            }
            if timed_out && g.dictation_helper == helper && g.hellos <= helper && g.late.len() < MAX_LATE {
                // Timed out (a lost helper releases the waiter with nothing instead, and sends no final) and the
                // helper is still the one that ran it, so its final may still come: keep it when it does.
                tracing::warn!(
                    wait_ms,
                    "no final from the helper in time; going ahead with what was shown"
                );
                // A final that raced the bound may already be here (`helper_line`): keep it.
                g.late.entry(id.clone()).or_insert(Late::Pending);
            }
            (
                stopped
                    .as_ref()
                    .map(|m| bound_final(&s(m, "text")))
                    .unwrap_or_default(),
                stopped
                    .as_ref()
                    .and_then(|m| m["durationMs"].as_f64())
                    .unwrap_or(g.key_up - g.key_down),
            )
        };
        let me = self.clone();
        tokio::spawn(async move { me.finish(id, text, dur).await });
        self.state()
    }

    pub fn cancel(&self) -> TalkView {
        let mut g = self.lock();
        let Some(id) = g.view.id.clone().filter(|_| g.view.active()) else {
            return g.view.clone();
        };
        self.send(json!({"type": "dictation.cancel", "id": id}));
        Self::release_waiters(&mut g, &id);
        g.attempt = None;
        g.view = g.view.idle_keeping();
        self.emit_view(&g);
        g.view.clone()
    }

    async fn ended_by_helper(
        &self,
        id: String,
        text: String,
        duration_ms: f64,
        kind: TalkErrorKind,
        msg: &str,
    ) {
        let dur = {
            let mut g = self.lock();
            if g.view.id.as_deref() != Some(id.as_str()) || g.view.phase != TalkPhase::Listening {
                return;
            }
            g.key_up = self.mono();
            g.view.phase = TalkPhase::Finalizing;
            g.view.level = 0.0;
            self.emit_view(&g);
            if duration_ms > 0.0 {
                duration_ms
            } else {
                g.key_up - g.key_down
            }
        };
        if !self.finish(id.clone(), text, dur).await {
            // A newer dictation took the view while this one was inserting (N110): its error stays in history.
            return;
        }
        let mut g = self.lock();
        g.view.phase = TalkPhase::Error;
        g.view.error = Some(msg.to_owned());
        g.view.error_kind = Some(kind);
        g.view.level = 0.0;
        self.emit_view(&g);
    }

    /// Flush the on-device session, pick the text, then the on-device cleanup → insert → history.
    ///
    /// From `inserting` on, a newer dictation may own the view (N110): everything this one needs (start time, key
    /// times, first partial) is captured before that, and its end only sets `last` on a view that isn't its own.
    /// Returns whether the view was still this dictation's at the end.
    async fn finish(&self, id: String, local_text: String, duration_ms: f64) -> bool {
        let own = self.finish_inner(id.clone(), local_text, duration_ms).await;
        // N124: a real final that came while this dictation was being inserted is applied now; one still to come is
        // applied when it arrives.
        let stashed = {
            let mut g = self.lock();
            match g.late.remove(&id) {
                Some(Late::Text(t)) => Some(t),
                Some(Late::Pending) => {
                    g.late.insert(id.clone(), Late::Done);
                    None
                }
                Some(Late::Done) | None => None,
            }
        };
        if let Some(t) = stashed {
            self.apply_late(&id, t);
        }
        own
    }

    /// N124: the helper's `dictation.stopped` for a dictation that already went ahead (its stop wait ran out).
    fn late_final(&self, id: &str, text: String) {
        let done = {
            let mut g = self.lock();
            match g.late.get(id) {
                Some(Late::Pending) | Some(Late::Text(_)) => {
                    g.late.insert(id.to_owned(), Late::Text(text.clone()));
                    false
                }
                Some(Late::Done) => {
                    g.late.remove(id);
                    true
                }
                None => return,
            }
        };
        if done {
            self.apply_late(id, text);
        }
    }

    /// N124 (ADR 0081): the full text replaces the shorter one in the dictation's history entry (raw and cleaned),
    /// which says the paste was cut short; nothing is typed again (the user may be typing elsewhere by now). With no
    /// entry (history off, or nothing was shown so nothing was kept) the full text goes to the clipboard instead, the
    /// same "never lose the words" rule as a failed insert.
    fn apply_late(&self, id: &str, text: String) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let cfg = self.cfg();
        let mut g = self.lock();
        let dictionary = g.store.dictionary.clone();
        let Some(pos) = g.store.history.iter().position(|e| e.id == id) else {
            drop(g);
            tracing::warn!("late final with no history entry; copied to the clipboard");
            let cleaned = cleanup_dictation(&text, &dictionary, cfg.auto_cleanup);
            self.deps().clipboard.write(&cleaned.text);
            return;
        };
        if g.store.history[pos].raw.trim() == text {
            return;
        }
        let cleaned = cleanup_dictation(&text, &dictionary, cfg.auto_cleanup);
        tracing::warn!("late final replaced a cut-short dictation in history");
        let e = &mut g.store.history[pos];
        e.raw = text;
        e.text = cleaned.text;
        e.cleanup = cleaned.method;
        e.error = Some(MSG_LATE_FINAL.to_owned());
        let entry = e.clone();
        if g.view.last.as_ref().map(|l| l.id.as_str()) == Some(id) {
            g.view.last = Some(entry);
        }
        self.save(&g);
        self.deps().events.emit("talk:history-changed", json!({"id": id}));
        self.emit_view(&g);
    }

    async fn finish_inner(&self, id: String, local_text: String, duration_ms: f64) -> bool {
        let t = self.deps().timeouts;
        // N165 (ADR 0109): the helper's `dictation.stopped` text wins when non-empty, so don't wait up to
        // `final_ms` for a speech-module flush we won't use. Still call `finish` so the session exits cleanly.
        let wait_for_module = local_text.trim().is_empty();
        let local_rx = {
            let mut g = self.lock();
            let Some(a) = g.attempt.as_mut().filter(|a| a.id == id) else {
                return g.view.id.as_deref() == Some(id.as_str());
            };
            a.local.as_ref().and_then(|l| {
                l.finish();
                if wait_for_module {
                    let (tx, rx) = oneshot::channel();
                    a.local_done = Some(tx);
                    Some(rx)
                } else {
                    None
                }
            })
        };
        // The on-device engine's last finals (only when the helper sent no text; bounded by the flush timeout).
        if let Some(l) = local_rx {
            let _ = tokio::time::timeout(Duration::from_millis(t.final_ms), l).await;
        }
        let final_at = self.mono();
        let text = {
            let mut g = self.lock();
            let Some(a) = g.attempt.take().filter(|a| a.id == id) else {
                return g.view.id.as_deref() == Some(id.as_str());
            };
            // The helper's full text from `dictation.stopped` wins; else what the session heard.
            let text = match local_text.trim() {
                "" => a.live(),
                t => t.to_owned(),
            };
            // Still finalizing, so still this dictation's view and key times (no start is taken until inserting).
            let times = (
                g.view.started_at,
                g.key_down,
                g.key_up,
                g.first_partial,
                g.head_ms,
                g.level.summary(),
                g.asr.clone(),
            );
            (text, times)
        };
        let (text, (started_at, key_down, key_up, first_partial, head_ms, level, segs)) = text;
        if text.trim().is_empty() {
            let mut g = self.lock();
            g.view = g.view.idle_keeping();
            self.emit_view(&g);
            return true;
        }
        let cfg = self.cfg();
        let dictionary = {
            let mut g = self.lock();
            g.view.text = text.clone();
            g.view.partial.clear();
            g.view.engine = Some(Engine::OnDevice);
            g.view.failover = Some(false);
            g.store.dictionary.clone()
        };
        let clean_at = self.mono();
        let cleaned = cleanup_dictation(&text, &dictionary, cfg.auto_cleanup);
        let cleanup_ms = self.mono() - clean_at;
        let mut insert: Option<Value> = None;
        let mut error: Option<String> = None;
        if cfg.auto_insert {
            let rx = {
                let mut g = self.lock();
                g.view.phase = TalkPhase::Inserting;
                g.view.text = cleaned.text.clone();
                self.emit_view(&g);
                Self::waiter(&mut g, format!("insert:{id}"))
            };
            // N142 (ADR 0089): a helper that is gone (it crashed while this dictation finalized) can't paste.
            // Don't wait out the insert bound for an answer that won't come; the clipboard gets the words now.
            let sent = self.send(json!({"type": "insert.text", "id": id, "text": cleaned.text, "restoreClipboard": cfg.restore_clipboard}));
            if sent {
                insert = self.wait(format!("insert:{id}"), rx, t.insert).await;
            } else {
                self.lock().waiters.remove(&format!("insert:{id}"));
            }
            if insert.as_ref().map(|m| m["ok"] == true) != Some(true) {
                // Never lose the words: leave them on the clipboard to paste by hand.
                self.deps().clipboard.write(&cleaned.text);
                let why = insert
                    .as_ref()
                    .and_then(|m| m["error"].as_str())
                    .unwrap_or(if sent {
                        "insert timed out"
                    } else {
                        MSG_NO_HELPER_INSERT
                    })
                    .to_owned();
                error = Some(format!("{why}; copied to clipboard"));
            }
        }
        let inserted_at = self.mono();
        let mut g = self.lock();
        let ok = insert.as_ref().map(|m| m["ok"] == true).unwrap_or(false);
        let method = insert
            .as_ref()
            .and_then(|m| serde_json::from_value::<InsertMethod>(m["method"].clone()).ok())
            .unwrap_or(InsertMethod::None);
        let entry = TalkEntry {
            id: id.clone(),
            at: started_at.unwrap_or_else(|| self.deps().clock.now_ms()),
            raw: text.clone(),
            text: cleaned.text.clone(),
            cleanup: cleaned.method,
            engine: Engine::OnDevice,
            failover: false,
            insert: method,
            inserted: ok,
            app: insert.as_ref().and_then(|m| m["app"].as_str()).map(str::to_owned),
            duration_ms: duration_ms.max(0.0).round(),
            latency: Latency {
                first_partial_ms: first_partial.map(|f| (f - key_down).max(0.0).round()),
                final_ms: Some((final_at - key_up).max(0.0).round()),
                cleanup_ms: Some(cleanup_ms.max(0.0).round()),
                inserted_ms: ok.then(|| (inserted_at - key_up).max(0.0).round()),
            },
            error,
        };
        // ADR 0099: one content-free line per dictation (with the helper's `echo timing:` line, the real-use numbers
        // for docs/echo/dictation-tuning.md): durations, the insert method, and sizes only.
        tracing::info!(
            engine = entry.engine.as_str(),
            audio_ms = entry.duration_ms,
            head_ms = head_ms.round(),
            first_partial_ms = ?entry.latency.first_partial_ms,
            final_ms = ?entry.latency.final_ms,
            cleanup_ms = ?entry.latency.cleanup_ms,
            inserted_ms = ?entry.latency.inserted_ms,
            insert = ?entry.insert,
            cleanup = ?entry.cleanup,
            raw_chars = entry.raw.chars().count(),
            inserted_chars = entry.text.chars().count(),
            level_blocks = level.map(|l| l.blocks),
            level_p10_dbfs = level.map(|l| l.p10_dbfs),
            level_p90_dbfs = level.map(|l| l.p90_dbfs),
            level_saturated = level.map(|l| l.saturated).filter(|n| *n > 0),
            // ADR 0118: the recogniser that heard it, and a segmented one's counts (ids and counts only).
            asr = segs.as_ref().map(|a| a.asr.as_str()),
            segments = segs.as_ref().and_then(|a| a.segments),
            tail_decode_ms = segs.as_ref().and_then(|a| a.tail_decode_ms),
            // ADR 0132: the release tail's confidence, and `tail_dropped = true` only when the floor dropped it.
            tail_conf = segs.as_ref().and_then(|a| a.tail_conf),
            tail_dropped = segs.as_ref().map(|a| a.tail_dropped).filter(|d| *d),
            "talk latency"
        );
        if cfg.save_history {
            g.store.history.insert(0, entry.clone());
            g.store.history.truncate(MAX_HISTORY);
            self.save(&g);
            self.deps().events.emit("talk:history-changed", json!({"id": id}));
        }
        let own = g.view.id.as_deref() == Some(id.as_str());
        if own {
            g.view = TalkView {
                last: Some(entry),
                ..TalkView::idle()
            };
        } else {
            // A newer dictation is listening: keep its view, show this one as the last. (If this insert outlived
            // the whole newer dictation, this entry lands on top: accepted, P4, ADR 0074.)
            g.view.last = Some(entry);
        }
        self.emit_view(&g);
        own
    }
}
