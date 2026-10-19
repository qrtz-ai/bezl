//! Runtime ports: what the shell injects and what it gets back (docs/rust-core-api.md §4.2).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use qrtz_model::{
    de_ms, de_ms_opt, AsrEvent, CaptureSource, Degradation, EpochMs, LifecycleError, LiveTranscriptDelta,
    MeetingPlatform, MeetingSummary, Ms, OsPermission, PermissionState, PermissionsStatus, PipelineContext,
    PipelineView, SessionView,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// `deps.config().note` + `privacy.excludedApps`, read on every use (settings can change at any time).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuntimeConfig {
    pub auto_detect_meetings: bool,
    pub auto_record: bool,
    pub detect_prompt: bool,
    pub auto_summarize: bool,
    pub background_indexing: bool,
    pub default_template: String,
    pub excluded_apps: Vec<String>,
    pub retain_audio_days: u32,
    /// The summary step's RPC timeout (TS `WorkerClient.call`, 120 s). Not a setting: the shell raises it when the
    /// summary engine is on-device (ADR 0075: a long meeting is several model calls).
    #[serde(skip)]
    pub summary_timeout_ms: u64,
}

impl Default for RuntimeConfig {
    /// The zod defaults (packages/core/src/config.ts).
    fn default() -> Self {
        RuntimeConfig {
            auto_detect_meetings: true,
            auto_record: false,
            detect_prompt: true,
            auto_summarize: true,
            background_indexing: true,
            default_template: "general".into(),
            excluded_apps: Vec::new(),
            retain_audio_days: 0,
            summary_timeout_ms: super::service::SUMMARY_TIMEOUT_MS,
        }
    }
}

impl RuntimeConfig {
    /// `isExcludedApp` (privacy.ts).
    pub fn is_excluded(&self, app: &str, platform: &str) -> bool {
        let (app, platform) = (app.to_lowercase(), platform.to_lowercase());
        self.excluded_apps.iter().any(|raw| {
            let x = crate::js::js_trim(raw).to_lowercase();
            !x.is_empty() && (app == x || app.contains(&x) || platform == x)
        })
    }
}

/// The error every runtime call can fail with: the IPC `AppErrorJSON` minus `name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct CoreError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl CoreError {
    pub fn new(code: &str, message: impl Into<String>, retryable: bool) -> Self {
        CoreError {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }
    pub fn validation(m: impl Into<String>) -> Self {
        Self::new("E_VALIDATION", m, false)
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new("E_FORBIDDEN", m, false)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new("E_NOT_FOUND", m, false)
    }
    pub fn timeout(m: impl Into<String>) -> Self {
        Self::new("E_TIMEOUT", m, true)
    }
    pub fn unavailable(m: impl Into<String>) -> Self {
        Self::new("E_UNAVAILABLE", m, true)
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new("E_INTERNAL", m, false)
    }
    /// `AppErrorJSON` for the IPC layer.
    pub fn to_app_error_json(&self) -> Value {
        json!({ "name": "AppError", "code": self.code, "message": self.message, "retryable": self.retryable })
    }
}

impl From<LifecycleError> for CoreError {
    fn from(e: LifecycleError) -> Self {
        CoreError {
            code: e.code,
            message: e.message,
            retryable: e.retryable,
        }
    }
}

impl From<CoreError> for LifecycleError {
    fn from(e: CoreError) -> Self {
        LifecycleError {
            code: e.code,
            message: e.message,
            retryable: e.retryable,
        }
    }
}

/// Host → helper commands the runtime sends. `to_json` is the protocol line (minus the newline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperCommand {
    PermissionsCheck,
    PermissionsRequest {
        permission: OsPermission,
    },
    RecordingStart {
        audio_path: String,
    },
    /// `audio_path`: only stop the recording writing this file (the helper ignores a stop for any other path).
    RecordingStop {
        audio_path: Option<String>,
    },
    /// N2: tear down the recording writing `audio_path` without a flush (scoped; dictation untouched). The helper
    /// answers `recording.aborted{audioPath}`.
    RecordingAbort {
        audio_path: String,
    },

    RecordingPause,
    RecordingResume,
    /// ADR 0079 / N121: the speech models were just installed: the helper loads each once (one at a time, then
    /// unloads) so the first recording doesn't pay the OS model compile. Ignored by the helper while recording.
    ModelsWarm,
}

