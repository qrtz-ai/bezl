//! Module output is untrusted (ADR 0046 §3.5 + review N68/N69/N78): schema, bounds, rate limit, per-channel
//! monotonic timestamps within the audio written so far, channels of this session only, typed speaker ids, a
//! BCP-47 allowlist, no control/bidi characters in text, fixed status codes, the session's own id and a duration
//! within the audio. A violating event is dropped; `violationsToFail` of them fail the session.
use crate::engine::{Channel, SpeakerId, SpeechEvent, SpeechWordOut, StatusError, StatusPhase};
use crate::protocol::{limit, parse_speech_line, protocol};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Event(SpeechEvent),
    /// Accepted but nothing for clients (log lines, a second hello).
    Ignore,
    Dropped(String),
    /// Too many violations: the host kills the module and fails the session.
    Fail(String),
}

/// N69: characters that could forge transcript lines or reorder text: C0/C1 controls (incl. CR/LF/TAB), line and
/// paragraph separators, bidi embeddings/overrides/isolates and marks, BOM.
pub fn forbidden_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{FEFF}'
        )
        || ('\u{202A}'..='\u{202E}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

pub struct OutputValidator {
    session_id: Option<String>,
    channels: BTreeSet<Channel>,
    violations: u32,
    last_end: HashMap<Channel, u64>,
    window_start_ms: u64,
    window_count: u64,
    rate_warned: bool,
    speakers: HashMap<Channel, BTreeSet<u8>>,
    open_utterances: BTreeSet<String>,
}

impl Default for OutputValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputValidator {
    /// Both channels, any session id (tests and the health check).
    pub fn new() -> Self {
        Self::build(None, &[Channel::Mic, Channel::System])
    }

    /// N68/N78: only this session's channels and id.
    pub fn for_session(session_id: &str, channels: &[Channel]) -> Self {
        Self::build(Some(session_id.to_owned()), channels)
    }

    fn build(session_id: Option<String>, channels: &[Channel]) -> Self {
        Self {
            session_id,
            channels: channels.iter().copied().collect(),
            violations: 0,
            last_end: HashMap::new(),
            window_start_ms: 0,
            window_count: 0,
            rate_warned: false,
            speakers: HashMap::new(),
            open_utterances: BTreeSet::new(),
        }
    }

    pub fn violations(&self) -> u32 {
        self.violations
    }

    fn violate(&mut self, why: String) -> Verdict {
        self.violations += 1;
        if self.violations as u64 >= limit("violationsToFail") {
            Verdict::Fail(format!("speech module protocol violations: {why}"))
        } else {
            Verdict::Dropped(why)
        }
    }

    fn channel(&self, v: &Value) -> Result<Channel, String> {
        let c = v["channel"]
            .as_str()
            .and_then(Channel::parse)
            .ok_or("missing channel")?;
        if !self.channels.contains(&c) {
            return Err(format!("channel {} is not in this session", c.as_str()));
        }
        Ok(c)
    }

    /// `now_ms`: host clock for the rate limit. `audio_ms`: audio written to the module so far (meeting time).
    pub fn check(&mut self, line: &str, now_ms: u64, audio_ms: u64) -> Verdict {
        if line.len() as u64 > limit("lineBytes") {
            return self.violate("line too long".into());
        }
        if now_ms.saturating_sub(self.window_start_ms) >= 1000 {
            self.window_start_ms = now_ms;
            self.window_count = 0;
        }
        self.window_count += 1;
        if self.window_count > limit("eventsPerSecond") {
            if !self.rate_warned {
                self.rate_warned = true;
                tracing::warn!("speech module over the event rate limit; dropping");
            }
            return Verdict::Dropped("rate limit".into());
        }
        let v = match parse_speech_line(line) {
            Ok(v) => v,
            Err(e) => return self.violate(format!("schema: {e}")),
        };
        let ahead = limit("aheadMs");
        match v["type"].as_str().unwrap_or("") {
            "words" => {
                let lang = v["language"].as_str().unwrap_or("");
                if !protocol().languages.iter().any(|l| l == lang) {
                    return self.violate(format!("language {lang} not allowed"));
                }
                let channel = match self.channel(&v) {
                    Ok(c) => c,
                    Err(e) => return self.violate(e),
                };
                let utt = v["utteranceId"].as_str().unwrap_or("").to_owned();
                if utt.chars().any(forbidden_char) {
                    return self.violate("control character in utterance id".into());
                }
                let mut words = Vec::new();
                let mut prev_start = 0;
                let mut chars = 0usize;
                for w in v["words"].as_array().into_iter().flatten() {
                    let (s, e) = (
                        w["startMs"].as_u64().unwrap_or(0),
                        w["endMs"].as_u64().unwrap_or(0),
                    );
                    if e < s || s < prev_start || e > audio_ms + ahead {
                        return self.violate(format!("word time {s}..{e} (audio {audio_ms})"));
                    }
                    prev_start = s;
                    let text = w["text"].as_str().unwrap_or("").to_owned();
                    if text.chars().any(forbidden_char) {
                        return self.violate("control or bidi character in word text".into());
                    }
                    chars += text.chars().count() + 1;
                    words.push(SpeechWordOut {
                        text,
                        start_ms: s,
                        end_ms: e,
                    });
                }
                if chars as u64 > limit("utteranceChars") {
                    return self.violate("utterance too long".into());
                }
                let fin = v["final"].as_bool().unwrap_or(false);
                // Monotonic per channel across finals (500 ms overlap tolerated at chunk edges).
                let last = *self.last_end.get(&channel).unwrap_or(&0);
                if let Some(first) = words.first() {
                    if first.start_ms + 500 < last {
                        return self.violate(format!("words go back in time ({} < {last})", first.start_ms));
                    }
                }
                if fin {
                    if let Some(lw) = words.last() {
                        self.last_end.insert(channel, lw.end_ms.max(last));
                    }
                    self.open_utterances.remove(&utt);
                } else {
                    self.open_utterances.insert(utt.clone());
                    if self.open_utterances.len() > 64 {
                        return self.violate("too many open utterances".into());
                    }
                }
                Verdict::Event(SpeechEvent::Words {
                    utterance_id: utt,
                    channel,
                    is_final: fin,
                    language: lang.into(),
                    words,
                })
            }
            "speaker" => {
                let ch = match self.channel(&v) {
                    Ok(c) => c,
                    Err(e) => return self.violate(e),
                };
                let Some(sp) = SpeakerId::parse(v["speaker"].as_str().unwrap_or(""), ch) else {
                    return self.violate(format!(
                        "speaker {} is not on channel {}",
                        v["speaker"],
                        ch.as_str()
                    ));
                };
                let (s, e) = (
                    v["startMs"].as_u64().unwrap_or(0),
                    v["endMs"].as_u64().unwrap_or(0),
                );
                if e < s || e > audio_ms + ahead {
                    return self.violate(format!("speaker span {s}..{e} (audio {audio_ms})"));
                }
                let set = self.speakers.entry(ch).or_default();
                set.insert(sp.index());
                if set.len() as u64 > limit("speakersPerChannel") {
                    return self.violate("too many speakers".into());
                }
                Verdict::Event(SpeechEvent::Speaker {
                    speaker: sp,
                    start_ms: s,
                    end_ms: e,
                })
            }
            "status" => {
                let phase = match v["phase"].as_str() {
                    Some("loading") => StatusPhase::Loading,
                    Some("ready") => StatusPhase::Ready,
                    _ => StatusPhase::Failed,
                };
                if let Some(d) = v["detail"].as_str() {
                    tracing::info!(target: "qrtz_speech::module", "status detail: {}", d.escape_debug());
                }
                let error = v["code"]
                    .as_str()
                    .map(StatusError::parse)
                    .or((phase == StatusPhase::Failed).then_some(StatusError::Internal));
                Verdict::Event(SpeechEvent::Status { phase, error })
            }
            "finished" => {
                let id = v["sessionId"].as_str().unwrap_or("");
                if self.session_id.as_deref().is_some_and(|s| s != id) {
                    return self.violate("finished for another session".into());
                }
                let d = v["durationMs"].as_u64().unwrap_or(0);
                if d > audio_ms + ahead {
                    return self.violate(format!("duration {d} beyond the audio ({audio_ms})"));
                }
                Verdict::Event(SpeechEvent::Finished { duration_ms: d })
            }
            "log" => {
                tracing::debug!(target: "qrtz_speech::module", "{}", v["msg"].as_str().unwrap_or("").escape_debug());
                Verdict::Ignore
            }
            _ => Verdict::Ignore,
        }
    }
}

/// The handshake: protocol and expected version. Confinement is checked by the host (code requirement,
/// entitlements, `sandbox_check`, Landlock/seccomp), never taken from the module's word (N57).
pub fn check_hello(line: &str, expected_version: &str) -> Result<Value, String> {
    let v = parse_speech_line(line)?;
    if v["type"] != "hello" {
        return Err("expected hello".into());
    }
    if v["protocol"].as_u64() != Some(crate::protocol::SPEECH_PROTOCOL_VERSION as u64) {
        return Err(format!("speech protocol {}", v["protocol"]));
    }
    if v["moduleVersion"].as_str() != Some(expected_version) {
        return Err(format!(
            "module says {} but the bundle is {expected_version}",
            v["moduleVersion"]
        ));
    }
    Ok(v)
}
