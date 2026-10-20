//! Talk on the shared speech seam (`qrtz_speech_types::{SpeechEngine, SpeechSession, SpeechEvent}`, ADR 0046). Talk
//! never names a model: the on-device engine is whatever `SpeechEngine` the host injects (the qrtz speech module).
//! There is no cloud engine (ADR 0076).
//!
//! - [`HelperAsrSpeech`]: transitional. Today's helper still sends `dictation.asr`; this adapter turns those
//!   lines into `SpeechEvent::Words` so the service has one path. It goes away with the helper's asr lines.
//! - [`drive`]: runs one `SpeechSession` on a task (audio / finish / cancel in, events out).
//! - [`TalkValidator`]: talk's own bounds on every event it consumes. The engine validates module lines
//!   (`qrtz_speech_types::validate::OutputValidator`); this is defence in depth on what reaches the dictation view, the
//!   clipboard and the focused app.
use crate::contract::TalkErrorKind;
use async_trait::async_trait;
use qrtz_speech_types::engine::SpeechWordOut;
use qrtz_speech_types::protocol::limit;
use qrtz_speech_types::{
    Channel, EngineInfo, EngineKind, FailReason, SessionSpec, SpeechEngine, SpeechError, SpeechEvent,
    SpeechSession,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Cap of the helper's full final text in `dictation.stopped` (untrusted too).
pub const MAX_FINAL_CHARS: usize = 20_000;
/// Word times above this are nonsense for a dictation.
pub const MAX_TIME_MS: u64 = 6 * 3600 * 1000;
/// Rejected events in one session before talk treats the engine as broken (the protocol's `violationsToFail`).
pub fn max_rejects() -> u32 {
    limit("violationsToFail") as u32
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(crate::poison::recover)
}

/// The full final text in `dictation.stopped`: cap it and drop control characters.
pub fn bound_final(t: &str) -> String {
    t.chars()
        .take(MAX_FINAL_CHARS)
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ validation

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject(pub &'static str);

/// Words a dictation can use, after [`TalkValidator::words`].
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    pub utterance_id: String,
    pub is_final: bool,
    pub text: String,
}

/// Per-session checks on `SpeechEvent::Words`: the session's channels only, bounded word count and sizes, no
/// control characters, timestamps that only move forward (a final may overlap the previous final by 500 ms, as
/// streaming recognisers do at chunk edges, the same rule as the module validator).
#[derive(Debug, Clone)]
pub struct TalkValidator {
    channels: Vec<Channel>,
    last_final_end: u64,
    pub rejected: u32,
}

impl TalkValidator {
    pub fn new(channels: &[Channel]) -> Self {
        Self {
            channels: channels.to_vec(),
            last_final_end: 0,
            rejected: 0,
        }
    }

    pub fn words(&mut self, ev: &SpeechEvent) -> Result<Heard, Reject> {
        let r = self.check(ev);
        if r.is_err() {
            self.rejected += 1;
        }
        r
    }

    fn check(&mut self, ev: &SpeechEvent) -> Result<Heard, Reject> {
        let SpeechEvent::Words {
            utterance_id,
            channel,
            is_final,
            language,
            words,
        } = ev
        else {
            return Err(Reject("not words"));
        };
        // N68: the channel is typed now; it must still be one of this session's.
        if !self.channels.contains(channel) {
            return Err(Reject("channel not in this session"));
        }
        let ok_label = |s: &str, max: usize| {
            !s.is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control)
        };
        if !ok_label(utterance_id, limit("idChars") as usize) {
            return Err(Reject("bad utterance id"));
        }
        if !(2..=16).contains(&language.len())
            || !language.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(Reject("bad language"));
        }
        if words.len() as u64 > limit("wordsPerEvent") {
            return Err(Reject("too many words"));
        }
        let mut prev = 0;
        let mut chars = 0usize;
        for w in words {
            if !ok_label(&w.text, limit("wordChars") as usize) {
                return Err(Reject("bad word"));
            }
            if w.end_ms < w.start_ms || w.start_ms < prev || w.end_ms > MAX_TIME_MS {
                return Err(Reject("word times go backwards or out of range"));
            }
            prev = w.start_ms;
            chars += w.text.chars().count() + 1;
        }
        if chars as u64 > limit("utteranceChars") {
            return Err(Reject("utterance too long"));
        }
        if let Some(first) = words.first() {
            if first.start_ms + 500 < self.last_final_end {
                return Err(Reject("words go back in time"));
            }
        }
        if *is_final {
            if let Some(l) = words.last() {
                self.last_final_end = self.last_final_end.max(l.end_ms);
            }
        }
        let text = words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(Heard {
            utterance_id: utterance_id.clone(),
            is_final: *is_final,
            text,
        })
    }
}

