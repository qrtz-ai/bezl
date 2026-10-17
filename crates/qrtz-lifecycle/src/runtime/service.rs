//! The `NoteService` port (apps/desktop/src/main/note-service.ts): the runtime owner of the recording
//! lifecycle. State lives in one place and is only touched under a short, never-held-across-await lock; every
//! async flow is the TS method body line for line (await points included), so a single-threaded tokio runtime
//! reproduces the TS event-loop behaviour and the tier-2 scenarios hold both to the same trace.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use qrtz_model::{
    ActiveSpeakerEvent, AsrEvent, Degradation, EpochMs, HelperHealth, LifecycleError, LifecycleEvent,
    LivePartial, LiveSegment, LiveTranscript, LiveTranscriptDelta, Meeting, MeetingStatus, MeetingSummary,
    OsPermission, PermissionState, PermissionsStatus, PipelineContext, PipelineState, PipelineView,
    RecordingPhase, RecordingState, Segment, SessionLine, SessionView, StepId, StepStatus, StopReason,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;

use super::api::*;
use crate::pipeline::max_retry_delay;
use crate::transcript::TranscriptAssembler;
use crate::{
    can_start, capture_report, initial_state, is_capturing, new_pipeline, next_runnable_step,
    pipeline_settled, pipeline_step, pipeline_usable, policies, progress_label, resume_after_restart,
    retry_delay, silent_input_view, to_view, Machine, StepPatch,
};

/// Everything the runtime needs from the app.
#[derive(Clone)]
pub struct RuntimeDeps {
    pub store: Arc<dyn LifecycleStore>,
    pub steps: Arc<dyn StepExecutor>,
    pub helper: Arc<dyn HelperPort>,
    pub events: Arc<dyn EventSink>,
    /// `deps.config()`: read on every use.
    pub config: Arc<dyn Fn() -> RuntimeConfig + Send + Sync>,
    pub clock: Arc<dyn Clock>,
    /// `randomUUID` (meeting and note ids).
    pub ids: IdGen,
    /// `Math.random` (retry jitter).
    pub random: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub recordings_dir: PathBuf,
    /// `QRTZ_RETRY_SCALE`: multiplies retry delays, ack timeouts and the reconnect window.
    pub retry_scale: f64,
    /// e2e fault injection (unpackaged builds): fail the first attempt of this step per meeting.
    pub fault_step: Option<StepId>,
    /// Title of an untitled recording (TS `Recording ${new Date(now).toLocaleString()}`); the shell injects a
    /// localized one, [`utc_recording_title`] is the default.
    pub default_title: Arc<dyn Fn(EpochMs) -> String + Send + Sync>,
    /// Meetings the user already deleted whose deferred delete has not run yet (undo tombstones). Recovery never
    /// resumes their post-processing (N17; TS `pendingDelete`). `None`: no deferred deletes in this shell.
    pub pending_delete: Option<PendingDelete>,
}

/// Refusal for deleting the meeting that is still recording or flushing (TS `DELETE_WHILE_RECORDING`).
pub const DELETE_WHILE_RECORDING: &str =
    "This meeting is still recording. Stop the recording, then delete it.";

/// [`RuntimeDeps::pending_delete`]: is this meeting's delete confirmed but not yet run?
pub type PendingDelete = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// `Recording YYYY-MM-DD HH:MM` (UTC).
pub fn utc_recording_title(now: EpochMs) -> String {
    let secs = now.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "Recording {y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

const HELPER_GONE: &str = "native helper is not running";
/// TS `WorkerClient.call` timeouts of the step calls.
const FINALIZE_TIMEOUT_MS: u64 = 60_000;
const SUMMARY_TIMEOUT_MS: u64 = 120_000;
const INDEX_TIMEOUT_MS: u64 = 300_000;

/// `degradationsForRevocation` (packages/core/src/helper/arbiter.ts). One Rust copy: the arbiter port's, pinned
/// to spec/vectors/helper; this re-export keeps the old path (N6, ADR 0011).
pub use crate::arbiter::degradations_for_revocation;

fn started_for(audio_path: &str) -> Option<Matcher> {
    let want = audio_path.to_string();
    Some(Box::new(
        move |m| matches!(m, HelperInput::RecordingStarted { audio_path } if *audio_path == want),
    ))
}

fn closed() -> CoreError {
    CoreError::unavailable("the recording runtime is not running")
}

/// `toLifecycleError`.
fn to_lifecycle_error(e: &CoreError) -> LifecycleError {
    LifecycleError {
        code: if e.code.is_empty() {
            "E_INTERNAL".into()
        } else {
            e.code.clone()
        },
        message: e.message.clone(),
        retryable: e.retryable,
    }
}

async fn with_timeout<T>(
    method: &str,
    ms: u64,
    f: impl std::future::Future<Output = Result<T, LifecycleError>>,
) -> Result<T, CoreError> {
    match tokio::time::timeout(Duration::from_millis(ms), f).await {
        Ok(r) => r.map_err(CoreError::from),
        Err(_) => Err(CoreError::timeout(format!("worker {method} timed out"))),
    }
}

/// Handle to the recording runtime; cheap to clone.
#[derive(Clone)]
pub struct NoteRuntime {
    sh: Arc<Shared>,
}

impl NoteRuntime {
    /// Creates the runtime on the current tokio runtime. Call [`NoteRuntime::recover`] next (`start` waits for it).
    pub fn new(deps: RuntimeDeps) -> NoteRuntime {
        let _ = std::fs::create_dir_all(&deps.recordings_dir);
        let (save_tx, mut save_rx) = mpsc::unbounded_channel::<RecordingState>();
        let (ready_tx, _) = watch::channel(false);
        let sh = Arc::new(Shared {
            d: deps,
            st: Mutex::new(State::new()),
            save_tx,
            ready: ready_tx,
            killed: AtomicBool::new(false),
            handles: Mutex::new(Vec::new()),
            heavy: tokio::sync::Mutex::new(()),
        });
        // `saveChain`: snapshots are written in order so an older one never lands after a newer one.
        let w = sh.clone();
        let h = tokio::spawn(async move {
            while let Some(s) = save_rx.recv().await {
                if w.killed() {
                    return;
                }
                if let Err(e) = w.d.store.save_session(s, w.d.clock.now_ms()).await {
                    tracing::warn!(err = %e.message, "session persist failed");
                }
            }
        });
        sh.handles.lock().unwrap().push(h.abort_handle());
        NoteRuntime { sh }
    }

    /// Simulates the process dying (tests) / hard shutdown: every runtime task stops where it is (the store sees
    /// nothing more), pending calls fail with `E_UNAVAILABLE`, and no further event is emitted.
    pub fn kill(&self) {
        self.sh.killed.store(true, Ordering::SeqCst);
        for h in self.sh.handles.lock().unwrap().drain(..) {
            h.abort();
        }
    }

    /// Runs `f` as a runtime task (aborted by [`NoteRuntime::kill`]).
    async fn task<T: Send + 'static>(
        &self,
        f: impl std::future::Future<Output = Result<T, CoreError>> + Send + 'static,
    ) -> Result<T, CoreError> {
        match self.sh.spawn_handle(f) {
            Some(h) => h.await.unwrap_or_else(|_| Err(closed())),
            None => Err(closed()),
        }
    }

    /// Startup recovery (R1–R3), then audio retention. Never fails (errors are logged, like TS).
    pub async fn recover(&self) -> Result<RecoverReport, CoreError> {
        let sh = self.sh.clone();
        self.task(async move { Ok(sh.recover().await) }).await
    }
    pub fn session(&self) -> SessionView {
        self.sh.view()
    }
    /// The raw machine state (tests, diagnostics).
    pub fn state(&self) -> RecordingState {
        self.sh.lock().machine.state().clone()
    }
    pub async fn start(&self, title: Option<String>) -> Result<SessionView, CoreError> {
        let sh = self.sh.clone();
        self.task(async move { sh.start(title).await }).await
    }
    pub async fn stop(&self, reason: StopReason) -> Result<SessionView, CoreError> {
        let sh = self.sh.clone();
        self.task(async move { sh.stop(reason).await }).await
    }
    pub fn dismiss(&self) -> SessionView {
        self.sh.dismiss()
    }
    /// User delete of a meeting (`note:delete`, or the deferred delete after its undo window): stops its
    /// pipeline, drops the rows and removes its audio inside the recordings directory. Returns removed paths.
    pub async fn delete_meeting(&self, id: &str) -> Result<Vec<String>, CoreError> {
        let (sh, id) = (self.sh.clone(), id.to_string());
        self.task(async move { sh.delete_meeting(&id).await }).await
    }
    /// Pipeline bookkeeping sizes `[pipelines, timers, served backoffs, manual flags]` (tests; N14).
    #[doc(hidden)]
    pub fn pipeline_debug(&self) -> [usize; 4] {
        self.sh.pipeline_debug()
    }
    /// Global shortcut / tray: stop when capturing, else start.
    pub async fn toggle(&self) -> Result<SessionView, CoreError> {
        let sh = self.sh.clone();
        self.task(async move { sh.toggle().await }).await
    }
    /// Privacy pause/resume while recording; `None` toggles.
    pub fn pause(&self, want: Option<bool>) -> Result<SessionView, CoreError> {
        if self.sh.killed() {
            return Err(closed());
        }
        self.sh.pause(want)
    }
    pub async fn pipeline(&self, id: &str) -> Result<Option<PipelineView>, CoreError> {
        let (sh, id) = (self.sh.clone(), id.to_string());
        self.task(async move { sh.pipeline(&id).await }).await
    }
    pub async fn retry_step(&self, id: &str, step: StepId) -> Result<PipelineView, CoreError> {
        let (sh, id) = (self.sh.clone(), id.to_string());
        self.task(async move { sh.retry_step(&id, step).await }).await
    }
    pub async fn resummarize(&self, id: &str) -> Result<Option<MeetingSummary>, CoreError> {
        let (sh, id) = (self.sh.clone(), id.to_string());
        self.task(async move { sh.resummarize(&id).await }).await
    }
    pub async fn set_template(&self, id: &str, template: &str) -> Result<MeetingSummary, CoreError> {
        let (sh, id, t) = (self.sh.clone(), id.to_string(), template.to_string());
        self.task(async move { Ok(sh.d.steps.resummarize(id, Some(t)).await?) })
            .await
    }
    pub fn live_transcript(&self, meeting_id: Option<&str>) -> Option<LiveTranscript> {
        self.sh.lock().live_transcript(&self.sh.d, meeting_id)
    }
    pub async fn add_note(&self, req: AddNoteRequest) -> Result<NoteWithMeeting, CoreError> {
        let sh = self.sh.clone();
        self.task(async move { sh.add_note(req).await }).await
    }
    pub fn permissions(&self) -> PermissionsStatus {
        self.sh.lock().permissions.clone()
    }
    pub async fn request_permission(&self, p: OsPermission) -> Result<PermissionsStatus, CoreError> {
        let sh = self.sh.clone();
        self.task(async move { sh.request_permission(p).await }).await
    }
    /// Feed every helper → host message here (`handle`).
    pub fn helper_message(&self, m: HelperInput) {
        if !self.sh.killed() {
            self.sh.handle(m);
        }
    }
    /// A validated helper line as JSON; messages the runtime doesn't consume are dropped.
    pub fn helper_json(&self, v: &serde_json::Value) {
        if let Some(m) = HelperInput::from_json(v) {
            self.helper_message(m);
        }
    }
    /// Supervisor: the helper went away (crash, kill, heartbeat lost). F3.
    pub fn helper_down(&self, reason: &str) {
        if !self.sh.killed() {
            self.sh.on_helper_down(reason);
        }
    }
    /// Supervisor circuit breaker opened: stop reconnecting now. F4.
    pub fn helper_failed(&self, reason: &str) {
        if !self.sh.killed() {
            self.sh.on_helper_failed(reason);
        }
    }
    pub fn set_helper_health(&self, h: HelperHealth, detail: Option<String>) {
        if !self.sh.killed() {
            self.sh.set_helper_health(h, detail);
        }
    }
}