impl HelperCommand {
    pub fn type_name(&self) -> &'static str {
        match self {
            HelperCommand::PermissionsCheck => "permissions.check",
            HelperCommand::PermissionsRequest { .. } => "permissions.request",
            HelperCommand::RecordingStart { .. } => "recording.start",
            HelperCommand::RecordingStop { .. } => "recording.stop",
            HelperCommand::RecordingAbort { .. } => "recording.abort",
            HelperCommand::RecordingPause => "recording.pause",
            HelperCommand::RecordingResume => "recording.resume",
            HelperCommand::ModelsWarm => "models.warm",
        }
    }
    pub fn to_json(&self) -> Value {
        match self {
            HelperCommand::PermissionsRequest { permission } => {
                json!({ "type": self.type_name(), "permission": permission })
            }
            HelperCommand::RecordingStart { audio_path }
            | HelperCommand::RecordingAbort { audio_path }
            | HelperCommand::RecordingStop {
                audio_path: Some(audio_path),
            } => {
                json!({ "type": self.type_name(), "audioPath": audio_path })
            }
            _ => json!({ "type": self.type_name() }),
        }
    }
}

/// Helper → host messages the runtime consumes. Everything else (talk, captions, log, heartbeat) is ignored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum HelperInput {
    #[serde(rename = "hello")]
    Hello {},
    #[serde(rename = "permissions.status")]
    PermissionsStatus {
        status: BTreeMap<OsPermission, PermissionState>,
    },
    /// H5: a TCC permission was withdrawn while capturing.
    #[serde(rename = "permission.revoked", rename_all = "camelCase")]
    PermissionRevoked {
        permission: OsPermission,
        #[serde(default, deserialize_with = "de_ms")]
        at_ms: Ms,
        /// N4: the helper arbiter's degradations for this revocation (`[]` when not recording). `None` = an older
        /// helper: the host derives them from its permission snapshot.
        #[serde(default)]
        degraded: Option<Vec<Degradation>>,
    },
    #[serde(rename = "ax.participants")]
    Participants { names: Vec<String> },
    #[serde(rename = "meeting.detected")]
    MeetingDetected {
        key: String,
        title: String,
        platform: MeetingPlatform,
        app: String,
    },
    #[serde(rename = "meeting.ended")]
    MeetingEnded { key: String },
    #[serde(rename = "ax.activeSpeaker", rename_all = "camelCase")]
    ActiveSpeaker {
        #[serde(deserialize_with = "de_ms")]
        at_ms: Ms,
        name: String,
    },
    #[serde(rename = "audio.level")]
    AudioLevel { rms: f64 },
    /// N29: `source` has been under the silence floor since `since_ms` (helper, part-relative ms).
    #[serde(rename = "capture.silent", rename_all = "camelCase")]
    CaptureSilent {
        source: CaptureSource,
        #[serde(deserialize_with = "de_ms")]
        since_ms: Ms,
    },
    /// N29: `source` is above the floor again.
    #[serde(rename = "capture.audible")]
    CaptureAudible { source: CaptureSource },
    /// ADR 0079: a speaker turn on one raw channel from the helper's own diarizer (the Mac path without the speech
    /// module), part-relative ms. Attributed exactly like a module `speaker` event (`mic:N` / `system:N`).
    #[serde(rename = "diarization.segment", rename_all = "camelCase")]
    DiarizationSegment {
        source: CaptureSource,
        speaker: u8,
        #[serde(deserialize_with = "de_ms")]
        start_ms: Ms,
        #[serde(deserialize_with = "de_ms")]
        end_ms: Ms,
    },
    /// N126 (ADR 0085): the helper's diarizer for `source` stood down (`backlog`) or never loaded
    /// (`model-unavailable`) at `at_ms` (part-relative); no more turns come from it this part.
    #[serde(rename = "diarization.unavailable", rename_all = "camelCase")]
    DiarizationUnavailable {
        source: CaptureSource,
        #[serde(deserialize_with = "de_ms")]
        at_ms: Ms,
        reason: qrtz_model::DiarizationUnavailableReason,
    },
    /// N35: a capture device failed inside a running source; applied like a revoke, sticky until a reconnect.
    #[serde(rename = "capture.degraded")]
    CaptureDegraded {
        degraded: Vec<Degradation>,
        reason: String,
    },
    /// H8: a capture source failed to start; `to` is the one tried next, `None` (null) when none is left.
    #[serde(rename = "capture.fallback")]
    CaptureFallback {
        from: String,
        #[serde(default)]
        to: Option<String>,
        reason: String,
    },
    #[serde(rename = "asr")]
    Asr { event: AsrEvent },
    /// ADR 0046: an event from a recording part's speech session (fed by the host through
    /// `NoteRuntime::speech_event`, never a helper line). `audio_path` names the part (`None` = the current one).
    #[serde(skip)]
    Speech {
        audio_path: Option<String>,
        /// The channels the part's session was started with (`SessionSpec::channels`).
        channels: Vec<qrtz_speech_types::Channel>,
        event: qrtz_speech_types::SpeechEvent,
    },
    #[serde(rename = "recording.started", rename_all = "camelCase")]
    RecordingStarted {
        #[serde(default)]
        audio_path: String,
    },
    #[serde(rename = "recording.stopped", rename_all = "camelCase")]
    RecordingStopped {
        #[serde(default)]
        audio_path: Option<String>,
        #[serde(default, deserialize_with = "de_ms_opt")]
        duration_ms: Option<Ms>,
    },
    /// Answer to `recording.abort{audioPath}`: no writer on that file remains.
    #[serde(rename = "recording.aborted", rename_all = "camelCase")]
    RecordingAborted { audio_path: String },
    /// `audio_path` / `active_audio_path` are set on `recording.busy`: the ignored start's file and the file the
    /// running recording writes.
    #[serde(rename = "error", rename_all = "camelCase")]
    Error {
        code: String,
        message: String,
        #[serde(default)]
        audio_path: Option<String>,
        #[serde(default)]
        active_audio_path: Option<String>,
    },
}

