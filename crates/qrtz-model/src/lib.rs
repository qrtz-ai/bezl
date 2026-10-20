//! qrtz wire types. Every type here serializes to exactly the JSON the TS zod schemas accept and produce
//! (packages/core/src/note/{types,lifecycle,contract,summary,notes}.ts), so either runtime can read what the
//! other wrote (`lifecycle_state`, `pipeline_state`, `summaries.json`, IPC payloads).
//!
//! Rules (docs/rust-core-api.md §2):
//! - structs are camelCase, string enums kebab-case;
//! - zod `.optional()` = `Option` that is omitted when `None`; zod `.default(x)` = `#[serde(default)]`;
//! - unknown fields are ignored (zod strips them);
//! - ms values are integers. TS may have written a fractional number, so ms fields are read through
//!   [`de_ms`] / [`de_ms_opt`], which round. Floats are never written for ms.

#![forbid(unsafe_code)]
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// Epoch milliseconds (`*At` fields; `Date.now()`).
pub type EpochMs = i64;
/// Millisecond offsets/durations (`*Ms` fields).
pub type Ms = i64;

fn round_f64(v: f64) -> i64 {
    // Math.round semantics for the non-negative values we store; ties away from zero otherwise.
    v.round() as i64
}

/// Reads a JSON number (integer or float) as integer ms, rounding.
pub fn de_ms<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    f64::deserialize(d).map(round_f64)
}

/// Optional variant of [`de_ms`].
pub fn de_ms_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map(round_f64))
}

/// Optional list variant of [`de_ms`] (H7 part offsets).
pub fn de_ms_vec_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<i64>>, D::Error> {
    Ok(Option::<Vec<f64>>::deserialize(d)?.map(|v| v.into_iter().map(round_f64).collect()))
}

macro_rules! kebab_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[cfg_attr(feature = "specta", derive(specta::Type))]
        pub enum $name { $(#[serde(rename = $s)] $variant),+ }
        impl $name {
            /// Every variant, in zod enum order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            /// The wire string (`"google-meet"`, ...).
            pub fn as_str(self) -> &'static str { match self { $($name::$variant => $s),+ } }
            /// Parses the wire string.
            pub fn parse(s: &str) -> Option<$name> { match s { $($s => Some($name::$variant),)+ _ => None } }
        }
    };
}

kebab_enum!(MeetingPlatform {
    Zoom = "zoom", GoogleMeet = "google-meet", Teams = "teams", SlackHuddle = "slack-huddle",
    Webex = "webex", Other = "other",
});
kebab_enum!(MeetingStatus { Recording = "recording", Processing = "processing", Done = "done", Failed = "failed" });
kebab_enum!(SegmentSource { Live = "live", Post = "post" });
kebab_enum!(OsPermission { Accessibility = "accessibility", ScreenRecording = "screen-recording", Microphone = "microphone" });
kebab_enum!(PermissionState {
    Granted = "granted", Denied = "denied", NotDetermined = "not-determined", Unsupported = "unsupported",
    Unknown = "unknown",
});
kebab_enum!(RecordingPhase {
    Idle = "idle", Detected = "detected", Starting = "starting", Recording = "recording",
    Reconnecting = "reconnecting", Stopping = "stopping", Processing = "processing", Done = "done", Error = "error",
});
kebab_enum!(StopReason { Manual = "manual", MeetingEnded = "meeting-ended", HelperLost = "helper-lost", CrashRecovered = "crash-recovered" });
kebab_enum!(Degradation {
    AsrUnavailable = "asr-unavailable", AudioUnavailable = "audio-unavailable", MicOnly = "mic-only", NoMic = "no-mic",
    AxUnavailable = "ax-unavailable", TranscriptGap = "transcript-gap",
});
kebab_enum!(
    /// N95 (ADR 0062): why the current part has no speech recognition, for the live notice. Set only with
    /// `Degradation::AsrUnavailable`; the helper's own `asr.unavailable` report carries none (Electron parity).
    AsrUnavailableReason {
        NotInstalled = "not-installed", Revoked = "revoked", Stale = "stale", Crashed = "crashed",
        StartFailed = "start-failed", Timeout = "timeout", ProtocolViolations = "protocol-violations",
        Network = "network", Cancelled = "cancelled",
    }
);
kebab_enum!(
    /// N29: a raw capture channel, as `capture.silent` / `capture.audible` name it (`CaptureSource` in protocol.ts).
    CaptureSource { System = "system", Mic = "mic" }
);
kebab_enum!(
    /// N126 (ADR 0085): why a channel's speakers stopped being separated (`diarization.unavailable` in protocol.ts):
    /// `backlog`, its diarizer fell too far behind and stood down; `model-unavailable`, its model didn't load.
    DiarizationUnavailableReason { Backlog = "backlog", ModelUnavailable = "model-unavailable" }
);
kebab_enum!(StepId { Finalize = "finalize", Summary = "summary", Index = "index" });
kebab_enum!(StepStatus {
    Pending = "pending", Running = "running", Retrying = "retrying", Done = "done", Failed = "failed",
    Skipped = "skipped",
});
kebab_enum!(HelperHealth { Ok = "ok", Simulated = "simulated", Missing = "missing", Down = "down" });

