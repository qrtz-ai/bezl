//! `TranscriptAssembler` (packages/core/src/note/transcript.ts): folds the streaming ASR events into finalized
//! segments plus live partials. Partials for an utterance are replaced in place; a final closes it.

use indexmap::IndexMap;
use qrtz_model::{AsrEvent, Ms, Segment, SegmentSource};

use crate::js::js_trim;

/// An in-flight hypothesis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partial {
    pub utterance_id: String,
    pub start_ms: Ms,
    pub text: String,
}

/// JS `Map` semantics (insertion order, re-set keeps position) via `IndexMap` + `shift_remove`.
#[derive(Debug, Clone, Default)]
pub struct TranscriptAssembler {
    finals: IndexMap<String, Segment>,
    partials: IndexMap<String, (Ms, String)>,
}

impl TranscriptAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, e: &AsrEvent) {
        match e {
            AsrEvent::Partial {
                utterance_id,
                start_ms,
                text,
            } => {
                if self.finals.contains_key(utterance_id) {
                    return; // late partial after final
                }
                self.partials
                    .insert(utterance_id.clone(), (*start_ms, text.clone()));
            }
            AsrEvent::Final {
                utterance_id,
                start_ms,
                end_ms,
                text,
                confidence,
            } => {
                self.partials.shift_remove(utterance_id);
                let text = js_trim(text);
                if text.is_empty() {
                    self.finals.shift_remove(utterance_id);
                    return;
                }
                self.finals.insert(
                    utterance_id.clone(),
                    Segment {
                        id: utterance_id.clone(),
                        start_ms: *start_ms,
                        end_ms: (*end_ms).max(*start_ms),
                        text: text.to_string(),
                        speaker: None,
                        speaker_name: None,
                        source: SegmentSource::Live,
                        confidence: *confidence,
                    },
                );
            }
        }
    }

    /// Finals ordered by start (stable for ties, like `Array.prototype.sort`).
    pub fn segments(&self) -> Vec<Segment> {
        let mut v: Vec<Segment> = self.finals.values().cloned().collect();
        v.sort_by_key(|s| s.start_ms);
        v
    }

    /// In-flight hypotheses, oldest first.
    pub fn live_partials(&self) -> Vec<Partial> {
        let mut v: Vec<Partial> = self
            .partials
            .iter()
            .map(|(id, (start_ms, text))| Partial {
                utterance_id: id.clone(),
                start_ms: *start_ms,
                text: text.clone(),
            })
            .collect();
        v.sort_by_key(|p| p.start_ms);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partial(id: &str, at: Ms, t: &str) -> AsrEvent {
        AsrEvent::Partial {
            utterance_id: id.into(),
            start_ms: at,
            text: t.into(),
        }
    }
    fn fin(id: &str, a: Ms, b: Ms, t: &str) -> AsrEvent {
        AsrEvent::Final {
            utterance_id: id.into(),
            start_ms: a,
            end_ms: b,
            text: t.into(),
            confidence: None,
        }
    }

    #[test]
    fn folds_like_ts() {
        let mut a = TranscriptAssembler::new();
        a.push(&partial("u1", 10, "he"));
        a.push(&partial("u0", 0, "x"));
        a.push(&partial("u1", 10, "hello"));
        assert_eq!(
            a.live_partials()
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>(),
            ["x", "hello"]
        );
        a.push(&fin("u1", 10, 5, "  hello  "));
        a.push(&partial("u1", 10, "late"));
        a.push(&fin("u0", 0, 9, " "));
        assert!(a.live_partials().is_empty());
        let s = a.segments();
        assert_eq!((s.len(), s[0].text.as_str(), s[0].end_ms), (1, "hello", 10));
    }
}
