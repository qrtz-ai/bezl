//! `packages/core/src/talk/contract.ts` as serde types. Wire names are camelCase and numbers are written the
//! way `JSON.stringify` writes them (`1760000000000`, not `1760000000000.0`), so a `talk.json` written by
//! either build is read by the other (spec/fixtures/talk).
use serde::{Deserialize, Serialize, Serializer};

/// Where the words came from (`TalkEngineId`): the on-device qrtz speech module (through the helper; the
/// default) or an opt-in cloud streaming provider ([`crate::provider`], run in the host process).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    Elevenlabs,
    OnDevice,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Elevenlabs => "elevenlabs",
            Engine::OnDevice => "on-device",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TalkPhase {
    Idle,
    /// Mic open, text streaming.
    Listening,
    /// Key released; waiting for the provider's final text.
    Finalizing,
    Inserting,
    Error,
}

/// Why a dictation ended in `error` (`TalkView.errorKind`). Typed so the UI and callers never parse `error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TalkErrorKind {
    /// The chosen cloud provider is unreachable (or dropped / stalled) and automatic failover to the on-device
    /// stream is off. With failover on (the default, as in Electron) the on-device text is used instead.
    Offline,
    /// The helper refused the start: microphone access denied (`error{dictation.mic}`).
    MicDenied,
    /// The helper never acknowledged `dictation.start` in time.
    MicDidNotStart,
    /// `error{dictation.busy}` for our start: another dictation owns the helper (orphan cancelled first).
    Busy,
    /// The helper process went away or restarted under the dictation (T1).
    HelperLost,
    /// The helper isn't running at all, so the mic can't open.
    HelperUnavailable,
    /// The helper ended the dictation itself: microphone access withdrawn (H5).
    PermissionRevoked,
    /// The helper ended the dictation itself: the input device went away.
    MicLost,
    /// The qrtz speech module speaks a protocol version this build doesn't support.
    SpeechIncompatible,
    /// The qrtz speech module isn't installed / ready (and no cloud provider is picked).
    SpeechUnavailable,
    /// The speech module crashed or kept sending invalid output mid-dictation (what was heard is kept).
    SpeechLost,
}

impl TalkErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::MicDenied => "mic-denied",
            Self::MicDidNotStart => "mic-did-not-start",
            Self::Busy => "busy",
            Self::HelperLost => "helper-lost",
            Self::HelperUnavailable => "helper-unavailable",
            Self::PermissionRevoked => "permission-revoked",
            Self::MicLost => "mic-lost",
            Self::SpeechIncompatible => "speech-incompatible",
            Self::SpeechUnavailable => "speech-unavailable",
            Self::SpeechLost => "speech-lost",
        }
    }
}

/// How a history entry's `text` was made from its `raw` ASR text. New entries are `rules` or `off`; `llm` is only
/// read, from entries written before talk's LLM cleanup was removed (ADR 0071), and kept as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleanupMethod {
    /// Legacy (read only): the removed LLM cleanup.
    Llm,
    Rules,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InsertMethod {
    AxSelectedText,
    AxValue,
    Paste,
    None,
}

/// `f64` written like JS (`JSON.stringify(3)` is `3`).
pub fn js_num<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.fract() == 0.0 && v.abs() < 9_007_199_254_740_992.0 {
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}
fn js_num_opt<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(v) => js_num(v, s),
        None => s.serialize_none(),
    }
}

/// Milliseconds; every mark is optional because a dictation can end early.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Latency {
    /// Key-down → first partial on screen.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "js_num_opt",
        default
    )]
    pub first_partial_ms: Option<f64>,
    /// Key-up → final text.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "js_num_opt",
        default
    )]
    pub final_ms: Option<f64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "js_num_opt",
        default
    )]
    /// The on-device cleanup pass (rules; old entries: also the removed LLM pass).
    pub cleanup_ms: Option<f64>,
    /// Key-up → text inserted into the focused app.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "js_num_opt",
        default
    )]
    pub inserted_ms: Option<f64>,
}

/// One history entry (`TalkEntry` in zod; unknown keys are dropped on read like zod's default strip).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TalkEntry {
    pub id: String,
    #[serde(serialize_with = "js_num")]
    pub at: f64,
    pub raw: String,
    pub text: String,
    pub cleanup: CleanupMethod,
    pub engine: Engine,
    /// True when the cloud provider failed and the on-device text was used.
    #[serde(default)]
    pub failover: bool,
    pub insert: InsertMethod,
    pub inserted: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub app: Option<String>,
    #[serde(serialize_with = "js_num")]
    pub duration_ms: f64,
    pub latency: Latency,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TalkView {
    pub phase: TalkPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "js_num_opt")]
    pub started_at: Option<f64>,
    /// Committed (final) text so far.
    pub text: String,
    /// Current unstable hypothesis after `text`.
    pub partial: String,
    #[serde(serialize_with = "js_num")]
    pub level: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<Engine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failover: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<TalkErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<TalkEntry>,
}

impl TalkView {
    pub fn idle() -> Self {
        Self {
            phase: TalkPhase::Idle,
            id: None,
            started_at: None,
            text: String::new(),
            partial: String::new(),
            level: 0.0,
            engine: None,
            failover: None,
            error: None,
            error_kind: None,
            last: None,
        }
    }
    /// Idle, keeping the last finished entry (`{...idleTalkView(), ...(last ? {last} : {})}`).
    pub fn idle_keeping(&self) -> Self {
        Self {
            last: self.last.clone(),
            ..Self::idle()
        }
    }
    pub fn active(&self) -> bool {
        !matches!(self.phase, TalkPhase::Idle | TalkPhase::Error)
    }
    /// A new dictation can start: nothing is listening or finalizing. `inserting` takes one (N110, ADR 0074): the
    /// mic is closed and the text is final, so the insert finishes in the background while the next dictation
    /// listens.
    pub fn takes_a_start(&self) -> bool {
        !matches!(self.phase, TalkPhase::Listening | TalkPhase::Finalizing)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntry {
    /// How it should be written.
    pub term: String,
    /// What the recogniser tends to hear instead.
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl DictionaryEntry {
    /// The zod schema: trimmed term and aliases of 1-80 UTF-16 units, at most 10 aliases.
    pub fn normalized(&self) -> Option<Self> {
        let ok = |s: &str| (1..=80).contains(&s.encode_utf16().count());
        let term = self.term.trim().to_owned();
        let aliases: Vec<String> = self.aliases.iter().map(|a| a.trim().to_owned()).collect();
        (ok(&term) && aliases.len() <= 10 && aliases.iter().all(|a| ok(a))).then_some(Self { term, aliases })
    }
}