#[allow(clippy::derivable_impls)] // the enum comes from a macro
impl Default for HelperHealth {
    fn default() -> Self {
        HelperHealth::Ok
    }
}

/// `record(OsPermission, PermissionState)`.
pub type PermissionsStatus = BTreeMap<OsPermission, PermissionState>;

// ------------------------------------------------------------------ meetings (types.ts, contract.ts)

/// `Meeting` extended with `durationMs` (`MeetingRow` / `MeetingWithDuration`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub platform: MeetingPlatform,
    pub started_at: EpochMs,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_ms_opt"
    )]
    pub ended_at: Option<EpochMs>,
    #[serde(default)]
    pub participants: Vec<String>,
    pub status: MeetingStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_path: Option<String>,
    /// Summary template id (templates.ts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_ms_opt"
    )]
    pub duration_ms: Option<Ms>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub id: String,
    #[serde(deserialize_with = "de_ms")]
    pub start_ms: Ms,
    #[serde(deserialize_with = "de_ms")]
    pub end_ms: Ms,
    pub text: String,
    /// Diarization slot before fusion; resolved name after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_name: Option<String>,
    pub source: SegmentSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct MeetingNote {
    pub id: String,
    #[serde(deserialize_with = "de_ms")]
    pub at_ms: Ms,
    pub text: String,
    pub created_at: EpochMs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ActionItem {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct QaItem {
    pub question: String,
    pub answer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct MeetingSummary {
    pub summary: String,
    pub action_items: Vec<ActionItem>,
    pub qa: Vec<QaItem>,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Set when the LLM failed and this is the extractive fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub meeting_id: String,
    pub title: String,
    pub started_at: EpochMs,
    pub snippet: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_ms_opt"
    )]
    pub start_ms: Option<Ms>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct MeetingListItem {
    pub meeting: Meeting,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit: Option<SearchHit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct MeetingDetail {
    pub meeting: Meeting,
    pub segments: Vec<Segment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<MeetingSummary>,
    #[serde(default)]
    pub notes: Vec<MeetingNote>,
}

// ------------------------------------------------------------------ recording lifecycle (lifecycle.ts)

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct LifecycleError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct DetectedMeeting {
    pub key: String,
    pub title: String,
    pub platform: MeetingPlatform,
    pub app: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ReconnectInfo {
    pub attempt: u32,
    pub since: EpochMs,
    pub deadline_at: EpochMs,
}

/// N29: a source the helper reported silent; live and transient (`SilentInput` in lifecycle.ts). `since_ms` is
/// meeting time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SilentInput {
    pub source: CaptureSource,
    #[serde(deserialize_with = "de_ms")]
    pub since_ms: Ms,
}

/// N29: a closed silent stretch, meeting time. `end_ms` is `None` only after a crash mid-stretch (it runs to the end
/// of the recording).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SilentSpan {
    pub source: CaptureSource,
    #[serde(deserialize_with = "de_ms")]
    pub start_ms: Ms,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_ms_opt"
    )]
    pub end_ms: Option<Ms>,
}

/// H8: one audio source that failed to start (`capture.fallback`), meeting time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct CaptureFallback {
    pub from: String,
    /// `null` (not omitted) when no source was left.
    pub to: Option<String>,
    pub reason: String,
    #[serde(deserialize_with = "de_ms")]
    pub at_ms: Ms,
}

/// N35: a capture device that failed inside a running source (`capture.degraded`): only the flags it newly took
/// away, meeting time. Never written for a permission revoke (`DeviceFailure` in lifecycle.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct DeviceFailure {
    pub degraded: Vec<Degradation>,
    pub reason: String,
    #[serde(deserialize_with = "de_ms")]
    pub at_ms: Ms,
}

