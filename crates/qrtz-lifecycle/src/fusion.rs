//! Live-only speaker fusion (packages/core/src/note/fusion.ts `nameSegmentsFromAx` and helpers).

use indexmap::IndexMap;
use qrtz_model::{ActiveSpeakerEvent, Ms, Segment};

use crate::js::{is_js_space, js_trim};

/// AX placeholders that mean "someone, but we don't know who" (the TS `UNKNOWN` regex, case-insensitive).
fn is_unknown(n: &str) -> bool {
    let l = n.to_lowercase();
    matches!(
        l.as_str(),
        "" | "unknown" | "unknown speaker" | "participant" | "guest" | "someone" | "…"
    ) || (l.chars().count() >= 2 && l.chars().all(|c| c == '.'))
        || (!l.is_empty() && l.chars().all(|c| c == '-'))
        || (!l.is_empty() && l.chars().all(|c| c == '?'))
}

/// `normalizeSpeakerName`: strips zero-width chars, collapses whitespace; `None` for unknown placeholders.
pub fn normalize_speaker_name(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut in_space = false;
    for c in raw
        .chars()
        .filter(|c| !matches!(c, '\u{200b}'..='\u{200d}' | '\u{feff}'))
    {
        if is_js_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    let n = js_trim(&out);
    if is_unknown(n) {
        None
    } else {
        Some(n.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakerInterval {
    pub name: String,
    pub start_ms: Ms,
    pub end_ms: Ms,
}

/// `activeSpeakerIntervals`: the AX step function as intervals ending at `end_ms`.
pub fn active_speaker_intervals(events: &[ActiveSpeakerEvent], end_ms: Ms) -> Vec<SpeakerInterval> {
    let mut sorted = events.to_vec();
    sorted.sort_by_key(|e| e.at_ms);
    let mut out: Vec<SpeakerInterval> = Vec::new();
    for (i, e) in sorted.iter().enumerate() {
        let name = normalize_speaker_name(&e.name);
        let end = sorted.get(i + 1).map_or(end_ms, |n| n.at_ms);
        let Some(name) = name else { continue };
        if end <= e.at_ms {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.name == name && last.end_ms == e.at_ms => last.end_ms = end,
            _ => out.push(SpeakerInterval {
                name,
                start_ms: e.at_ms,
                end_ms: end,
            }),
        }
    }
    out
}

fn overlap(a0: Ms, a1: Ms, b0: Ms, b1: Ms) -> Ms {
    (a1.min(b1) - a0.max(b0)).max(0)
}

/// `nameSegmentsFromAx` with the TS defaults (`lagMs` 300, `minShare` 0.25).
pub fn name_segments_from_ax(
    segments: &[Segment],
    active: &[ActiveSpeakerEvent],
    end_ms: Ms,
) -> Vec<Segment> {
    name_segments_from_ax_with(segments, active, end_ms, 300, 0.25)
}

pub fn name_segments_from_ax_with(
    segments: &[Segment],
    active: &[ActiveSpeakerEvent],
    end_ms: Ms,
    lag_ms: Ms,
    min_share: f64,
) -> Vec<Segment> {
    let end = segments.iter().map(|s| s.end_ms + lag_ms).fold(end_ms, Ms::max);
    let ivs = active_speaker_intervals(active, end);
    segments
        .iter()
        .map(|s| {
            let mut votes: IndexMap<&str, Ms> = IndexMap::new();
            for iv in &ivs {
                let o = overlap(s.start_ms + lag_ms, s.end_ms + lag_ms, iv.start_ms, iv.end_ms);
                if o != 0 {
                    *votes.entry(iv.name.as_str()).or_insert(0) += o;
                }
            }
            // Stable sort descending: the first-seen name wins a tie.
            let mut best: Option<(&str, Ms)> = None;
            for (name, ms) in votes {
                if best.is_none_or(|(_, b)| ms > b) {
                    best = Some((name, ms));
                }
            }
            let dur = (s.end_ms - s.start_ms).max(1);
            match best {
                Some((name, ms)) if ms as f64 / dur as f64 >= min_share => Segment {
                    speaker_name: Some(name.to_string()),
                    ..s.clone()
                },
                _ => s.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use qrtz_model::SegmentSource;

    fn ev(at: Ms, n: &str) -> ActiveSpeakerEvent {
        ActiveSpeakerEvent {
            at_ms: at,
            name: n.into(),
        }
    }
    fn seg(a: Ms, b: Ms) -> Segment {
        Segment {
            id: format!("s{a}"),
            start_ms: a,
            end_ms: b,
            text: "x".into(),
            speaker: None,
            speaker_name: None,
            source: SegmentSource::Live,
            confidence: None,
        }
    }

    #[test]
    fn names_and_placeholders() {
        assert_eq!(
            normalize_speaker_name("  Ada\u{200b}  Lovelace "),
            Some("Ada Lovelace".into())
        );
        for p in [
            "",
            "Unknown",
            "GUEST",
            "...",
            "…",
            "--",
            "??",
            " unknown  speaker ",
        ] {
            assert_eq!(normalize_speaker_name(p), None, "{p:?}");
        }
        assert_eq!(normalize_speaker_name("."), Some(".".into()));
    }

    #[test]
    fn intervals_merge_and_unknown_ends_previous() {
        let iv = active_speaker_intervals(
            &[ev(0, "Ada"), ev(100, "Ada"), ev(200, "guest"), ev(300, "Bo")],
            400,
        );
        assert_eq!(
            iv,
            vec![
                SpeakerInterval {
                    name: "Ada".into(),
                    start_ms: 0,
                    end_ms: 200
                },
                SpeakerInterval {
                    name: "Bo".into(),
                    start_ms: 300,
                    end_ms: 400
                }
            ]
        );
    }

    #[test]
    fn fuses_by_overlap_with_lag_and_min_share() {
        let out = name_segments_from_ax(
            &[seg(0, 1000), seg(1000, 2000), seg(5000, 9000)],
            &[ev(0, "Ada"), ev(1200, "Bo")],
            9000,
        );
        assert_eq!(out[0].speaker_name.as_deref(), Some("Ada"));
        assert_eq!(out[1].speaker_name.as_deref(), Some("Bo"));
        assert_eq!(out[2].speaker_name.as_deref(), Some("Bo"));
        let tie = name_segments_from_ax_with(&[seg(0, 100)], &[ev(0, "A"), ev(50, "B")], 100, 0, 0.25);
        assert_eq!(tie[0].speaker_name.as_deref(), Some("A"));
    }
}
