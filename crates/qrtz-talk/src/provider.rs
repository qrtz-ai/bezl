//! Optional cloud streaming providers behind one trait. A provider only knows its wire protocol: how to connect
//! ([`StreamingProvider::request`]) and how to turn audio and server frames into text ([`ProviderCodec`]). The
//! transport is injected ([`crate::ws::WsConnector`]); failover to the on-device stream is [`crate::arbiter`]'s.
//!
//! The default engine is on-device (the helper's recogniser, `dictation.asr`); a cloud provider runs only when the
//! user picks it (`talk.engine = elevenlabs`, off by default). ElevenLabs Scribe v2 Realtime is the only cloud
//! option (Deepgram was removed, decisions-parity).
//!
//! Text is never attributed to a speaker here: the mic can pick up a whole room. A provider frame that carries a
//! speaker label is read for its text only (diarization belongs to the note path).
use crate::contract::{DictionaryEntry, Engine};
use crate::ws::{WsOut, WsRequest};
use base64::Engine as _;
use serde_json::{json, Value};

/// What a codec makes of one server frame.
#[derive(Debug, Clone, PartialEq)]
pub enum CodecEvent {
    /// `committed`: text that will not change; `partial`: the current hypothesis after it.
    Text {
        committed: String,
        partial: String,
    },
    /// The full final text, only after [`ProviderCodec::finish`].
    Final(String),
    Error(String),
}

pub trait ProviderCodec: Send {
    /// One PCM16 LE mono 16 kHz chunk as a frame.
    fn audio(&mut self, pcm: &[u8]) -> WsOut;
    /// Frames that flush the stream; from now on the codec reports [`CodecEvent::Final`].
    fn finish(&mut self) -> Vec<WsOut>;
    fn on_message(&mut self, text: &str) -> Vec<CodecEvent>;
    /// Committed text so far (the final when the server closes after a flush without a final frame).
    fn committed(&self) -> String;
    /// Frames sent right after connecting (before any audio).
    fn hello(&mut self, _dictionary: &[DictionaryEntry]) -> Vec<WsOut> {
        Vec::new()
    }
}

pub trait StreamingProvider: Send + Sync {
    fn engine(&self) -> Engine;
    /// The secret that holds this provider's credential (`None` = the provider needs none).
    fn secret_name(&self) -> Option<&str>;
    /// Usable with this credential (the key looked up under [`Self::secret_name`]).
    fn configured(&self, key: Option<&str>) -> bool;
    fn request(&self, key: Option<&str>, dictionary: &[DictionaryEntry]) -> WsRequest;
    fn codec(&self) -> Box<dyn ProviderCodec>;
}

pub(crate) fn join(a: &str, b: &str) -> String {
    let s = if !a.is_empty() && !b.is_empty() {
        format!("{a} {b}")
    } else if a.is_empty() {
        b.to_owned()
    } else {
        a.to_owned()
    };
    s.trim().to_owned()
}

fn query(pairs: &[(&str, &str)]) -> String {
    fn enc(s: &str) -> String {
        let mut o = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => o.push(b as char),
                b' ' => o.push('+'),
                _ => o.push_str(&format!("%{b:02X}")),
            }
        }
        o
    }
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

// ---------------------------------------------------------------------------------------------------------------
// ElevenLabs Scribe v2 Realtime (`elevenLabsStream` in packages/core/src/talk/asr.ts)

pub struct ElevenLabsProvider {
    pub model: String,
    pub base: String,
}
impl Default for ElevenLabsProvider {
    fn default() -> Self {
        Self {
            model: "scribe_v2_realtime".into(),
            base: "wss://api.elevenlabs.io".into(),
        }
    }
}

impl StreamingProvider for ElevenLabsProvider {
    fn engine(&self) -> Engine {
        Engine::Elevenlabs
    }
    fn secret_name(&self) -> Option<&str> {
        Some("asr.elevenlabs")
    }
    fn configured(&self, key: Option<&str>) -> bool {
        key.is_some_and(|k| !k.is_empty())
    }
    fn request(&self, key: Option<&str>, _d: &[DictionaryEntry]) -> WsRequest {
        let q = query(&[
            ("model_id", &self.model),
            ("audio_format", "pcm_16000"),
            ("commit_strategy", "manual"),
        ]);
        WsRequest {
            url: format!("{}/v1/speech-to-text/realtime?{q}", self.base),
            headers: vec![("xi-api-key".into(), key.unwrap_or_default().to_owned())],
            protocols: vec![],
        }
    }
    fn codec(&self) -> Box<dyn ProviderCodec> {
        Box::new(ElevenLabsCodec::default())
    }
}

#[derive(Default)]
pub struct ElevenLabsCodec {
    committed: String,
    finishing: bool,
}

impl ElevenLabsCodec {
    fn chunk(pcm: &[u8], commit: bool) -> WsOut {
        WsOut::Text(
            json!({"message_type": "input_audio_chunk",
                   "audio_base_64": base64::engine::general_purpose::STANDARD.encode(pcm),
                   "commit": commit, "sample_rate": 16000})
            .to_string(),
        )
    }
}

impl ProviderCodec for ElevenLabsCodec {
    fn audio(&mut self, pcm: &[u8]) -> WsOut {
        Self::chunk(pcm, false)
    }
    fn finish(&mut self) -> Vec<WsOut> {
        self.finishing = true;
        vec![Self::chunk(&[], true)]
    }
    fn on_message(&mut self, text: &str) -> Vec<CodecEvent> {
        let Ok(m) = serde_json::from_str::<Value>(text) else {
            return vec![];
        };
        let ty = m["message_type"].as_str().unwrap_or_default();
        let t = m["text"].as_str().unwrap_or_default();
        if ty == "partial_transcript" {
            vec![CodecEvent::Text {
                committed: self.committed.clone(),
                partial: t.to_owned(),
            }]
        } else if ty.starts_with("committed_transcript") {
            self.committed = join(&self.committed, t);
            let mut out = vec![CodecEvent::Text {
                committed: self.committed.clone(),
                partial: String::new(),
            }];
            if self.finishing {
                out.push(CodecEvent::Final(self.committed.clone()));
            }
            out
        } else if ty.to_ascii_lowercase().contains("error") {
            vec![CodecEvent::Error(
                m["error"]
                    .as_str()
                    .or(m["message"].as_str())
                    .unwrap_or(ty)
                    .chars()
                    .take(200)
                    .collect(),
            )]
        } else {
            vec![]
        }
    }
    fn committed(&self) -> String {
        self.committed.clone()
    }
}