/// N126 (ADR 0085): from `at_ms` (meeting time) on, `source`'s speakers are not separated for the rest of that
/// recording part (the helper's diarizer stood down or never loaded). Lines after it keep no speaker by design; this
/// says why, so the meeting can tell "not separated" from "nobody spoke".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SpeakersUnavailable {
    pub source: CaptureSource,
    #[serde(deserialize_with = "de_ms")]
    pub at_ms: Ms,
    pub reason: DiarizationUnavailableReason,
}

/// What a recording's capture looked like, carried from the session to its pipeline (N29, H8, N35).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct CaptureReport {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub silent_spans: Vec<SilentSpan>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<CaptureFallback>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_failures: Vec<DeviceFailure>,
    /// N39: stretches in which the helper did not deliver a source at all (`degraded[]` took it away); the summary
    /// treats them as that source's silence (`CaptureReport.lostSpans` in lifecycle.ts).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lost_spans: Vec<SilentSpan>,
    /// N126 (ADR 0085): channels whose speakers stopped being separated, in order (omitted when none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speakers_unavailable: Vec<SpeakersUnavailable>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct RecordingState {
    pub phase: RecordingPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected: Option<DetectedMeeting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<EpochMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<EpochMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default)]
    pub degraded: Vec<Degradation>,
    /// Helper restart in progress while recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconnect: Option<ReconnectInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<LifecycleError>,
    /// WAV parts in order: one per helper (re)start during this recording.
    #[serde(default)]
    pub audio_parts: Vec<String>,
    /// Offset added to helper timestamps for the current part (ms since startedAt).
    #[serde(default, deserialize_with = "de_ms")]
    pub part_offset_ms: Ms,
    /// H7 (ADR 0032): each part's `part_offset_ms`, parallel to `audio_parts`; written by the first
    /// `HelperRestored`, so omitted for a one-part recording. Missing with several parts (older row): no padding.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_ms_vec_opt"
    )]
    pub part_offsets_ms: Option<Vec<Ms>>,
    /// N29: sources silent right now (omitted when none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub silent: Vec<SilentInput>,
    /// N29: closed silent stretches of this recording (omitted when none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub silent_spans: Vec<SilentSpan>,
    /// H8: capture sources that failed to start during this recording (omitted when none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<CaptureFallback>,
    /// N35: the `degraded` flags held only because a capture device failed; a reconnect drops them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_lost: Vec<Degradation>,
    /// N95 (ADR 0062): why `degraded` holds `asr-unavailable`, when the speech session said; `None` whenever the
    /// flag is not set (the reducer enforces it). Omitted when none, so older rows and the TS oracle are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_unavailable: Option<AsrUnavailableReason>,
    /// N35: device failures of this recording, for the meeting record (omitted when none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_failures: Vec<DeviceFailure>,
    /// N39: stretches in which a source was lost; one without `end_ms` is still lost (ends at the reconnect that
    /// brings it back, or at the stop).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lost_spans: Vec<SilentSpan>,
}

impl Default for RecordingState {
    /// `initialRecordingState()`.
    fn default() -> Self {
        RecordingState {
            phase: RecordingPhase::Idle,
            detected: None,
            meeting_id: None,
            started_at: None,
            stopped_at: None,
            stop_reason: None,
            degraded: Vec::new(),
            reconnect: None,
            error: None,
            audio_parts: Vec::new(),
            part_offset_ms: 0,
            part_offsets_ms: None,
            silent: Vec::new(),
            silent_spans: Vec::new(),
            fallbacks: Vec::new(),
            device_lost: Vec::new(),
            asr_unavailable: None,
            device_failures: Vec::new(),
            lost_spans: Vec::new(),
        }
    }
}