type Matcher = Box<dyn Fn(&HelperInput) -> bool + Send>;

/// Why the helper refused a start (see `busy_refusals`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    Busy,
    File,
    Invariant,
}

struct WaiterEntry {
    id: u64,
    kind: &'static str,
    tx: oneshot::Sender<Option<HelperInput>>,
    /// Correlates the message to this request (the audio path of this start/stop).
    matcher: Option<Matcher>,
    timer: Option<AbortHandle>,
}

struct State {
    machine: Machine,
    asr: TranscriptAssembler,
    active: Vec<ActiveSpeakerEvent>,
    participants: Vec<String>,
    level: f64,
    permissions: PermissionsStatus,
    waiters: Vec<WaiterEntry>,
    /// Audio path of the `recording.start` in flight (start or resume); acks for any other path are stale.
    pending_start_path: Option<String>,
    /// Starts the helper refused, keyed by the refused attempt's file, so a refusal only ever fails the start it
    /// names: `error{recording.busy, audioPath}` (another recording still owns it), `error{recording.file,
    /// audioPath}` (the file could not be created; the helper already tore that recording down), or a busy naming
    /// no active file (the helper broke "a running recording always has a file": last-resort restart).
    busy_refusals: HashMap<String, Refusal>,
    emit_timer: bool,
    persisted: HashSet<String>,
    paused: bool,
    helper_crashed: bool,
    reconnect_timer: Option<AbortHandle>,
    t_rev: u64,
    t_sent: HashSet<String>,
    pipelines: HashMap<String, PipelineState>,
    pipeline_timers: HashMap<String, AbortHandle>,
    /// Per meeting, the `{id}:{step}:{attempts}` whose backoff timer already fired (N12: never wait on the wall
    /// clock twice). One entry per meeting at most, dropped with the pipeline (N14).
    backoff_served: HashMap<String, String>,
    pipeline_running: HashSet<String>,
    start_in_flight: bool,
    helper_health: HelperHealth,
    helper_detail: Option<String>,
    faults_fired: HashSet<String>,
    manual_steps: HashSet<String>,
    seq: u64,
}

/// Outcome of saving a pipeline record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Persisted {
    Saved,
    /// The meeting no longer exists (deleted meanwhile).
    Gone,
    Failed,
}

impl State {
    fn new() -> State {
        State {
            machine: Machine::new(initial_state()),
            asr: TranscriptAssembler::new(),
            active: Vec::new(),
            participants: Vec::new(),
            level: 0.0,
            permissions: [
                OsPermission::Accessibility,
                OsPermission::ScreenRecording,
                OsPermission::Microphone,
            ]
            .into_iter()
            .map(|p| (p, PermissionState::Unknown))
            .collect(),
            waiters: Vec::new(),
            pending_start_path: None,
            busy_refusals: HashMap::new(),
            emit_timer: false,
            persisted: HashSet::new(),
            paused: false,
            helper_crashed: false,
            reconnect_timer: None,
            t_rev: 0,
            t_sent: HashSet::new(),
            pipelines: HashMap::new(),
            pipeline_timers: HashMap::new(),
            backoff_served: HashMap::new(),
            pipeline_running: HashSet::new(),
            start_in_flight: false,
            helper_health: HelperHealth::Ok,
            helper_detail: None,
            faults_fired: HashSet::new(),
            manual_steps: HashSet::new(),
            seq: 0,
        }
    }

    fn speaker_at(&self, ms: i64) -> Option<String> {
        self.active
            .iter()
            .rev()
            .find(|a| a.at_ms <= ms + 1500)
            .map(|a| a.name.clone())
    }

    fn live_segments(&self) -> Vec<LiveSegment> {
        self.asr
            .segments()
            .into_iter()
            .map(|s| LiveSegment {
                speaker: self.speaker_at(s.start_ms),
                id: s.id,
                text: s.text,
                start_ms: s.start_ms,
                end_ms: s.end_ms,
            })
            .collect()
    }

    fn live_partial(&self) -> Option<LivePartial> {
        self.asr.live_partials().pop().map(|p| LivePartial {
            id: p.utterance_id,
            text: p.text,
            start_ms: p.start_ms,
        })
    }

    fn live_transcript(&self, _d: &RuntimeDeps, meeting_id: Option<&str>) -> Option<LiveTranscript> {
        let s = self.machine.state();
        let id = s.meeting_id.as_ref()?;
        if meeting_id.is_some_and(|m| !m.is_empty() && m != id) {
            return None;
        }
        Some(LiveTranscript {
            meeting_id: id.clone(),
            rev: self.t_rev,
            segments: self.live_segments(),
            partial: if is_capturing(s.phase) {
                self.live_partial()
            } else {
                None
            },
        })
    }

    fn view(&self, now: EpochMs) -> SessionView {
        let s = self.machine.state();
        let segs = self.live_segments();
        let lines = segs[segs.len().saturating_sub(6)..]
            .iter()
            .map(|l| SessionLine {
                id: l.id.clone(),
                speaker: l.speaker.clone(),
                text: l.text.clone(),
                start_ms: l.start_ms,
            })
            .collect();
        let pipeline = s.meeting_id.as_ref().and_then(|id| self.pipelines.get(id));
        let live = is_capturing(s.phase);
        let started = s.started_at.filter(|v| *v != 0);
        let denied = |k| self.permissions.get(&k) == Some(&PermissionState::Denied);
        let (silent, no_audio) = silent_input_view(
            s,
            denied(OsPermission::Microphone),
            denied(OsPermission::ScreenRecording),
        );
        SessionView {
            phase: s.phase,
            detected: s.detected.clone(),
            meeting_id: s.meeting_id.clone(),
            started_at: started,
            elapsed_ms: started.map_or(0, |st| s.stopped_at.unwrap_or(now) - st),
            participants: self.participants.clone(),
            active_speaker: self.active.last().map(|a| a.name.clone()),
            level: if s.phase == RecordingPhase::Recording {
                self.level
            } else {
                0.0
            },
            lines,
            partial: if live {
                self.live_partial().map(|p| p.text).unwrap_or_default()
            } else {
                String::new()
            },
            progress: (s.phase == RecordingPhase::Processing).then(|| progress_label(pipeline)),
            error: s.error.as_ref().map(|e| e.message.clone()),
            helper: self.helper_health,
            helper_detail: self.helper_detail.clone().filter(|d| !d.is_empty()),
            paused: s.phase == RecordingPhase::Recording && self.paused,
            stop_reason: s.stop_reason,
            degraded: s.degraded.clone(),
            device_lost: s.device_lost.clone(),
            error_info: s.error.clone(),
            reconnect: s.reconnect.clone(),
            pipeline: pipeline.map(to_view),
            silent,
            no_audio,
        }
    }
}