impl HelperInput {
    /// Narrows a validated helper line; `None` for messages the runtime doesn't consume.
    pub fn from_json(v: &Value) -> Option<HelperInput> {
        serde_json::from_value(v.clone()).ok()
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            HelperInput::Hello {} => "hello",
            HelperInput::PermissionsStatus { .. } => "permissions.status",
            HelperInput::PermissionRevoked { .. } => "permission.revoked",
            HelperInput::Participants { .. } => "ax.participants",
            HelperInput::MeetingDetected { .. } => "meeting.detected",
            HelperInput::MeetingEnded { .. } => "meeting.ended",
            HelperInput::ActiveSpeaker { .. } => "ax.activeSpeaker",
            HelperInput::AudioLevel { .. } => "audio.level",
            HelperInput::CaptureSilent { .. } => "capture.silent",
            HelperInput::CaptureAudible { .. } => "capture.audible",
            HelperInput::DiarizationSegment { .. } => "diarization.segment",
            HelperInput::DiarizationUnavailable { .. } => "diarization.unavailable",
            HelperInput::CaptureFallback { .. } => "capture.fallback",
            HelperInput::CaptureDegraded { .. } => "capture.degraded",
            HelperInput::Asr { .. } => "asr",
            HelperInput::Speech { .. } => "speech",
            HelperInput::RecordingStarted { .. } => "recording.started",
            HelperInput::RecordingStopped { .. } => "recording.stopped",
            HelperInput::RecordingAborted { .. } => "recording.aborted",
            HelperInput::Error { .. } => "error",
        }
    }
}

/// Events the shell fans out (`qrtz:event`).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // moved once into the sink; boxing would only add an allocation
pub enum CoreEvent {
    /// `note:session`: every transition synchronously; level/ASR/participants throttled.
    Session(SessionView),
    /// `note:pipeline`.
    Pipeline(PipelineView),
    /// `note:transcript`.
    Transcript(LiveTranscriptDelta),
    /// `note:changed`.
    Changed { id: String },
    /// `permissions:changed`.
    PermissionsChanged(PermissionsStatus),
    /// A meeting was detected and auto-record is off: the shell shows an OS notification. Not an IPC event.
    DetectedPrompt {
        title: String,
        platform: MeetingPlatform,
    },
    /// `note:auto-stop`: the recorded meeting ended and the recording stops at `deadline_at` unless it comes
    /// back or the user keeps recording (`Some`), or that countdown was cleared (`None`).
    AutoStop { deadline_at: Option<EpochMs> },
}

impl CoreEvent {
    pub fn ipc_name(&self) -> Option<&'static str> {
        match self {
            CoreEvent::Session(_) => Some("note:session"),
            CoreEvent::Pipeline(_) => Some("note:pipeline"),
            CoreEvent::Transcript(_) => Some("note:transcript"),
            CoreEvent::Changed { .. } => Some("note:changed"),
            CoreEvent::PermissionsChanged(_) => Some("permissions:changed"),
            CoreEvent::DetectedPrompt { .. } => None,
            CoreEvent::AutoStop { .. } => Some("note:auto-stop"),
        }
    }
    pub fn payload(&self) -> Value {
        match self {
            CoreEvent::Session(v) => serde_json::to_value(v).unwrap(),
            CoreEvent::Pipeline(v) => serde_json::to_value(v).unwrap(),
            CoreEvent::Transcript(v) => serde_json::to_value(v).unwrap(),
            CoreEvent::Changed { id } => json!({ "id": id }),
            CoreEvent::PermissionsChanged(p) => serde_json::to_value(p).unwrap(),
            CoreEvent::DetectedPrompt { title, platform } => json!({ "title": title, "platform": platform }),
            CoreEvent::AutoStop { deadline_at: Some(d) } => json!({ "state": "armed", "deadlineAt": d }),
            CoreEvent::AutoStop { deadline_at: None } => json!({ "state": "cleared" }),
        }
    }
}