/// Events fed to the session reducer (`LifecycleEvent` in lifecycle.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum LifecycleEvent {
    Detected {
        meeting: DetectedMeeting,
    },
    MeetingEnded {
        key: String,
    },
    StartRequested {
        meeting_id: String,
        at: EpochMs,
        audio_path: String,
    },
    StartAcked,
    StartFailed {
        error: LifecycleError,
    },
    /// `at` (wall clock) closes a silent stretch of a source the flag takes away (N29).
    Degraded {
        flag: Degradation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<EpochMs>,
    },
    /// N29 `capture.silent`: `since_ms` is helper (part-relative) time.
    InputSilent {
        source: CaptureSource,
        since_ms: Ms,
    },
    /// N29 `capture.audible`, stamped by the host.
    InputAudible {
        source: CaptureSource,
        at: EpochMs,
    },
    /// N35 `capture.degraded`, stamped by the host.
    CaptureDegraded {
        degraded: Vec<Degradation>,
        reason: String,
        at: EpochMs,
    },
    /// H8 `capture.fallback`, stamped by the host. `to: None` is written as `null`.
    CaptureFallback {
        from: String,
        to: Option<String>,
        reason: String,
        at: EpochMs,
    },
    /// N42 (ADR 0046 §3b): the current part's speech session reached `ready`; an earlier session's
    /// "speech unavailable" no longer applies.
    SpeechReady,
    /// N95 (ADR 0062): the current part's speech session failed: `Degraded {asr-unavailable}` plus the reason
    /// (the latest one wins while the flag is held).
    AsrUnavailable {
        reason: AsrUnavailableReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<EpochMs>,
    },
    HelperLost {
        at: EpochMs,
        deadline_at: EpochMs,
    },
    HelperRestored {
        at: EpochMs,
        audio_path: String,
    },
    StopRequested {
        reason: StopReason,
        at: EpochMs,
    },
    Stopped {
        at: EpochMs,
    },
    ProcessingDone,
    ProcessingFailed {
        error: LifecycleError,
    },
    Rejected {
        error: LifecycleError,
    },
    ClearError,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct StepState {
    pub status: StepStatus,
    pub attempts: u32,
    pub max_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<EpochMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<LifecycleError>,
    /// Summary only: an on-device extractive summary was saved in place of the LLM one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<EpochMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<EpochMs>,
}

impl StepState {
    /// A fresh `pending` step with no attempts (`step(maxAttempts)` in lifecycle.ts).
    pub fn pending(max_attempts: u32) -> StepState {
        StepState {
            status: StepStatus::Pending,
            attempts: 0,
            max_attempts,
            next_retry_at: None,
            error: None,
            fallback: None,
            started_at: None,
            finished_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ActiveSpeakerEvent {
    #[serde(deserialize_with = "de_ms")]
    pub at_ms: Ms,
    pub name: String,
}

/// Inputs finalize needs, persisted so a restart can resume post-processing exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PipelineContext {
    pub started_at: EpochMs,
    pub ended_at: EpochMs,
    #[serde(deserialize_with = "de_ms")]
    pub duration_ms: Ms,
    pub audio_parts: Vec<String>,
    /// H7: meeting-time start of each part (parallel to `audio_parts`); finalize pads the gaps. `None`: no padding.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "de_ms_vec_opt"
    )]
    pub part_offsets_ms: Option<Vec<Ms>>,
    pub active: Vec<ActiveSpeakerEvent>,
    pub helper_crashed: bool,
    /// Diarized speaker turns in meeting time, per channel (`mic:0`, `system:1`, ...; ADR 0058 §12). Empty when
    /// nothing diarized (Electron, the helper's own ASR): finalize then fuses AX names only, as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_turns: Vec<SpeakerTurn>,
}

/// One diarized turn: a per-channel speaker id and its span in meeting time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SpeakerTurn {
    pub speaker: String,
    #[serde(deserialize_with = "de_ms")]
    pub start_ms: Ms,
    #[serde(deserialize_with = "de_ms")]
    pub end_ms: Ms,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PipelineSteps {
    pub finalize: StepState,
    pub summary: StepState,
    pub index: StepState,
}