/// Words from plain text (today's helper gives no per-word times): one span for the whole segment.
pub fn words_of(text: &str, start_ms: u64, end_ms: u64) -> Vec<SpeechWordOut> {
    let max = limit("wordsPerEvent") as usize;
    text.split_whitespace()
        .take(max)
        .map(|w| SpeechWordOut {
            text: w.chars().take(limit("wordChars") as usize).collect(),
            start_ms,
            end_ms: end_ms.max(start_ms),
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ engine checks

/// A refused `start` as a dictation error: fixed texts only (no version, no module text; N72/N78).
pub fn start_error(e: &SpeechError) -> (TalkErrorKind, String) {
    let (k, m) = match e {
        SpeechError::NotInstalled => (TalkErrorKind::SpeechUnavailable, MSG_NOT_INSTALLED),
        SpeechError::Incompatible(_) => (
            TalkErrorKind::SpeechIncompatible,
            "this qrtz speech version doesn't fit this app; update qrtz",
        ),
        SpeechError::Denied(_) | SpeechError::RevocationExpired => {
            (TalkErrorKind::SpeechUnavailable, MSG_REVOKED)
        }
        SpeechError::Stale { .. } => (TalkErrorKind::SpeechUnavailable, MSG_STALE),
        SpeechError::Busy => (
            TalkErrorKind::SpeechUnavailable,
            "qrtz speech is busy (an update is being applied)",
        ),
        _ => (TalkErrorKind::SpeechLost, "qrtz speech could not start"),
    };
    (k, m.into())
}

/// ADR 0175: echo's own view offers its speech model's download in place, so point there, not at settings.
pub const MSG_NOT_INSTALLED: &str =
    "echo's speech model isn't downloaded yet; open echo to download it (about 464 MB)";
const MSG_REVOKED: &str =
    "this qrtz speech version was withdrawn, so talk is unavailable until qrtz connects to update it";
const MSG_STALE: &str =
    "qrtz speech hasn't checked for updates in too long, so talk is unavailable until qrtz connects";

/// Why an on-device session ended mid-dictation, as a dictation error. "Not installed", "revoked" and "stale" are
/// `speech-unavailable` (fixable by the user: download or connect); a crash, a protocol violation,
/// a failed start or a timeout is `speech-lost`.
pub fn fail_error(r: FailReason) -> (TalkErrorKind, &'static str) {
    match r {
        FailReason::NotInstalled => (TalkErrorKind::SpeechUnavailable, MSG_NOT_INSTALLED),
        FailReason::Revoked => (TalkErrorKind::SpeechUnavailable, MSG_REVOKED),
        FailReason::Stale => (TalkErrorKind::SpeechUnavailable, MSG_STALE),
        FailReason::Crashed
        | FailReason::ProtocolViolations
        | FailReason::StartFailed
        | FailReason::Timeout
        | FailReason::Cancelled => (
            TalkErrorKind::SpeechLost,
            "qrtz speech stopped working, so dictation stopped",
        ),
    }
}

// ------------------------------------------------------------------------------------------------ driving a session

enum Cmd {
    Audio(Vec<u8>),
    Finish,
}

/// A session running on its own task. Dropping it cancels the session.
pub struct SessionCtl {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl SessionCtl {
    pub fn push_audio(&self, pcm: Vec<u8>) {
        let _ = self.tx.send(Cmd::Audio(pcm));
    }
    pub fn finish(&self) {
        let _ = self.tx.send(Cmd::Finish);
    }
}

/// Run `session` until it reports `Finished` or `Failed`, or the [`SessionCtl`] is dropped (then it is cancelled).
/// `on_event` runs on the task, never under a caller's lock.
pub fn drive(
    mut session: Box<dyn SpeechSession>,
    channel: Channel,
    on_event: Arc<dyn Fn(SpeechEvent) + Send + Sync>,
) -> SessionCtl {
    let (tx, mut rx) = mpsc::unbounded_channel::<Cmd>();
    tokio::spawn(async move {
        let mut finishing = false;
        loop {
            tokio::select! {
                cmd = rx.recv(), if !finishing => match cmd {
                    Some(Cmd::Audio(pcm)) => {
                        if session.push_audio(channel, &pcm).await.is_err() {
                            on_event(SpeechEvent::Failed { reason: FailReason::Crashed });
                            session.cancel().await;
                            return;
                        }
                    }
                    Some(Cmd::Finish) => {
                        finishing = true;
                        if session.finish().await.is_err() {
                            on_event(SpeechEvent::Failed { reason: FailReason::Crashed });
                            return;
                        }
                    }
                    None => {
                        session.cancel().await;
                        return;
                    }
                },
                ev = session.events().recv() => match ev {
                    Some(ev) => {
                        let end = matches!(ev, SpeechEvent::Finished { .. } | SpeechEvent::Failed { .. });
                        on_event(ev);
                        if end {
                            return;
                        }
                    }
                    None => {
                        on_event(SpeechEvent::Failed { reason: FailReason::Crashed });
                        return;
                    }
                },
            }
        }
    });
    SessionCtl { tx }
}

// ------------------------------------------------------------------------------------------------ today's helper

/// Engine version of the host-side adapter (audit only, never shown).
const ADAPTER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Transitional: today's helper recognises speech itself and sends `dictation.asr {id, event {kind, utteranceId,
/// startMs, endMs?, text}}`. Each dictation id is a session; `finish` ends it (the helper's full text arrives with
/// `dictation.stopped`). Remove when the helper's asr lines go away (ADR 0046 §3b).
#[derive(Default)]
pub struct HelperAsrSpeech {
    sessions: Arc<Mutex<HashMap<String, mpsc::Sender<SpeechEvent>>>>,
}

impl HelperAsrSpeech {
    pub fn new() -> Self {
        Self::default()
    }
    /// `dictation.asr` lines; `true` = consumed.
    pub fn on_helper_line(&self, m: &Value) -> bool {
        if m["type"] != "dictation.asr" {
            return false;
        }
        let id = m["id"].as_str().unwrap_or_default();
        let Some(tx) = lock(&self.sessions).get(id).cloned() else {
            return true;
        };
        let e = &m["event"];
        let text = e["text"].as_str().unwrap_or_default();
        let start = e["startMs"].as_u64().unwrap_or(0);
        let end = e["endMs"].as_u64().unwrap_or(start);
        let utt: String = e["utteranceId"]
            .as_str()
            .unwrap_or("u")
            .chars()
            .take(64)
            .collect();
        let _ = tx.try_send(SpeechEvent::Words {
            utterance_id: if utt.is_empty() { "u".into() } else { utt },
            channel: Channel::Mic,
            is_final: e["kind"] == "final",
            language: "en".into(),
            words: words_of(text, start, end),
        });
        true
    }
}

struct HelperAsrSession {
    id: String,
    sessions: Arc<Mutex<HashMap<String, mpsc::Sender<SpeechEvent>>>>,
    rx: mpsc::Receiver<SpeechEvent>,
}

#[async_trait]
impl SpeechSession for HelperAsrSession {
    fn version(&self) -> &str {
        ADAPTER_VERSION
    }
    fn events(&mut self) -> &mut mpsc::Receiver<SpeechEvent> {
        &mut self.rx
    }
    async fn finish(&mut self) -> Result<(), SpeechError> {
        if let Some(tx) = lock(&self.sessions).remove(&self.id) {
            let _ = tx.try_send(SpeechEvent::Finished { duration_ms: 0 });
        }
        Ok(())
    }
    async fn cancel(&mut self) {
        lock(&self.sessions).remove(&self.id);
    }
}

impl Drop for HelperAsrSession {
    fn drop(&mut self) {
        lock(&self.sessions).remove(&self.id);
    }
}

#[async_trait]
impl SpeechEngine for HelperAsrSpeech {
    fn info(&self) -> EngineInfo {
        EngineInfo {
            version: ADAPTER_VERSION.into(),
            kind: EngineKind::OnDevice,
        }
    }
    async fn start(&self, spec: SessionSpec) -> Result<Box<dyn SpeechSession>, SpeechError> {
        let (tx, rx) = mpsc::channel(512);
        lock(&self.sessions).insert(spec.session_id.clone(), tx);
        Ok(Box::new(HelperAsrSession {
            id: spec.session_id,
            sessions: self.sessions.clone(),
            rx,
        }))
    }
}