pub trait EventSink: Send + Sync {
    fn emit(&self, e: CoreEvent);
}

/// Outbound to the helper. `false` = not running (TS `Supervisor.send` returning false).
pub trait HelperPort: Send + Sync {
    fn send(&self, cmd: &HelperCommand) -> bool;
    /// `helper.stop(); helper.start()`: a stop ack timed out, so the helper may still be writing the WAV that
    /// finalize is about to repair; the supervisor brings a fresh one back.
    fn restart(&self) {}
}

/// Wall and monotonic clocks in ms. Timers are `tokio::time`, so tests pair a paused runtime with [`TokioClock`].
///
/// N45 (ADR 0037): `mono_ms` advances with real/async time and ignores wall jumps (`clockJump`, sleep, NTP).
/// Part offsets for `helper-lost` / `helper-restored` are derived from it so a laptop sleep during a reconnect
/// cannot slide the next part's transcript by an hour while the WAV pad stays capped at 35 s.
pub trait Clock: Send + Sync {
    /// Wall clock in epoch ms (subject to NTP / sleep jumps).
    fn now_ms(&self) -> EpochMs;
    /// Monotonic ms since an arbitrary origin. Default = [`Self::now_ms`] for clocks without wall skew.
    fn mono_ms(&self) -> EpochMs {
        self.now_ms()
    }
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> EpochMs {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
    fn mono_ms(&self) -> EpochMs {
        use std::sync::OnceLock;
        static START: OnceLock<std::time::Instant> = OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_millis() as i64
    }
}

/// `base + elapsed tokio time`: follows `tokio::time::pause`/`advance` exactly (tests).
pub struct TokioClock {
    base: EpochMs,
    start: tokio::time::Instant,
    /// Wall-clock steps (NTP, VM resume, manual change); timers are unaffected, like a real wall clock.
    skew: std::sync::atomic::AtomicI64,
}
impl TokioClock {
    pub fn new(base: EpochMs) -> Self {
        TokioClock {
            base,
            start: tokio::time::Instant::now(),
            skew: std::sync::atomic::AtomicI64::new(0),
        }
    }
    /// Steps the wall clock by `ms` (negative = backwards) without moving tokio time (tests, scenario `clockJump`).
    pub fn jump(&self, ms: i64) {
        self.skew.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Clock for TokioClock {
    fn now_ms(&self) -> EpochMs {
        self.mono_ms() + self.skew.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn mono_ms(&self) -> EpochMs {
        self.base + (tokio::time::Instant::now() - self.start).as_millis() as i64
    }
}

pub type IdGen = Arc<dyn Fn() -> String + Send + Sync>;
pub type StoreResult<T> = Result<T, LifecycleError>;

/// `note.addNote` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteWithMeeting {
    #[serde(flatten)]
    pub note: qrtz_model::MeetingNote,
    pub meeting_id: String,
}

/// `note:add-note` request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddNoteRequest {
    #[serde(default)]
    pub meeting_id: Option<String>,
    pub text: String,
    #[serde(default, deserialize_with = "de_ms_opt")]
    pub at_ms: Option<Ms>,
}

/// ADR 0217: nano's Apple Calendar, as the runtime sees it when a recording starts. The shell keeps the cache
/// (qrtz-storage `calendar_events`) and a snapshot for this sync call.
pub trait CalendarLookup: Send + Sync {
    /// The event a recording starting at `now` belongs to (`platform`: the detected meeting's, if any), as
    /// `(event id, title)`; `None` when the calendar is off or nothing overlaps.
    fn event_at(&self, now: EpochMs, platform: Option<&str>) -> Option<(String, String)>;
    /// The recording `meeting_id` was titled from `event_id`: link them (`meetings.calendar_event_id`).
    fn link(&self, meeting_id: &str, event_id: &str);
}

/// Persistence the runtime needs; one method per worker call it made in TS (names in [`STORE_METHODS`]).
#[async_trait]
pub trait LifecycleStore: Send + Sync {
    /// `note.create`: upsert + template.
    async fn create_meeting(&self, m: qrtz_model::Meeting) -> StoreResult<()>;
    /// `note.delete`: drops the rows and returns the meeting's files for the host to remove (N15).
    async fn delete_meeting(&self, id: String) -> StoreResult<Vec<String>>;
    /// `note.appendLive`: idempotent upsert of one live final.
    async fn append_live(&self, meeting_id: String, s: qrtz_model::Segment) -> StoreResult<()>;
    /// `note.fail`.
    async fn mark_failed(&self, meeting_id: String) -> StoreResult<()>;
    /// `note.addNote` (the id is drawn only once the meeting is known to exist, like TS).
    async fn add_note(
        &self,
        meeting_id: String,
        text: String,
        at_ms: Ms,
        created_at: EpochMs,
        ids: IdGen,
    ) -> StoreResult<NoteWithMeeting>;
    /// `note.setTemplate` alone (N115: a template change then re-summarizes through the pipeline).
    async fn set_template(&self, meeting_id: String, template: String) -> StoreResult<()>;
    /// `note.get(...).summary`.
    async fn get_summary(&self, meeting_id: String) -> StoreResult<Option<MeetingSummary>>;
    /// `lifecycle.saveSession`.
    async fn save_session(&self, s: qrtz_model::RecordingState, now: EpochMs) -> StoreResult<()>;
    /// `lifecycle.loadSession`.
    async fn load_session(&self) -> StoreResult<Option<qrtz_model::RecordingState>>;
    /// `pipeline.save` (`false` = meeting deleted meanwhile).
    async fn save_pipeline(&self, p: qrtz_model::PipelineState, settled: bool) -> StoreResult<bool>;
    /// `pipeline.get`.
    async fn get_pipeline(&self, meeting_id: String) -> StoreResult<Option<qrtz_model::PipelineState>>;
    /// `pipeline.listUnsettled`.
    async fn list_unsettled(&self) -> StoreResult<Vec<qrtz_model::PipelineState>>;
    /// `pipeline.orphans`.
    async fn orphans(&self) -> StoreResult<Vec<qrtz_model::Meeting>>;
    /// `note.pruneAudio`: deletes audio of done meetings older than `days`; returns their ids.
    async fn prune_audio(&self, days: u32, now: EpochMs) -> StoreResult<Vec<String>>;
}

/// The TS worker method behind each store/step call (fault injection keys in the tier-2 scenarios).
pub const STORE_METHODS: &[&str] = &[
    "note.create",
    "note.delete",
    "note.appendLive",
    "note.fail",
    "note.addNote",
    "note.get",
    "note.setTemplate",
    "lifecycle.saveSession",
    "lifecycle.loadSession",
    "pipeline.save",
    "pipeline.get",
    "pipeline.listUnsettled",
    "pipeline.orphans",
    "note.pruneAudio",
    "note.stepFinalize",
    "note.stepSummary",
    "note.stepIndex",
    "note.resummarize",
];

/// `note.stepFinalize` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeOutcome {
    pub duration_ms: Ms,
    pub segments: u32,
}

/// `note.stepIndex` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexOutcome {
    pub indexed: u32,
    pub skipped: bool,
}