struct Shared {
    d: RuntimeDeps,
    st: Mutex<State>,
    save_tx: mpsc::UnboundedSender<RecordingState>,
    ready: watch::Sender<bool>,
    killed: AtomicBool,
    handles: Mutex<Vec<AbortHandle>>,
    /// `heavyChain`.
    heavy: tokio::sync::Mutex<()>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }
    fn now(&self) -> EpochMs {
        self.d.clock.now_ms()
    }
    fn scale(&self) -> f64 {
        self.d.retry_scale
    }
    fn config(&self) -> RuntimeConfig {
        (self.d.config)()
    }
    fn emit(&self, e: CoreEvent) {
        if !self.killed() {
            self.d.events.emit(e);
        }
    }
    fn view(&self) -> SessionView {
        let now = self.now();
        self.lock().view(now)
    }
    fn emit_session(&self) {
        let v = self.view();
        self.emit(CoreEvent::Session(v));
    }

    fn spawn_handle<T: Send + 'static>(
        &self,
        f: impl std::future::Future<Output = T> + Send + 'static,
    ) -> Option<tokio::task::JoinHandle<T>> {
        if self.killed() {
            return None;
        }
        let h = tokio::spawn(f);
        let mut hs = self.handles.lock().unwrap();
        if hs.len() > 64 {
            hs.retain(|h| !h.is_finished());
        }
        hs.push(h.abort_handle());
        Some(h)
    }
    /// `void promise`: a detached runtime task.
    fn spawn(&self, f: impl std::future::Future<Output = ()> + Send + 'static) -> Option<AbortHandle> {
        self.spawn_handle(f).map(|h| h.abort_handle())
    }
    /// `setTimeout(f, ms)`.
    fn set_timeout(
        self: &Arc<Self>,
        ms: f64,
        f: impl FnOnce(Arc<Shared>) + Send + 'static,
    ) -> Option<AbortHandle> {
        let ms = if ms.is_finite() && ms > 0.0 { ms } else { 0.0 };
        let sh = self.clone();
        self.spawn(async move {
            tokio::time::sleep(Duration::from_micros((ms * 1000.0).round() as u64)).await;
            if !sh.killed() {
                f(sh);
            }
        })
    }

    /// `machine.send` + the `onChange` listener (persist through the ordered writer, broadcast synchronously).
    fn send(&self, e: LifecycleEvent) -> bool {
        let now = self.now();
        let (s, v) = {
            let mut st = self.lock();
            let Some(s) = st.machine.send(&e).cloned() else {
                return false;
            };
            let v = st.view(now);
            (s, v)
        };
        tracing::info!(phase = s.phase.as_str(), "lifecycle");
        self.persist_session(s);
        self.emit(CoreEvent::Session(v));
        true
    }

    fn persist_session(&self, s: RecordingState) {
        if !self.killed() {
            let _ = self.save_tx.send(s);
        }
    }

    /// Stamped with the wall clock, so a silent stretch of a source the flag takes away ends now (N29).
    fn degrade(&self, flag: Degradation) {
        let at = Some(self.now());
        if self.send(LifecycleEvent::Degraded { flag, at }) {
            tracing::warn!(flag = flag.as_str(), "capture degraded");
        }
    }

    fn emit_soon(self: &Arc<Self>, delay: f64) {
        {
            let mut st = self.lock();
            if st.emit_timer {
                return;
            }
            st.emit_timer = true;
        }
        self.set_timeout(delay, |sh| {
            sh.lock().emit_timer = false;
            sh.emit_session();
        });
    }

    /// `this.send(m)`: `E_UNAVAILABLE` when the helper isn't running.
    fn send_helper(&self, c: HelperCommand) -> Result<(), CoreError> {
        if self.killed() || !self.d.helper.send(&c) {
            return Err(CoreError::unavailable(HELPER_GONE));
        }
        Ok(())
    }

    /// `waitFor(type, ms)`: registered now; resolves with the next message of that type or `None` on timeout.
    fn wait_for(
        self: &Arc<Self>,
        kind: &'static str,
        ms: f64,
        matcher: Option<Matcher>,
    ) -> oneshot::Receiver<Option<HelperInput>> {
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut st = self.lock();
            st.seq += 1;
            st.seq
        };
        let timer = self.set_timeout(ms, move |sh| {
            let w = {
                let mut st = sh.lock();
                st.waiters
                    .iter()
                    .position(|w| w.id == id)
                    .map(|i| st.waiters.remove(i))
            };
            if let Some(w) = w {
                let _ = w.tx.send(None);
            }
        });
        self.lock().waiters.push(WaiterEntry {
            id,
            kind,
            tx,
            matcher,
            timer,
        });
        rx
    }

    fn ack_ms(&self, base: u64) -> f64 {
        base as f64 * self.scale().max(0.2)
    }

    fn emit_transcript(&self, meeting_id: &str) {
        let delta = {
            let mut st = self.lock();
            let append: Vec<LiveSegment> = st
                .live_segments()
                .into_iter()
                .filter(|x| !st.t_sent.contains(&x.id))
                .collect();
            for a in &append {
                st.t_sent.insert(a.id.clone());
            }
            let base_rev = st.t_rev;
            st.t_rev += 1;
            let partial = if is_capturing(st.machine.state().phase) {
                st.live_partial()
            } else {
                None
            };
            LiveTranscriptDelta {
                meeting_id: meeting_id.to_string(),
                base_rev,
                rev: st.t_rev,
                append,
                partial,
            }
        };
        self.emit(CoreEvent::Transcript(delta));
    }

    /// `false` when the save failed (callers use it as the step-start durability barrier).
    /// `Gone`: the store refused because the meeting was deleted meanwhile; stop driving it (N16).
    async fn persist_pipeline(self: &Arc<Self>, p: PipelineState) -> Persisted {
        let current = {
            let mut st = self.lock();
            st.pipelines.insert(p.meeting_id.clone(), p.clone());
            st.machine.state().meeting_id.as_deref() == Some(p.meeting_id.as_str())
        };
        self.emit(CoreEvent::Pipeline(to_view(&p)));
        if current {
            self.emit_soon(0.0);
        }
        let settled = pipeline_settled(&p);
        match self.d.store.save_pipeline(p, settled).await {
            Ok(true) => Persisted::Saved,
            Ok(false) => Persisted::Gone,
            Err(e) => {
                tracing::warn!(err = %e.message, "pipeline persist failed");
                Persisted::Failed
            }
        }
    }

    /// The meeting was deleted: drop every bit of host state for its pipeline (timer, served backoff, manual
    /// flags) so nothing runs or retries for it again (N14, N16). The session lets go of it too.
    fn forget_pipeline(&self, id: &str) {
        let phase = {
            let mut st = self.lock();
            if let Some(t) = st.pipeline_timers.remove(id) {
                t.abort();
            }
            st.pipelines.remove(id);
            st.backoff_served.remove(id);
            let prefix = format!("{id}:");
            st.manual_steps.retain(|k| !k.starts_with(&prefix));
            let s = st.machine.state();
            (s.meeting_id.as_deref() == Some(id)).then_some(s.phase)
        };
        match phase {
            Some(RecordingPhase::Processing) => {
                self.send(LifecycleEvent::ProcessingDone);
                self.dismiss();
            }
            Some(RecordingPhase::Error | RecordingPhase::Done) => {
                self.dismiss();
            }
            _ => {}
        }
    }

    /// Pipeline bookkeeping sizes (pipelines, timers, served backoffs, manual flags) for tests (N14).
    fn pipeline_debug(&self) -> [usize; 4] {
        let st = self.lock();
        [
            st.pipelines.len(),
            st.pipeline_timers.len(),
            st.backoff_served.len(),
            st.manual_steps.len(),
        ]
    }

    // ------------------------------------------------------------------ helper I/O

    /// `handle(m)`: waiters of this type first, then the message itself.
    fn handle(self: &Arc<Self>, m: HelperInput) {
        let kind = m.type_name();
        let (s, ready) = {
            let mut st = self.lock();
            let mut ready = Vec::new();
            let mut i = 0;
            while i < st.waiters.len() {
                let w = &st.waiters[i];
                if w.kind == kind && w.matcher.as_ref().is_none_or(|f| f(&m)) {
                    ready.push(st.waiters.remove(i));
                } else {
                    i += 1;
                }
            }
            (st.machine.state().clone(), ready)
        };
        for w in ready {
            if let Some(t) = &w.timer {
                t.abort();
            }
            let _ = w.tx.send(Some(m.clone()));
        }
        match m {
            HelperInput::Hello {} => {
                if self.send_helper(HelperCommand::PermissionsCheck).is_err() {
                    return; // TS: `send` throws out of `handle`
                }
                if s.phase == RecordingPhase::Reconnecting {
                    let sh = self.clone();
                    self.spawn(async move { sh.resume_after_restart().await });
                }
            }
            HelperInput::PermissionsStatus { status } => {
                let p = {
                    let mut st = self.lock();
                    st.permissions.extend(status);
                    st.permissions.clone()
                };
                self.emit(CoreEvent::PermissionsChanged(p));
            }
            HelperInput::PermissionRevoked {
                permission, degraded, ..
            } => {
                // H5: TCC withdrawn mid-capture: degrade live. N4: apply the helper arbiter's list; derive it from
                // our own (possibly `unknown`) snapshot only for an older helper that omits it.
                let denied: Vec<OsPermission> = {
                    let st = self.lock();
                    st.permissions
                        .iter()
                        .filter(|(_, v)| **v == PermissionState::Denied)
                        .map(|(k, _)| *k)
                        .collect()
                };
                for f in degraded.unwrap_or_else(|| degradations_for_revocation(permission, &denied)) {
                    self.degrade(f);
                }
                let p = {
                    let mut st = self.lock();
                    st.permissions.insert(permission, PermissionState::Denied);
                    st.permissions.clone()
                };
                self.emit(CoreEvent::PermissionsChanged(p));
                tracing::warn!(permission = permission.as_str(), "permission revoked mid-capture");
            }
            HelperInput::Participants { names } => {
                self.lock().participants = names;
                self.emit_soon(50.0);
            }
            HelperInput::MeetingDetected {
                key,
                title,
                platform,
                app,
            } => {
                let cfg = self.config();
                if cfg.is_excluded(&app, platform.as_str()) {
                    tracing::info!(platform = platform.as_str(), "meeting ignored (excluded app)");
                    return;
                }
                let meeting = qrtz_model::DetectedMeeting {
                    key,
                    title: title.clone(),
                    platform,
                    app,
                };
                if cfg.auto_detect_meetings
                    && (cfg.auto_record || cfg.detect_prompt)
                    && self.send(LifecycleEvent::Detected { meeting })
                {
                    if self.config().auto_record {
                        let sh = self.clone();
                        self.spawn(async move {
                            if let Err(e) = sh.start(None).await {
                                tracing::warn!(err = %e, "auto-record failed");
                            }
                        });
                    } else {
                        self.emit(CoreEvent::DetectedPrompt { title, platform });
                    }
                }
            }
            HelperInput::MeetingEnded { key } => {
                if is_capturing(s.phase) && s.detected.as_ref().is_some_and(|d| d.key == key) {
                    let sh = self.clone();
                    self.spawn(async move {
                        let _ = sh.stop(StopReason::MeetingEnded).await;
                    });
                } else {
                    self.send(LifecycleEvent::MeetingEnded { key });
                }
            }
            HelperInput::ActiveSpeaker { at_ms, name } => {
                if is_capturing(s.phase) {
                    self.lock().active.push(ActiveSpeakerEvent {
                        at_ms: at_ms + s.part_offset_ms,
                        name,
                    });
                    self.emit_soon(50.0);
                }
            }
            HelperInput::AudioLevel { rms } => {
                self.lock().level = rms;
                self.emit_soon(150.0);
            }
            HelperInput::RecordingStarted { audio_path } => {
                // A late ack for a start we already abandoned (timeout, give-up): never leave that capture running.
                let pending = self.lock().pending_start_path.clone();
                if pending.as_deref() != Some(audio_path.as_str())
                    && !(is_capturing(s.phase) && s.audio_parts.contains(&audio_path))
                {
                    tracing::warn!("stale recording.started; stopping it");
                    // With the path, so a late started{A} can never stop the recording that replaced it.
                    let _ = self.send_helper(HelperCommand::RecordingStop {
                        audio_path: Some(audio_path),
                    });
                }
            }
            HelperInput::Asr { event } => {
                // `stopping` is accepted: the helper flushes the recogniser's last finals before recording.stopped.
                if !matches!(s.phase, RecordingPhase::Recording | RecordingPhase::Stopping)
                    || s.meeting_id.is_none()
                    || self.lock().paused
                {
                    return;
                }
                self.on_asr(&s, event);
            }
            HelperInput::Error {
                code,
                message,
                audio_path,
                active_audio_path,
            } => {
                tracing::warn!(code = %code, message = %message, "helper error");
                if code == "recording.busy" {
                    self.on_busy(&s, audio_path, active_audio_path);
                    return;
                }
                if code == "recording.file" {
                    self.on_file_failed(audio_path);
                    return;
                }
                match code.as_str() {
                    "asr.unavailable" => self.degrade(Degradation::AsrUnavailable),
                    "audio.unavailable" => self.degrade(Degradation::AudioUnavailable),
                    _ => {}
                }
            }
            HelperInput::CaptureSilent { source, since_ms } => {
                // N29: live and transient (capture.audible, a helper loss or the stop end it); not a degradation.
                if self.send(LifecycleEvent::InputSilent { source, since_ms }) {
                    tracing::warn!(source = source.as_str(), since_ms, "capture silent");
                    if self.view().no_audio {
                        tracing::warn!("no audio from any capture source");
                    }
                }
            }
            HelperInput::CaptureAudible { source } => {
                let at = self.now();
                if self.send(LifecycleEvent::InputAudible { source, at }) {
                    tracing::info!(source = source.as_str(), "capture audible");
                }
            }
            HelperInput::CaptureDegraded { degraded, reason } => {
                // N35: a capture device failed inside a running source: like a revoke (N4), kept as a device failure.
                let at = self.now();
                let flags: Vec<&str> = degraded.iter().map(|d| d.as_str()).collect();
                let flags = flags.join(",");
                if self.send(LifecycleEvent::CaptureDegraded {
                    degraded,
                    reason: reason.clone(),
                    at,
                }) {
                    tracing::warn!(degraded = %flags, reason = %reason, "capture device failed");
                    if self.view().no_audio {
                        tracing::warn!("no audio from any capture source");
                    }
                }
            }
            HelperInput::CaptureFallback { from, to, reason } => {
                // H8: kept on the session (then the meeting's pipeline); `to: null` = nothing left to capture.
                let at = self.now();
                self.send(LifecycleEvent::CaptureFallback { from, to, reason, at });
            }
            HelperInput::RecordingStopped { .. } | HelperInput::RecordingAborted { .. } => {}
        }
    }

    /// `error{recording.busy, audioPath, activeAudioPath}` (port of NoteService.onBusy). Acts only on the start in
    /// flight. A busy naming that same file as active is a duplicate start of a running recording: treat it as the
    /// ack. If the active file is not one of ours (an orphan from before a reload or an abandoned attempt), stop it
    /// by its path before failing this attempt, so the retry can start; never stop a recording we are capturing.
    fn on_busy(self: &Arc<Self>, s: &RecordingState, audio_path: Option<String>, active: Option<String>) {
        let pending = self.lock().pending_start_path.clone();
        let Some(path) = audio_path.filter(|p| pending.as_deref() == Some(p.as_str())) else {
            tracing::warn!("recording.busy for another start; ignored");
            return;
        };
        if active.as_deref() == Some(path.as_str()) {
            self.resolve_started(&path, true);
            return;
        }
        let Some(a) = active else {
            // The helper says a recording runs but names no file: it broke its own invariant (a running recording
            // always has a file). Nothing can stop that recording by path, and a pathless stop is never sent (it
            // would stop whatever runs). A bug, not a normal path: the start fails and the helper is restarted.
            tracing::error!(
                audio_path = %path,
                "BUG: helper invariant broken: recording.busy names no active recording file; restarting the helper"
            );
            self.lock().busy_refusals.insert(path.clone(), Refusal::Invariant);
            self.resolve_started(&path, false);
            return;
        };
        let ours = is_capturing(s.phase) && s.audio_parts.contains(&a);
        if !ours {
            tracing::warn!(audio_path = %a, "stopping orphaned recording");
            let _ = self.send_helper(HelperCommand::RecordingStop {
                audio_path: Some(a.clone()),
            });
        }
        // The start was ignored: fail it now instead of waiting out the ack timeout.
        self.lock().busy_refusals.insert(path.clone(), Refusal::Busy);
        self.resolve_started(&path, false);
    }

    /// `error{recording.file, audioPath}` (port of NoteService.onFileFailed): the helper could not create the file of
    /// the start in flight and already tore that recording down (fail closed; no `recording.started` follows), so
    /// the start fails now. Any other `recording.file` error (a WAV that would not finalize at stop) only logs.
    fn on_file_failed(self: &Arc<Self>, audio_path: Option<String>) {
        let pending = self.lock().pending_start_path.clone();
        let Some(path) = audio_path.filter(|p| pending.as_deref() == Some(p.as_str())) else {
            return;
        };
        self.lock().busy_refusals.insert(path.clone(), Refusal::File);
        self.resolve_started(&path, false);
    }

    /// Resolves the `recording.started` waiters for `path`: with a synthetic ack, or with `None` (refused).
    fn resolve_started(&self, path: &str, ack: bool) {
        let probe = HelperInput::RecordingStarted {
            audio_path: path.to_string(),
        };
        let ws: Vec<WaiterEntry> = {
            let mut st = self.lock();
            let (ws, keep) = std::mem::take(&mut st.waiters)
                .into_iter()
                .partition(|w| w.kind == "recording.started" && w.matcher.as_ref().is_none_or(|m| m(&probe)));
            st.waiters = keep;
            ws
        };
        for w in ws {
            if let Some(t) = &w.timer {
                t.abort();
            }
            let _ = w.tx.send(ack.then(|| probe.clone()));
        }
    }

    fn on_asr(self: &Arc<Self>, s: &RecordingState, ev: AsrEvent) {
        let meeting_id = s.meeting_id.clone().unwrap_or_default();
        let part = s.audio_parts.len() as i64 - 1;
        let off = s.part_offset_ms;
        let ns = |id: String| if part > 0 { format!("p{part}-{id}") } else { id };
        let e = match ev {
            AsrEvent::Partial {
                utterance_id,
                start_ms,
                text,
            } => AsrEvent::Partial {
                utterance_id: ns(utterance_id),
                start_ms: start_ms + off,
                text,
            },
            AsrEvent::Final {
                utterance_id,
                start_ms,
                end_ms,
                text,
                confidence,
            } => AsrEvent::Final {
                utterance_id: ns(utterance_id),
                start_ms: start_ms + off,
                end_ms: end_ms + off,
                text,
                confidence,
            },
        };
        let append = {
            let mut st = self.lock();
            st.asr.push(&e);
            match &e {
                AsrEvent::Final {
                    utterance_id,
                    start_ms,
                    ..
                } => {
                    let seg = st.asr.segments().into_iter().find(|x| &x.id == utterance_id);
                    let speaker = st.speaker_at(*start_ms);
                    match seg {
                        Some(mut seg) if !st.persisted.contains(&seg.id) => {
                            st.persisted.insert(seg.id.clone());
                            if speaker.is_some() {
                                seg.speaker_name = speaker;
                            }
                            Some(seg)
                        }
                        _ => None,
                    }
                }
                AsrEvent::Partial { .. } => None,
            }
        };
        if let Some(seg) = append {
            let (sh, id) = (self.clone(), meeting_id.clone());
            self.spawn(async move { sh.append_live(id, seg).await });
        }
        self.emit_transcript(&meeting_id);
        self.emit_soon(50.0);
    }

    /// Persists one final segment with retry (idempotent upsert).
    async fn append_live(self: Arc<Self>, meeting_id: String, seg: Segment) {
        let p = policies::APPEND_LIVE;
        let mut attempt = 1;
        loop {
            match self.d.store.append_live(meeting_id.clone(), seg.clone()).await {
                Ok(()) => return,
                Err(e) => {
                    if attempt >= p.max_attempts {
                        // Not lost: still in the assembler; stop re-flushes everything before finalize.
                        tracing::error!(err = %e.message, "appendLive gave up; will flush at finalize");
                        self.lock().persisted.remove(&seg.id);
                        return;
                    }
                    let delay = retry_delay(&p, attempt, (self.d.random)(), self.scale());
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn request_permission(self: Arc<Self>, p: OsPermission) -> Result<PermissionsStatus, CoreError> {
        self.send_helper(HelperCommand::PermissionsRequest { permission: p })?;
        let _ = self.wait_for("permissions.status", 5000.0, None).await;
        Ok(self.lock().permissions.clone())
    }

    fn set_helper_health(&self, h: HelperHealth, detail: Option<String>) {
        {
            let mut st = self.lock();
            if h == st.helper_health && detail == st.helper_detail {
                return;
            }
            st.helper_health = h;
            st.helper_detail = detail;
        }
        self.emit_session();
    }

    async fn pipeline(self: Arc<Self>, id: &str) -> Result<Option<PipelineView>, CoreError> {
        let cached = self.lock().pipelines.get(id).cloned();
        let p = match cached {
            Some(p) => Some(p),
            None => self.d.store.get_pipeline(id.to_string()).await?,
        };
        Ok(p.as_ref().map(to_view))
    }

    fn pause(&self, want: Option<bool>) -> Result<SessionView, CoreError> {
        let (phase, paused) = {
            let st = self.lock();
            (st.machine.state().phase, st.paused)
        };
        if phase != RecordingPhase::Recording {
            return Err(CoreError::validation("not recording"));
        }
        let next = want.unwrap_or(!paused);
        if next != paused {
            self.send_helper(if next {
                HelperCommand::RecordingPause
            } else {
                HelperCommand::RecordingResume
            })?;
            self.lock().paused = next;
            tracing::info!(paused = next, "capture pause");
        }
        self.emit_session();
        Ok(self.view())
    }

    async fn add_note(self: Arc<Self>, req: AddNoteRequest) -> Result<NoteWithMeeting, CoreError> {
        let s = self.lock().machine.state().clone();
        let req_id = req.meeting_id.filter(|m| !m.is_empty());
        let live = req_id.is_none() || req_id == s.meeting_id;
        let Some(meeting_id) = req_id.or(s.meeting_id.clone()) else {
            return Err(CoreError::validation("no meeting is being recorded"));
        };
        let at_ms = match req.at_ms {
            Some(v) => v,
            None => match s.started_at.filter(|v| live && *v != 0) {
                Some(st) => s.stopped_at.unwrap_or_else(|| self.now()) - st,
                None => 0,
            },
        };
        Ok(self
            .d
            .store
            .add_note(meeting_id, req.text, at_ms, self.now(), self.d.ids.clone())
            .await?)
    }

    // ------------------------------------------------------------------ start

    async fn start(self: Arc<Self>, title: Option<String>) -> Result<SessionView, CoreError> {
        {
            let mut st = self.lock();
            if st.start_in_flight {
                return Err(CoreError::validation("already starting"));
            }
            st.start_in_flight = true;
        }
        let r = self.clone().start_inner(title).await;
        self.lock().start_in_flight = false;
        r
    }

    async fn start_inner(self: Arc<Self>, title: Option<String>) -> Result<SessionView, CoreError> {
        let mut ready = self.ready.subscribe();
        let _ = ready.wait_for(|v| *v).await;
        let (s, p, helper, participants) = {
            let st = self.lock();
            (
                st.machine.state().clone(),
                st.permissions.clone(),
                st.helper_health,
                st.participants.clone(),
            )
        };
        if helper == HelperHealth::Missing {
            return Err(CoreError::new(
                "E_UNAVAILABLE",
                "Recording is unavailable: the native capture helper is missing from this build.",
                true,
            ));
        }
        if !can_start(s.phase) {
            return Err(CoreError::validation("already recording"));
        }
        let perm = |k| p.get(&k).copied().unwrap_or(PermissionState::Unknown);
        if perm(OsPermission::ScreenRecording) == PermissionState::Denied
            && perm(OsPermission::Microphone) == PermissionState::Denied
        {
            let error = LifecycleError {
                code: "permission-denied".into(),
                message: "Recording needs Screen & System Audio Recording or Microphone access. Grant one in System Settings › Privacy & Security, then try again.".into(),
                retryable: true,
            };
            let msg = error.message.clone();
            self.send(LifecycleEvent::Rejected { error });
            return Err(CoreError::forbidden(msg));
        }
        let id = (self.d.ids)();
        {
            let mut st = self.lock();
            st.asr = TranscriptAssembler::new();
            st.active.clear();
            st.persisted.clear();
            st.t_sent.clear();
            st.t_rev = 0;
            st.paused = false;
            st.helper_crashed = false;
        }
        let audio_path = self
            .d
            .recordings_dir
            .join(format!("{id}.wav"))
            .to_string_lossy()
            .into_owned();
        let now = self.now();
        let title = title
            .filter(|t| !t.is_empty())
            .or_else(|| {
                s.detected
                    .as_ref()
                    .map(|d| d.title.clone())
                    .filter(|t| !t.is_empty())
            })
            .unwrap_or_else(|| (self.d.default_title)(now));
        self.d
            .store
            .create_meeting(Meeting {
                id: id.clone(),
                title,
                platform: s
                    .detected
                    .as_ref()
                    .map_or(qrtz_model::MeetingPlatform::Other, |d| d.platform),
                started_at: now,
                ended_at: None,
                participants,
                status: MeetingStatus::Recording,
                audio_path: Some(audio_path.clone()),
                template: Some(self.config().default_template),
                duration_ms: None,
            })
            .await?;
        self.send(LifecycleEvent::StartRequested {
            meeting_id: id.clone(),
            at: now,
            audio_path: audio_path.clone(),
        });
        self.emit(CoreEvent::Changed { id: id.clone() });
        {
            let mut st = self.lock();
            st.pending_start_path = Some(audio_path.clone());
            st.busy_refusals.remove(&audio_path);
        }
        let acked = self.wait_for(
            "recording.started",
            self.ack_ms(policies::START_ACK_MS),
            started_for(&audio_path),
        );
        if let Err(e) = self.send_helper(HelperCommand::RecordingStart {
            audio_path: audio_path.clone(),
        }) {
            if self.lock().machine.state().phase == RecordingPhase::Starting {
                self.abort_start(&id, to_lifecycle_error(&e)).await;
                return Err(e);
            }
            return Ok(self.view());
        }
        let acked = acked.await.ok().flatten();
        self.clear_pending_start(&audio_path);
        let still_ours = |sh: &Shared| {
            let st = sh.lock();
            let s = st.machine.state();
            s.meeting_id.as_deref() == Some(id.as_str()) && s.phase == RecordingPhase::Starting
        };
        if acked.is_none() {
            // The helper may have died meanwhile: then `reconnecting` owns the meeting, do not delete it.
            if !still_ours(&self) {
                return Ok(self.view());
            }
            // Only a refusal that named this attempt's file counts; one for any other start never relabels it.
            let refusal = self.lock().busy_refusals.remove(&audio_path);
            let error = match refusal {
                Some(Refusal::File) => LifecycleError {
                    code: "recording-file".into(),
                    message:
                        "The recording file could not be created. Check free disk space, then try again."
                            .into(),
                    retryable: true,
                },
                Some(_) => LifecycleError {
                    code: "helper-busy".into(),
                    message: "The capture helper is still busy with another recording. Try again.".into(),
                    retryable: true,
                },
                None => LifecycleError {
                    code: "start-timeout".into(),
                    message: "The capture helper did not start recording. Try again.".into(),
                    retryable: true,
                },
            };
            if refusal != Some(Refusal::Invariant) {
                // Only this attempt's file: never the recording that made the helper busy.
                let _ = self.send_helper(HelperCommand::RecordingStop {
                    audio_path: Some(audio_path.clone()),
                });
            }
            let msg = error.message.clone();
            self.abort_start(&id, error).await;
            // Last-resort watchdog: a helper that broke the file invariant is restarted. After abort_start, so the
            // restart never looks like a crash of this (already failed) start.
            if refusal == Some(Refusal::Invariant) {
                self.d.helper.restart();
            }
            let busy = refusal.is_some();
            return Err(if busy {
                CoreError::unavailable(msg)
            } else {
                CoreError::timeout(msg)
            });
        }
        if !still_ours(&self) {
            return Ok(self.view()); // helper crashed during start: the reconnect path owns it now
        }
        self.send(LifecycleEvent::StartAcked);
        if perm(OsPermission::ScreenRecording) == PermissionState::Denied {
            self.degrade(Degradation::MicOnly);
        }
        if perm(OsPermission::Accessibility) == PermissionState::Denied {
            self.degrade(Degradation::AxUnavailable);
        }
        tracing::info!(id = %id, "recording started");
        Ok(self.view())
    }

    /// N2: `recording.abort{audioPath}` with a hard deadline. True when the helper answered
    /// `recording.aborted{audioPath}` (no writer on that file remains); false on timeout or a dead helper.
    async fn abort_recording(self: &Arc<Self>, audio_path: &str) -> bool {
        let want = audio_path.to_owned();
        let matcher: Matcher = Box::new(
            move |m| matches!(m, HelperInput::RecordingAborted { audio_path } if *audio_path == want),
        );
        let p = self.wait_for(
            "recording.aborted",
            self.ack_ms(policies::ABORT_ACK_MS),
            Some(matcher),
        );
        if self
            .send_helper(HelperCommand::RecordingAbort {
                audio_path: audio_path.to_owned(),
            })
            .is_err()
        {
            return false;
        }
        p.await.ok().flatten().is_some()
    }

    async fn abort_start(self: &Arc<Self>, id: &str, error: LifecycleError) {
        self.send(LifecycleEvent::StartFailed { error });
        // Nothing was captured: drop the empty meeting (and any file the helper began) instead of a broken row.
        let _ = self.drop_meeting(id).await;
        self.emit(CoreEvent::Changed { id: id.to_string() });
    }

    /// User delete: forget the pipeline first so no retry timer races the delete, then rows, then audio (N15).
    async fn delete_meeting(self: &Arc<Self>, id: &str) -> Result<Vec<String>, CoreError> {
        {
            let g = self.lock();
            let s = g.machine.state();
            if s.meeting_id.as_deref() == Some(id)
                && (is_capturing(s.phase) || s.phase == RecordingPhase::Stopping)
            {
                return Err(CoreError::validation(DELETE_WHILE_RECORDING));
            }
        }
        self.forget_pipeline(id);
        self.drop_meeting(id).await
    }

    /// Rows, then the files inside the recordings directory (no session or pipeline side effects).
    async fn drop_meeting(self: &Arc<Self>, id: &str) -> Result<Vec<String>, CoreError> {
        let listed = self.d.store.delete_meeting(id.to_string()).await?;
        let mut removed = Vec::new();
        for f in meeting_audio(&self.d.recordings_dir, id, &listed) {
            if !inside_dir(&self.d.recordings_dir, &f) {
                tracing::warn!(id, "not removing audio outside recordings");
                continue;
            }
            match std::fs::remove_file(&f) {
                Ok(()) => removed.push(f.to_string_lossy().into_owned()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    removed.push(f.to_string_lossy().into_owned())
                }
                Err(e) => tracing::warn!(id, err = %e, "audio delete failed"),
            }
        }
        Ok(removed)
    }

    // ------------------------------------------------------------------ helper crash / reconnect

    fn on_helper_down(self: &Arc<Self>, reason: &str) {
        let s = self.lock().machine.state().clone();
        if !is_capturing(s.phase) || s.meeting_id.is_none() {
            return;
        }
        self.lock().helper_crashed = true;
        let now = self.now();
        let window = policies::HELPER_RECONNECT_WINDOW_MS as f64 * self.scale();
        tracing::warn!(reason, "helper went away mid-recording; reconnecting");
        self.send(LifecycleEvent::HelperLost {
            at: now,
            deadline_at: now + window.round() as i64,
        });
        if self.lock().reconnect_timer.is_none() {
            let t = self.set_timeout(window, |sh| {
                sh.lock().reconnect_timer = None;
                if sh.lock().machine.state().phase == RecordingPhase::Reconnecting {
                    let s2 = sh.clone();
                    sh.spawn(async move { s2.give_up_reconnect("reconnect window elapsed").await });
                }
            });
            self.lock().reconnect_timer = t;
        }
    }

    fn on_helper_failed(self: &Arc<Self>, reason: &str) {
        if self.lock().machine.state().phase == RecordingPhase::Reconnecting {
            let (sh, reason) = (self.clone(), reason.to_string());
            self.spawn(async move { sh.give_up_reconnect(&reason).await });
        }
    }

    fn clear_pending_start(&self, audio_path: &str) {
        let mut st = self.lock();
        if st.pending_start_path.as_deref() == Some(audio_path) {
            st.pending_start_path = None;
        }
    }

    fn clear_reconnect_timer(&self) {
        if let Some(t) = self.lock().reconnect_timer.take() {
            t.abort();
        }
    }

    async fn give_up_reconnect(self: Arc<Self>, reason: &str) {
        self.clear_reconnect_timer();
        tracing::warn!(reason, "helper did not come back; salvaging");
        if let Err(e) = self.stop(StopReason::HelperLost).await {
            tracing::error!(err = %e, "salvage stop failed");
        }
    }

    async fn resume_after_restart(self: Arc<Self>) {
        let s = self.lock().machine.state().clone();
        let Some(meeting_id) = s
            .meeting_id
            .clone()
            .filter(|_| s.phase == RecordingPhase::Reconnecting)
        else {
            return;
        };
        let part = s.audio_parts.len();
        let audio_path = self
            .d
            .recordings_dir
            .join(format!("{meeting_id}.p{part}.wav"))
            .to_string_lossy()
            .into_owned();
        self.lock().pending_start_path = Some(audio_path.clone());
        let acked = self.wait_for(
            "recording.started",
            self.ack_ms(policies::START_ACK_MS),
            started_for(&audio_path),
        );
        // Offset from when capture was asked to begin, not from the ack (ack latency would shift timestamps).
        let at = self.now();
        let paused = self.lock().paused;
        if self
            .send_helper(HelperCommand::RecordingStart {
                audio_path: audio_path.clone(),
            })
            .is_err()
            || (paused && self.send_helper(HelperCommand::RecordingPause).is_err())
        {
            return; // went down again; the next hello retries
        }
        let acked = acked.await.ok().flatten();
        self.clear_pending_start(&audio_path);
        self.lock().busy_refusals.remove(&audio_path);
        if acked.is_none() || self.lock().machine.state().phase != RecordingPhase::Reconnecting {
            return;
        }
        self.clear_reconnect_timer();
        // A fresh helper starts unpaused: re-apply a privacy pause before anything is transcribed.
        if self.lock().paused {
            let _ = self.send_helper(HelperCommand::RecordingPause);
        }
        self.send(LifecycleEvent::HelperRestored { at, audio_path });
        tracing::info!(part, "recording resumed after helper restart");
    }

    // ------------------------------------------------------------------ stop

    async fn stop(self: Arc<Self>, reason: StopReason) -> Result<SessionView, CoreError> {
        let s = self.lock().machine.state().clone();
        // Idempotent: a second stop (double-click, tray + hotkey, meeting.ended racing a click) is a no-op.
        if s.meeting_id.is_some() && matches!(s.phase, RecordingPhase::Stopping | RecordingPhase::Processing)
        {
            return Ok(self.view());
        }
        let Some(meeting_id) = s.meeting_id.clone().filter(|_| is_capturing(s.phase)) else {
            return Err(CoreError::validation("not recording"));
        };
        let started_at = s.started_at.unwrap_or_else(|| self.now());
        self.clear_reconnect_timer();
        let was_live = s.phase != RecordingPhase::Reconnecting;
        self.send(LifecycleEvent::StopRequested {
            reason,
            at: self.now(),
        });
        let mut stopped: Option<Option<i64>> = None;
        if was_live {
            let part = s.audio_parts.last().cloned().filter(|p| !p.is_empty());
            let stop_part = part.clone();
            let matcher: Matcher = Box::new(move |m| match (m, &stop_part) {
                (HelperInput::RecordingStopped { audio_path, .. }, Some(part)) => {
                    audio_path.as_ref() == Some(part)
                }
                _ => true,
            });
            let p = self.wait_for(
                "recording.stopped",
                self.ack_ms(policies::STOP_ACK_MS),
                Some(matcher),
            );
            match self.send_helper(HelperCommand::RecordingStop { audio_path: None }) {
                Ok(()) => {
                    if let Some(HelperInput::RecordingStopped { duration_ms, .. }) = p.await.ok().flatten() {
                        stopped = Some(duration_ms);
                    } else {
                        // Still flushing after the timeout: it may still be writing the WAV finalize repairs. First
                        // abort just that recording (scoped by its file, hard deadline), so a dictation sharing the
                        // helper survives; restart only if the abort is not acked either (port of NoteService N2).
                        let aborted = match &part {
                            Some(part) => self.abort_recording(part).await,
                            None => false,
                        };
                        if aborted {
                            tracing::warn!("recording.stop not acked; aborted the recording (helper kept)");
                        } else {
                            tracing::warn!("recording.stop and recording.abort not acked; restarting helper");
                            self.d.helper.restart();
                        }
                    }
                }
                Err(_) => self.lock().helper_crashed = true, // helper down: finish from what's persisted
            }
        } else {
            // Giving up a reconnect: a helper that came back late may have opened a part; never leave it capturing.
            let _ = self.send_helper(HelperCommand::RecordingStop { audio_path: None });
        }
        let (st, segs, persisted) = {
            let mut g = self.lock();
            g.paused = false;
            (g.machine.state().clone(), g.asr.segments(), g.persisted.clone())
        };
        // Flush any final segment whose live append gave up.
        // Same retry policy as live appends; whatever still fails is logged loudly.
        for seg in segs {
            if !persisted.contains(&seg.id) {
                self.lock().persisted.insert(seg.id.clone());
                self.clone().append_live(meeting_id.clone(), seg).await;
            }
        }
        let ended_at = self.now();
        let (active, crashed) = {
            let g = self.lock();
            (g.active.clone(), g.helper_crashed)
        };
        let helper_dur = stopped.flatten().filter(|d| *d != 0);
        let mut pipeline = new_pipeline(
            &meeting_id,
            reason,
            PipelineContext {
                started_at,
                ended_at,
                duration_ms: helper_dur.unwrap_or(ended_at - started_at),
                audio_parts: st.audio_parts.clone(),
                active,
                helper_crashed: crashed || stopped.is_none(),
            },
            ended_at,
        );
        // N29/H8: silent stretches (closed at the stop) and capture fallbacks go to the meeting's pipeline.
        pipeline.capture = capture_report(&st);
        self.persist_pipeline(pipeline).await;
        self.send(LifecycleEvent::Stopped { at: ended_at });
        self.emit_transcript(&meeting_id); // final rev: partial cleared
        let (sh, id) = (self.clone(), meeting_id);
        self.spawn(async move { sh.run_pipeline(id).await });
        Ok(self.view())
    }

    fn dismiss(&self) -> SessionView {
        if !self.send(LifecycleEvent::Reset) {
            self.send(LifecycleEvent::ClearError);
        }
        self.view()
    }

    async fn toggle(self: Arc<Self>) -> Result<SessionView, CoreError> {
        if is_capturing(self.lock().machine.state().phase) {
            self.stop(StopReason::Manual).await
        } else {
            self.start(None).await
        }
    }

    // ------------------------------------------------------------------ post-processing pipeline

    async fn cached_or_stored(&self, id: &str) -> Result<Option<PipelineState>, CoreError> {
        let cached = self.lock().pipelines.get(id).cloned();
        match cached {
            Some(p) => Ok(Some(p)),
            None => Ok(self.d.store.get_pipeline(id.to_string()).await?),
        }
    }

    async fn retry_step(self: Arc<Self>, id: &str, step: StepId) -> Result<PipelineView, CoreError> {
        let Some(p) = self.cached_or_stored(id).await? else {
            return Err(CoreError::not_found(format!(
                "no post-processing record for {id}"
            )));
        };
        {
            let mut st = self.lock();
            if st.pipeline_running.contains(id) && p.steps.get(step).status == StepStatus::Running {
                return Ok(to_view(&p));
            }
            if let Some(t) = st.pipeline_timers.remove(id) {
                t.abort();
            }
            st.backoff_served.remove(id);
        }
        let p = pipeline_step(&p, step, StepPatch::ManualRetry { now: self.now() });
        // A manual run always executes, even when Settings turned the automatic step off.
        self.lock().manual_steps.insert(format!("{id}:{}", step.as_str()));
        self.persist_pipeline(p.clone()).await;
        let (sh, mid) = (self.clone(), id.to_string());
        self.spawn(async move { sh.run_pipeline(mid).await });
        Ok(to_view(&p))
    }

    /// Boxed: the backoff timer re-enters it, and a named `Send` future breaks the auto-trait cycle.
    fn run_pipeline(
        self: Arc<Self>,
        id: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
            if !self.lock().pipeline_running.insert(id.clone()) {
                return;
            }
            self.clone().run_pipeline_loop(&id).await;
            self.lock().pipeline_running.remove(&id);
        })
    }

    async fn run_pipeline_loop(self: Arc<Self>, id: &str) {
        loop {
            let Some(p) = self.lock().pipelines.get(id).cloned() else {
                return;
            };
            let Some(step) = next_runnable_step(&p) else { break };
            let cur = p.steps.get(step);
            let now = self.now();
            // Wait out the backoff (in memory; a restart resumes immediately). `next_retry_at` is wall clock, so
            // the wait is capped at the longest delay this attempt could have drawn, and once that timer fired the
            // step runs: a clock that jumped back must not stall it for the size of the jump (N12).
            let backoff = format!("{id}:{}:{}", step.as_str(), cur.attempts);
            let served = self.lock().backoff_served.remove(id).as_deref() == Some(backoff.as_str());
            if !served && cur.status == StepStatus::Retrying {
                if let Some(at) = cur.next_retry_at.filter(|at| *at != 0 && *at > now) {
                    let cap = max_retry_delay(&policies::for_step(step), cur.attempts, self.scale()) as i64;
                    let mid = id.to_string();
                    let t = self.set_timeout((at - now).min(cap) as f64, move |sh| {
                        sh.lock().pipeline_timers.remove(&mid);
                        sh.lock().backoff_served.insert(mid.clone(), backoff);
                        let s2 = sh.clone();
                        sh.spawn(async move { s2.run_pipeline(mid).await });
                    });
                    if let Some(t) = t {
                        self.lock().pipeline_timers.insert(id.to_string(), t);
                    }
                    return;
                }
            }
            let p = pipeline_step(&p, step, StepPatch::Start { now: self.now() });
            let started = self.persist_pipeline(p.clone()).await;
            if started == Persisted::Gone {
                return self.forget_pipeline(id);
            }
            // Durability barrier: never run a step whose start is not on disk (a crash would re-run it blind).
            let r = if started == Persisted::Saved {
                self.clone().exec_step(&p, step).await
            } else {
                Err(CoreError::unavailable("could not persist step start"))
            };
            // N27: the meeting was deleted while the step ran (`forget_pipeline`): no retry, no fallback, no
            // event, no save for it. Falling back to the stale copy here re-inserted a deleted meeting's pipeline.
            let Some(latest) = self.lock().pipelines.get(id).cloned() else {
                return;
            };
            // A manual retry re-armed this step while it was in flight: its result is stale, drop it.
            if latest.steps.get(step).status != StepStatus::Running {
                continue;
            }
            let mut p = match r {
                Ok(skipped) => pipeline_step(
                    &latest,
                    step,
                    if skipped {
                        StepPatch::Skip { now: self.now() }
                    } else {
                        StepPatch::Success {
                            now: self.now(),
                            fallback: false,
                        }
                    },
                ),
                Err(e) => {
                    let err = to_lifecycle_error(&e);
                    let policy = policies::for_step(step);
                    let attempts = latest.steps.get(step).attempts;
                    let delay_ms = retry_delay(&policy, attempts, (self.d.random)(), self.scale());
                    let message = err.message.clone();
                    let mut p = pipeline_step(
                        &latest,
                        step,
                        StepPatch::Failure {
                            now: self.now(),
                            error: err,
                            delay_ms,
                            terminal: false,
                            fallback: false,
                        },
                    );
                    tracing::warn!(id, step = step.as_str(), attempt = attempts, err = %message, "pipeline step failed");
                    if step == StepId::Summary && p.steps.summary.status == StepStatus::Failed {
                        // Out of attempts: the meeting still gets the on-device summary; the step stays failed.
                        match self
                            .d
                            .steps
                            .summary(id.to_string(), SummaryRequest::Fallback { error: message })
                            .await
                        {
                            Ok(_) => p.steps.summary.fallback = Some(true),
                            Err(fe) => tracing::error!(err = %fe.message, "fallback summary failed"),
                        }
                    }
                    if step == StepId::Finalize && p.steps.finalize.status == StepStatus::Failed {
                        let _ = self.d.store.mark_failed(id.to_string()).await;
                    }
                    p
                }
            };
            p.meeting_id = id.to_string();
            if self.persist_pipeline(p.clone()).await == Persisted::Gone {
                return self.forget_pipeline(id);
            }
            self.emit(CoreEvent::Changed { id: id.to_string() });
            self.sync_session_with_pipeline(&p);
        }
    }

    /// Summary and index run one at a time across all meetings (FIFO), so a recovered backlog or synchronized
    /// retry timers never burst the LLM provider. Finalize is local and runs immediately.
    async fn exec_step(self: Arc<Self>, p: &PipelineState, step: StepId) -> Result<bool, CoreError> {
        if step == StepId::Finalize {
            return self.exec_step_inner(p, step).await;
        }
        let _turn = self.heavy.lock().await;
        self.clone().exec_step_inner(p, step).await
    }

    /// `Ok(true)` when the step was skipped (nothing to do on this machine).
    async fn exec_step_inner(self: Arc<Self>, p: &PipelineState, step: StepId) -> Result<bool, CoreError> {
        let key = format!("{}:{}", p.meeting_id, step.as_str());
        if self.d.fault_step == Some(step) && self.lock().faults_fired.insert(key.clone()) {
            return Err(CoreError::unavailable(format!(
                "injected {} failure",
                step.as_str()
            )));
        }
        let manual = self.lock().manual_steps.remove(&key);
        let cfg = self.config();
        let (steps, id) = (self.d.steps.clone(), p.meeting_id.clone());
        match step {
            StepId::Finalize => {
                with_timeout(
                    "note.stepFinalize",
                    FINALIZE_TIMEOUT_MS,
                    steps.finalize(id, p.ctx.clone()),
                )
                .await?;
                Ok(false)
            }
            StepId::Summary => {
                if !cfg.auto_summarize && !manual {
                    // Settings → Summarize after meetings is off: finish without a summary.
                    with_timeout(
                        "note.stepSummary",
                        10_000,
                        steps.summary(id, SummaryRequest::Skip),
                    )
                    .await?;
                    return Ok(true);
                }
                with_timeout(
                    "note.stepSummary",
                    SUMMARY_TIMEOUT_MS,
                    steps.summary(id, SummaryRequest::Llm),
                )
                .await?;
                Ok(false)
            }
            StepId::Index => {
                if !cfg.background_indexing && !manual {
                    return Ok(true);
                }
                let r = with_timeout("note.stepIndex", INDEX_TIMEOUT_MS, steps.index(id)).await?;
                Ok(r.skipped)
            }
        }
    }

    /// The session's own meeting: processing → done once usable, → error if finalize failed for good.
    fn sync_session_with_pipeline(&self, p: &PipelineState) {
        let s = self.lock().machine.state().clone();
        // `error` too: a manual finalize retry that succeeds brings the session back to done.
        if s.meeting_id.as_deref() != Some(p.meeting_id.as_str())
            || !matches!(s.phase, RecordingPhase::Processing | RecordingPhase::Error)
        {
            return;
        }
        if pipeline_usable(p) {
            self.send(LifecycleEvent::ProcessingDone);
        } else if s.phase == RecordingPhase::Processing && p.steps.finalize.status == StepStatus::Failed {
            let error = p.steps.finalize.error.clone().unwrap_or(LifecycleError {
                code: "E_INTERNAL".into(),
                message: "post-processing failed".into(),
                retryable: true,
            });
            self.send(LifecycleEvent::ProcessingFailed { error });
        }
    }

    // ------------------------------------------------------------------ restart recovery

    /// R1: a session persisted mid-capture → crash-recovered pipeline; R3: orphans; R2: resume unsettled
    /// pipelines one at a time. Idempotent.
    async fn recover(self: Arc<Self>) -> RecoverReport {
        let mut report = RecoverReport::default();
        if let Err(e) = self.clone().recover_inner(&mut report).await {
            tracing::warn!(err = %e.message, "recovery failed");
        }
        self.ready.send_replace(true);
        let days = self.config().retain_audio_days;
        if days > 0 {
            match self.d.store.prune_audio(days, self.now()).await {
                Ok(ids) => {
                    for id in &ids {
                        self.emit(CoreEvent::Changed { id: id.clone() });
                    }
                    report.pruned = ids;
                }
                Err(e) => tracing::warn!(err = %e.message, "audio prune failed"),
            }
        }
        report
    }

    fn pending_delete(&self, id: &str) -> bool {
        self.d.pending_delete.as_ref().is_some_and(|f| f(id))
    }

    async fn recover_inner(self: Arc<Self>, report: &mut RecoverReport) -> Result<(), LifecycleError> {
        let saved = self.d.store.load_session().await?;
        let now = self.now();
        if let Some(saved) = &saved {
            if let Some(mid) = saved
                .meeting_id
                .clone()
                .filter(|_| is_capturing(saved.phase) || saved.phase == RecordingPhase::Stopping)
                .filter(|id| !self.pending_delete(id))
            {
                if self.d.store.get_pipeline(mid.clone()).await?.is_none() {
                    tracing::warn!(id = %mid, "app died mid-recording; recovering");
                    let ctx = PipelineContext {
                        started_at: saved.started_at.unwrap_or(now),
                        ended_at: saved.stopped_at.unwrap_or(now),
                        duration_ms: 0, // finalize derives it from the repaired audio
                        audio_parts: saved.audio_parts.clone(),
                        active: Vec::new(),
                        helper_crashed: true,
                    };
                    // N29/H8: the session row carries the capture record; a stretch open at the crash keeps no end.
                    let mut p = new_pipeline(&mid, StopReason::CrashRecovered, ctx, now);
                    p.capture = capture_report(saved);
                    self.persist_pipeline(p).await;
                    report.crash_recovered = Some(mid);
                }
            }
        }
        for m in self.d.store.orphans().await? {
            if self.pending_delete(&m.id) {
                continue;
            }
            let ctx = PipelineContext {
                started_at: m.started_at,
                ended_at: m.ended_at.unwrap_or(now),
                duration_ms: m.duration_ms.unwrap_or(0),
                audio_parts: m.audio_path.iter().filter(|p| !p.is_empty()).cloned().collect(),
                active: Vec::new(),
                helper_crashed: true,
            };
            self.persist_pipeline(new_pipeline(&m.id, StopReason::CrashRecovered, ctx, now))
                .await;
            report.orphans.push(m.id);
        }
        // The session itself always restarts idle (nothing is capturing after a restart).
        if saved.as_ref().is_some_and(|s| s.phase != RecordingPhase::Idle) {
            let s = self.lock().machine.state().clone();
            self.persist_session(s);
        }
        self.emit_session();
        let mut pending = self.d.store.list_unsettled().await?;
        pending.retain(|p| !self.pending_delete(&p.meeting_id));
        for p in &pending {
            let p = resume_after_restart(p.clone());
            self.lock().pipelines.insert(p.meeting_id.clone(), p.clone());
            self.emit(CoreEvent::Pipeline(to_view(&p)));
        }
        report.resumed = pending.iter().map(|p| p.meeting_id.clone()).collect();
        // One at a time: a backlog after a long outage must not fire N LLM calls at once.
        let (sh, ids) = (self.clone(), report.resumed.clone());
        self.spawn(async move {
            for id in ids {
                sh.clone().run_pipeline(id).await;
            }
        });
        Ok(())
    }

    // ------------------------------------------------------------------ summaries

    async fn resummarize(self: Arc<Self>, id: &str) -> Result<Option<MeetingSummary>, CoreError> {
        let p = self.cached_or_stored(id).await?;
        if p.is_some_and(|p| p.steps.finalize.status == StepStatus::Done) {
            // Through the pipeline so retries/backoff/progress are tracked like any other step.
            self.clone().retry_step(id, StepId::Summary).await?;
            self.wait_settled(id, StepId::Summary).await;
            return Ok(self.d.store.get_summary(id.to_string()).await?);
        }
        let summary = with_timeout(
            "note.resummarize",
            SUMMARY_TIMEOUT_MS,
            self.d.steps.resummarize(id.to_string(), None),
        )
        .await?;
        self.emit(CoreEvent::Changed { id: id.to_string() });
        Ok(Some(summary))
    }

    async fn wait_settled(&self, id: &str, step: StepId) {
        for _ in 0..2400 {
            let st = self.lock().pipelines.get(id).map(|p| p.steps.get(step).status);
            if matches!(
                st,
                Some(StepStatus::Done | StepStatus::Failed | StepStatus::Skipped)
            ) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// True when `file` resolves lexically to a path strictly inside `dir` (no `..` escape). TS `insideDir`.
pub fn inside_dir(dir: &std::path::Path, file: &std::path::Path) -> bool {
    use std::path::Component;
    let norm = |p: &std::path::Path| {
        let mut out: Vec<std::ffi::OsString> = Vec::new();
        for c in p.components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                c => out.push(c.as_os_str().to_owned()),
            }
        }
        out
    };
    if !file.is_absolute() || !dir.is_absolute() {
        return false;
    }
    let (d, f) = (norm(dir), norm(file));
    f.len() > d.len() && f.starts_with(&d)
}

/// Files to remove for a deleted meeting: those the DB listed plus any `<id>.wav` / `<id>.p<n>.wav` in the
/// recordings directory. TS `meetingAudio`.
pub fn meeting_audio(dir: &std::path::Path, id: &str, listed: &[String]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for f in listed {
        add(PathBuf::from(f));
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // `<id>.wav`, `<id>.p<n>.wav`, and a `<id>.wav.stitch` an interrupted finalize left (N28).
            let Some(stem) = name.strip_suffix(".stitch").unwrap_or(&name).strip_suffix(".wav") else {
                continue;
            };
            let base = match stem.rsplit_once(".p") {
                Some((b, n)) if !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) => b,
                _ => stem,
            };
            if base == id {
                add(dir.join(&name));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_title_is_civil_utc() {
        assert_eq!(utc_recording_title(0), "Recording 1970-01-01 00:00");
        assert_eq!(
            utc_recording_title(1_760_000_000_000),
            "Recording 2025-10-09 08:53"
        );
        assert_eq!(utc_recording_title(951_782_400_000), "Recording 2000-02-29 00:00");
    }

    #[test]
    fn revocation_matches_the_arbiter() {
        use Degradation as D;
        use OsPermission as P;
        assert_eq!(
            degradations_for_revocation(P::Accessibility, &[]),
            vec![D::AxUnavailable]
        );
        assert_eq!(degradations_for_revocation(P::Microphone, &[]), vec![D::NoMic]);
        assert_eq!(
            degradations_for_revocation(P::Microphone, &[P::ScreenRecording]),
            vec![D::AudioUnavailable]
        );
        assert_eq!(
            degradations_for_revocation(P::ScreenRecording, &[]),
            vec![D::MicOnly]
        );
        assert_eq!(
            degradations_for_revocation(P::ScreenRecording, &[P::Microphone]),
            vec![D::AudioUnavailable]
        );
    }
}
