//! What note and talk see (ADR 0046 §2): a model-agnostic `SpeechEngine`. `OnDeviceSpeech` (the module) and the
//! host-side ElevenLabs adapter implement it; `qrtz-talk` consumes it.
use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Recording,
    Dictation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
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
#[serde(rename_all = "lowercase")]
pub enum StatusPhase {
    Loading,
    Ready,
    Failed,
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
        error: Option<String>,
    },
    Words {
        utterance_id: String,
        channel: Option<String>,
        is_final: bool,
        language: String,
        words: Vec<SpeechWordOut>,
    },
    Speaker {
        channel: String,
        speaker: String,
        start_ms: u64,
        end_ms: u64,
    },
    Finished {
        duration_ms: u64,
    },
    /// The session ended without `finished` (crash, violations, cancel); the reason is for logs and the UI state.
    Failed {
        reason: String,
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

/// A running session. Audio reaches the engine out of band (the helper writes the module's fds); for engines that
/// take audio in-process (the ElevenLabs adapter), `push_audio` carries PCM16LE 16 kHz mono.
#[async_trait]
pub trait SpeechSession: Send {
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
    /// Opaque, user-visible engine info (version), never a model name in client code.
    fn info(&self) -> serde_json::Value;
    async fn start(&self, spec: SessionSpec) -> Result<Box<dyn SpeechSession>, crate::SpeechError>;
}
