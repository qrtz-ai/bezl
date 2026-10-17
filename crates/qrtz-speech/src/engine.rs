//! What note and talk see (ADR 0046 §2, N72): a model-agnostic `SpeechEngine`. `OnDeviceSpeech` (the module) and
//! the host-side ElevenLabs adapter implement it; `qrtz-talk` consumes it. No model or engine names, no module
//! free text, no paths/fds/URLs in anything a client builds or receives.
use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Recording,
    Dictation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Mic,
    System,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Mic => "mic",
            Channel::System => "system",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mic" => Some(Channel::Mic),
            "system" => Some(Channel::System),
            _ => None,
        }
    }
}

/// A per-channel speaker (`mic:0`..`mic:7`, `system:0`..`system:7`). Built only by the validator, so an id from
/// another channel cannot exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpeakerId {
    channel: Channel,
    index: u8,
}

pub const MAX_SPEAKER_INDEX: u8 = 7;

impl SpeakerId {
    pub fn new(channel: Channel, index: u8) -> Option<Self> {
        (index <= MAX_SPEAKER_INDEX).then_some(Self { channel, index })
    }
    pub fn channel(self) -> Channel {
        self.channel
    }
    pub fn index(self) -> u8 {
        self.index
    }
    /// `mic:0` form; must be on `channel`.
    pub fn parse(s: &str, channel: Channel) -> Option<Self> {
        let (c, i) = s.split_once(':')?;
        if Channel::parse(c)? != channel || i.len() != 1 {
            return None;
        }
        Self::new(channel, i.parse().ok()?)
    }
}

impl std::fmt::Display for SpeakerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.channel.as_str(), self.index)
    }
}

impl Serialize for SpeakerId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeakerMode {
    Fast,
    Accurate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionSpec {
    pub session_id: String,
    pub kind: SessionKind,
    pub channels: Vec<Channel>,
    pub language_hint: Option<String>,
    pub speaker_mode: SpeakerMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EngineKind {
    OnDevice,
    Cloud,
}

/// Everything a client may know about an engine (N72).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EngineInfo {
    pub version: String,
    pub kind: EngineKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusPhase {
    Loading,
    Ready,
    Failed,
}

/// N78: fixed codes only; module text is logged, never shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StatusError {
    LoadFailed,
    EngineFailed,
    UnsupportedAudio,
    Internal,
}

impl StatusError {
    pub fn parse(s: &str) -> Self {
        match s {
            "load-failed" => Self::LoadFailed,
            "engine-failed" => Self::EngineFailed,
            "unsupported-audio" => Self::UnsupportedAudio,
            _ => Self::Internal,
        }
    }
}

/// Why a session ended without `Finished` (N72/N78).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailReason {
    /// The module exited or was killed.
    Crashed,
    /// Too many invalid events (ADR 0046 §3.5).
    ProtocolViolations,
    /// The module failed to load or start (handshake, confinement checks, `status failed`).
    StartFailed,
    /// No reply in time.
    Timeout,
    Cancelled,
    NotInstalled,
    /// The pinned version was revoked.
    Revoked,
    /// The revocation list is past the 90-day cap.
    Stale,
    /// A cloud engine's connection failed.
    Network,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SpeechWordOut {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SpeechEvent {
    Status {
        phase: StatusPhase,
        error: Option<StatusError>,
    },
    Words {
        utterance_id: String,
        channel: Channel,
        is_final: bool,
        language: String,
        words: Vec<SpeechWordOut>,
    },
    Speaker {
        speaker: SpeakerId,
        start_ms: u64,
        end_ms: u64,
    },
    Finished {
        duration_ms: u64,
    },
    Failed {
        reason: FailReason,
    },
}

impl SpeechEvent {
    /// Plain text of a words event (for clients that only want text, e.g. the dictation preview).
    pub fn text(&self) -> Option<String> {
        match self {
            SpeechEvent::Words { words, .. } => Some(
                words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        }
    }
}

/// A running session. Audio reaches the module out of band (the helper writes its fds); engines that take audio
/// in-process (the ElevenLabs adapter) get PCM16LE 16 kHz mono through `push_audio`.
#[async_trait]
pub trait SpeechSession: Send {
    /// The engine version this session is pinned to (for audit after a revocation).
    fn version(&self) -> &str;
    fn events(&mut self) -> &mut mpsc::Receiver<SpeechEvent>;
    async fn push_audio(&mut self, _channel: Channel, _pcm16: &[u8]) -> Result<(), crate::SpeechError> {
        Ok(())
    }
    /// Flush; the last event is `Finished` (or `Failed`).
    async fn finish(&mut self) -> Result<(), crate::SpeechError>;
    async fn cancel(&mut self);
}

#[async_trait]
pub trait SpeechEngine: Send + Sync {
    fn info(&self) -> EngineInfo;
    async fn start(&self, spec: SessionSpec) -> Result<Box<dyn SpeechSession>, crate::SpeechError>;
}