impl PipelineSteps {
    pub fn get(&self, id: StepId) -> &StepState {
        match id {
            StepId::Finalize => &self.finalize,
            StepId::Summary => &self.summary,
            StepId::Index => &self.index,
        }
    }
    pub fn get_mut(&mut self, id: StepId) -> &mut StepState {
        match id {
            StepId::Finalize => &mut self.finalize,
            StepId::Summary => &mut self.summary,
            StepId::Index => &mut self.index,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PipelineState {
    pub meeting_id: String,
    pub stop_reason: StopReason,
    pub steps: PipelineSteps,
    pub ctx: PipelineContext,
    pub updated_at: EpochMs,
    /// N29/H8: silent stretches and capture fallbacks of the recording (omitted when there were none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<CaptureReport>,
}

/// What the renderer gets (`PipelineState` without `ctx`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct PipelineView {
    pub meeting_id: String,
    pub stop_reason: StopReason,
    pub steps: PipelineSteps,
    pub updated_at: EpochMs,
    /// N29/H8: what the renderer shows about the recording's capture (silent stretches, fallbacks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<CaptureReport>,
}

// ------------------------------------------------------------------ live transcript (lifecycle.ts)

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LiveSegment {
    pub id: String,
    pub text: String,
    #[serde(deserialize_with = "de_ms")]
    pub start_ms: Ms,
    #[serde(deserialize_with = "de_ms")]
    pub end_ms: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LivePartial {
    pub id: String,
    pub text: String,
    #[serde(deserialize_with = "de_ms")]
    pub start_ms: Ms,
}

/// Snapshot. `partial` is nullable, not optional: `None` is written as `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LiveTranscript {
    pub meeting_id: String,
    pub rev: u64,
    pub segments: Vec<LiveSegment>,
    pub partial: Option<LivePartial>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LiveTranscriptDelta {
    pub meeting_id: String,
    pub base_rev: u64,
    pub rev: u64,
    pub append: Vec<LiveSegment>,
    pub partial: Option<LivePartial>,
}

/// ASR stream event from the helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum AsrEvent {
    Partial {
        utterance_id: String,
        #[serde(deserialize_with = "de_ms")]
        start_ms: Ms,
        text: String,
    },
    Final {
        utterance_id: String,
        #[serde(deserialize_with = "de_ms")]
        start_ms: Ms,
        #[serde(deserialize_with = "de_ms")]
        end_ms: Ms,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confidence: Option<f64>,
    },
}

// ------------------------------------------------------------------ session view (note/contract.ts)

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SessionLine {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub text: String,
    #[serde(deserialize_with = "de_ms")]
    pub start_ms: Ms,
}

/// What `note:session` returns and broadcasts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub phase: RecordingPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected: Option<DetectedMeeting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<EpochMs>,
    #[serde(default, deserialize_with = "de_ms")]
    pub elapsed_ms: Ms,
    #[serde(default)]
    pub participants: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_speaker: Option<String>,
    #[serde(default)]
    pub level: f64,
    /// Recent finalized lines for the HUD (last ~6).
    #[serde(default)]
    pub lines: Vec<SessionLine>,
    /// Current in-flight partial hypothesis.
    #[serde(default)]
    pub partial: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// Legacy duplicate of `error_info.message` (kept for parity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub helper: HelperHealth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_detail: Option<String>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default)]
    pub degraded: Vec<Degradation>,
    /// N35: the `degraded` flags a failed capture device set, not a permission (omitted when none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_lost: Vec<Degradation>,
    /// N95 (ADR 0062): why speech recognition is unavailable (`degraded` has `asr-unavailable`); omitted when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_unavailable: Option<AsrUnavailableReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_info: Option<LifecycleError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconnect: Option<ReconnectInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<PipelineView>,
    /// N29: sources silent right now, while capturing (omitted when none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub silent: Vec<SilentInput>,
    /// N29: every source still delivered is silent ("no audio at all"); omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_audio: bool,
    /// N155: increases with every view the runtime builds (snapshot or event), so a window applies a `note:session`
    /// snapshot only if no newer event was applied meanwhile. 0 (omitted): unordered, always applied.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub seq: u64,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_lenient_ms_match_zod() {
        // zod .default([]) / .default(0) and a fractional ms written by TS.
        let s: RecordingState =
            serde_json::from_str(r#"{"phase":"idle","partOffsetMs":12.6,"extra":1}"#).unwrap();
        assert_eq!(s.part_offset_ms, 13);
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"phase":"idle","degraded":[],"audioParts":[],"partOffsetMs":13}"#
        );
        assert_eq!(
            s,
            RecordingState {
                part_offset_ms: 13,
                ..Default::default()
            }
        );
    }

    #[test]
    fn nullable_partial_is_written_as_null() {
        let t = LiveTranscript {
            meeting_id: "m".into(),
            rev: 0,
            segments: vec![],
            partial: None,
        };
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            r#"{"meetingId":"m","rev":0,"segments":[],"partial":null}"#
        );
    }

    #[test]
    fn kebab_enums_and_tagged_events() {
        assert_eq!(
            MeetingPlatform::parse("slack-huddle"),
            Some(MeetingPlatform::SlackHuddle)
        );
        assert_eq!(
            serde_json::to_string(&Degradation::AsrUnavailable).unwrap(),
            r#""asr-unavailable""#
        );
        let e = LifecycleEvent::StopRequested {
            reason: StopReason::MeetingEnded,
            at: 5,
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"type":"stop-requested","reason":"meeting-ended","at":5}"#
        );
        assert_eq!(
            serde_json::to_string(&LifecycleEvent::StartAcked).unwrap(),
            r#"{"type":"start-acked"}"#
        );
        let a: AsrEvent =
            serde_json::from_str(r#"{"kind":"final","utteranceId":"u","startMs":1.4,"endMs":2,"text":"hi"}"#)
                .unwrap();
        assert!(matches!(a, AsrEvent::Final { start_ms: 1, .. }));
    }
}