/// How `note.stepSummary` is asked to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryRequest {
    /// Strict LLM summary: provider failures are errors (`retryable` says whether a retry can help).
    Llm,
    /// Attempts exhausted: save the on-device summary with this LLM error.
    Fallback { error: String },
    /// Auto-summarize is off: mark the meeting done without a summary.
    Skip,
}

/// The post-processing steps. Each must be idempotent (recording-lifecycle.md, pipeline table).
#[async_trait]
pub trait StepExecutor: Send + Sync {
    async fn finalize(
        &self,
        meeting_id: String,
        ctx: PipelineContext,
    ) -> Result<FinalizeOutcome, LifecycleError>;
    async fn summary(
        &self,
        meeting_id: String,
        req: SummaryRequest,
    ) -> Result<Option<MeetingSummary>, LifecycleError>;
    async fn index(&self, meeting_id: String) -> Result<IndexOutcome, LifecycleError>;
    /// `note.resummarize` (non-strict; also how `note:set-template` re-runs the summary).
    async fn resummarize(
        &self,
        meeting_id: String,
        template: Option<String>,
    ) -> Result<MeetingSummary, LifecycleError>;
}

/// What `recover()` did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverReport {
    /// R1: the meeting whose capture the app died in.
    pub crash_recovered: Option<String>,
    /// R3: legacy meetings stuck in recording/processing.
    pub orphans: Vec<String>,
    /// R2: unsettled pipelines resumed.
    pub resumed: Vec<String>,
    pub pruned: Vec<String>,
}
