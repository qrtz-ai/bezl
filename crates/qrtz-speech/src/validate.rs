//! Module output is untrusted (ADR 0046 §3.5): schema, bounds, rate limit, per-channel monotonic timestamps that
//! stay within the audio written so far, bounded speaker ids, a BCP-47 allowlist. A violating event is dropped;
//! `violationsToFail` of them fail the session.
use crate::engine::{SpeechEvent, SpeechWordOut, StatusPhase};
use crate::protocol::{limit, parse_speech_line, protocol};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Event(SpeechEvent),
    /// Accepted but nothing for clients (hello after the handshake, rate-limited log, …).
    Ignore,
    Dropped(String),
    /// Too many violations: the host kills the module and fails the session.
    Fail(String),
}

pub struct OutputValidator {
    violations: u32,
    /// Highest end time seen per channel (`None` key = words without a channel).
    last_end: HashMap<Option<String>, u64>,
    window_start_ms: u64,
    window_count: u64,
    rate_warned: bool,
    speakers: HashMap<String, std::collections::BTreeSet<String>>,
    utterance_chars: HashMap<String, usize>,
}

impl Default for OutputValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputValidator {
    pub fn new() -> Self {
        Self {
            violations: 0,
            last_end: HashMap::new(),
            window_start_ms: 0,
            window_count: 0,
            rate_warned: false,
            speakers: HashMap::new(),
            utterance_chars: HashMap::new(),
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
                let channel = v["channel"].as_str().map(String::from);
                let utt = v["utteranceId"].as_str().unwrap_or("").to_owned();
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
                // Monotonic per channel across finals: a final may not start before the previous final ended
                // (by more than one word of overlap, 500 ms, which streaming ASR produces at chunk edges).
                let last = *self.last_end.get(&channel).unwrap_or(&0);
                if let Some(first) = words.first() {
                    if first.start_ms + 500 < last {
                        return self.violate(format!("words go back in time ({} < {last})", first.start_ms));
                    }
                }
                if fin {
                    if let Some(lw) = words.last() {
                        self.last_end.insert(channel.clone(), lw.end_ms.max(last));
                    }
                    self.utterance_chars.remove(&utt);
                } else {
                    self.utterance_chars.insert(utt.clone(), chars);
                    if self.utterance_chars.len() > 64 {
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
                let ch = v["channel"].as_str().unwrap_or("").to_owned();
                let sp = v["speaker"].as_str().unwrap_or("").to_owned();
                if !sp.starts_with(&format!("{ch}:")) {
                    return self.violate(format!("speaker {sp} is not on channel {ch}"));
                }
                let (s, e) = (
                    v["startMs"].as_u64().unwrap_or(0),
                    v["endMs"].as_u64().unwrap_or(0),
                );
                if e < s || e > audio_ms + ahead {
                    return self.violate(format!("speaker span {s}..{e} (audio {audio_ms})"));
                }
                let set = self.speakers.entry(ch.clone()).or_default();
                set.insert(sp.clone());
                if set.len() as u64 > limit("speakersPerChannel") {
                    return self.violate("too many speakers".into());
                }
                Verdict::Event(SpeechEvent::Speaker {
                    channel: ch,
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
                Verdict::Event(SpeechEvent::Status {
                    phase,
                    error: v["error"].as_str().map(String::from),
                })
            }
            "finished" => Verdict::Event(SpeechEvent::Finished {
                duration_ms: v["durationMs"].as_u64().unwrap_or(0),
            }),
            "log" => {
                tracing::debug!(target: "qrtz_speech::module", "{}", v["msg"].as_str().unwrap_or(""));
                Verdict::Ignore
            }
            // A second hello is not part of a session.
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
