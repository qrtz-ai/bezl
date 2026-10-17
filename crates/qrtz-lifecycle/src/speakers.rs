//! Per-channel speaker attribution at finalize (ADR 0045 §3, ADR 0058 §12). Pure, no IO.
//!
//! The speech module diarizes each channel on its own: speaker ids are `mic:0`..`mic:7` and `system:0`..`system:7`
//! (`qrtz_speech::SpeakerId`), and the runtime keeps their turns in meeting time (`PipelineContext::speaker_turns`).
//! Rules:
//! - A segment's channel comes from its utterance id (`system-` ids are the system channel, every other id the mic;
//!   `p<n>-` part prefixes are skipped). Its speaker is that channel's speaker with the most overlap; a tie goes to
//!   the speaker heard first. A segment no turn of its channel overlaps keeps no speaker: the mic is never assumed
//!   to be one person, so an unattributed mic line is not "me".
//! - Default names: the mic speaker with the most speech is **"me"**; every other speaker, mic or system, is
//!   "speaker n" in order of first appearance across the meeting.
//! - AX active-speaker names (the meeting app showing who talks) beat a cluster label on the system channel, never
//!   on the mic, and never produce "me".
//! - `Segment::speaker` keeps the per-channel id; `speaker_name` is the name. A name already on a segment (a rename
//!   through `note:rename-speaker`, or an earlier finalize) is kept, so re-running finalize never undoes a rename.
//! - Without any turns (Electron, the helper's own ASR) this is exactly `fusion::name_segments_from_ax`.
use indexmap::IndexMap;
use qrtz_model::{ActiveSpeakerEvent, Ms, Segment, SpeakerTurn};

use crate::fusion::name_segments_from_ax;

/// The default name of the mic's main voice (renameable like any speaker name).
pub const ME: &str = "me";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Mic,
    System,
}

/// The channel a live segment came from, by its utterance id (see the module docs).
pub fn segment_channel(segment_id: &str) -> Channel {
    let id = match segment_id.split_once('-') {
        Some((p, rest))
            if p.len() > 1 && p.starts_with('p') && p[1..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            rest
        }
        _ => segment_id,
    };
    if id.starts_with("system-") {
        Channel::System
    } else {
        Channel::Mic
    }
}

/// `mic:N` / `system:N` (N ≤ 7); anything else is not a speaker id.
pub fn speaker_channel(id: &str) -> Option<Channel> {
    let (c, n) = id.split_once(':')?;
    if n.len() != 1 || !matches!(n.as_bytes()[0], b'0'..=b'7') {
        return None;
    }
    match c {
        "mic" => Some(Channel::Mic),
        "system" => Some(Channel::System),
        _ => None,
    }
}

fn overlap(a0: Ms, a1: Ms, b0: Ms, b1: Ms) -> Ms {
    (a1.min(b1) - a0.max(b0)).max(0)
}

/// Default names for every speaker in `turns`: "me" for the mic's main voice, "speaker n" for the rest.
pub fn default_names(turns: &[SpeakerTurn]) -> IndexMap<String, String> {
    let mut sorted: Vec<&SpeakerTurn> = turns
        .iter()
        .filter(|t| speaker_channel(&t.speaker).is_some() && t.end_ms > t.start_ms)
        .collect();
    sorted.sort_by_key(|t| (t.start_ms, t.end_ms));
    // First appearance order, and total speech per speaker.
    let mut total: IndexMap<&str, Ms> = IndexMap::new();
    for t in &sorted {
        let e = total.entry(t.speaker.as_str()).or_insert(0);
        *e = e.saturating_add(t.end_ms - t.start_ms);
    }
    let mut me: Option<(&str, Ms)> = None;
    for (s, ms) in &total {
        if speaker_channel(s) == Some(Channel::Mic) && me.is_none_or(|(_, b)| *ms > b) {
            me = Some((s, *ms));
        }
    }
    let mut out = IndexMap::new();
    let mut n = 0;
    for s in total.keys() {
        let name = if me.is_some_and(|(m, _)| m == *s) {
            ME.to_string()
        } else {
            n += 1;
            format!("speaker {n}")
        };
        out.insert(s.to_string(), name);
    }
    out
}

/// The finalize fusion: per-channel speakers, default names, AX on the system channel (module docs).
pub fn attribute(
    segments: &[Segment],
    turns: &[SpeakerTurn],
    active: &[ActiveSpeakerEvent],
    end_ms: Ms,
) -> Vec<Segment> {
    if turns.is_empty() {
        return if active.is_empty() {
            segments.to_vec()
        } else {
            name_segments_from_ax(segments, active, end_ms)
        };
    }
    let names = default_names(turns);
    let order: IndexMap<&str, usize> = names.keys().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    // AX names on their own (segments without names), used for system segments only.
    let ax = if active.is_empty() {
        None
    } else {
        let bare: Vec<Segment> = segments
            .iter()
            .map(|s| Segment {
                speaker_name: None,
                ..s.clone()
            })
            .collect();
        Some(name_segments_from_ax(&bare, active, end_ms))
    };
    segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let ch = segment_channel(&s.id);
            let mut votes: IndexMap<&str, Ms> = IndexMap::new();
            for t in turns {
                if speaker_channel(&t.speaker) != Some(ch) {
                    continue;
                }
                let o = overlap(s.start_ms, s.end_ms, t.start_ms, t.end_ms);
                if o > 0 {
                    *votes.entry(t.speaker.as_str()).or_insert(0) += o;
                }
            }
            let mut best: Option<(&str, Ms)> = None;
            for (sp, ms) in votes {
                let earlier = |a: &str, b: &str| order.get(a) < order.get(b);
                if best.is_none_or(|(b, bm)| ms > bm || (ms == bm && earlier(sp, b))) {
                    best = Some((sp, ms));
                }
            }
            let ax_name = match (&ax, ch) {
                (Some(ax), Channel::System) => ax[i].speaker_name.clone().filter(|n| n != ME),
                _ => None,
            };
            let Some((sp, _)) = best else {
                // No turn of its channel: no speaker. A system line may still take an AX name.
                return Segment {
                    speaker_name: s.speaker_name.clone().or(ax_name),
                    ..s.clone()
                };
            };
            let default = ax_name.or_else(|| names.get(sp).cloned());
            Segment {
                speaker: Some(sp.to_string()),
                speaker_name: s.speaker_name.clone().or(default),
                ..s.clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use qrtz_model::SegmentSource;

    fn seg(id: &str, a: Ms, b: Ms) -> Segment {
        Segment {
            id: id.into(),
            start_ms: a,
            end_ms: b,
            text: "x".into(),
            speaker: None,
            speaker_name: None,
            source: SegmentSource::Live,
            confidence: None,
        }
    }
    fn turn(s: &str, a: Ms, b: Ms) -> SpeakerTurn {
        SpeakerTurn {
            speaker: s.into(),
            start_ms: a,
            end_ms: b,
        }
    }
    fn names(v: &[Segment]) -> Vec<(Option<&str>, Option<&str>)> {
        v.iter()
            .map(|s| (s.speaker.as_deref(), s.speaker_name.as_deref()))
            .collect()
    }

    #[test]
    fn channels_from_ids() {
        assert_eq!(segment_channel("u1"), Channel::Mic);
        assert_eq!(segment_channel("system-u1"), Channel::System);
        assert_eq!(segment_channel("p2-system-u1"), Channel::System);
        assert_eq!(segment_channel("p2-u1"), Channel::Mic);
        assert_eq!(segment_channel("post-system-x"), Channel::Mic);
        assert_eq!(speaker_channel("mic:0"), Some(Channel::Mic));
        assert_eq!(speaker_channel("system:7"), Some(Channel::System));
        for bad in ["mic:8", "mic:10", "me", "tape:0", "mic:", ":0"] {
            assert_eq!(speaker_channel(bad), None, "{bad}");
        }
    }

    #[test]
    fn each_channel_has_its_own_speakers_and_the_mic_is_never_one_person() {
        let turns = [
            turn("system:0", 0, 1000),
            turn("mic:0", 1000, 4000), // the main mic voice
            turn("mic:1", 4000, 5000), // someone else in the room, on the same mic
            turn("system:1", 5000, 6000),
            turn("system:0", 6000, 7000),
        ];
        let segs = [
            seg("system-u1", 0, 1000),
            seg("u2", 1000, 4000),
            seg("u3", 4000, 5000),
            seg("system-u4", 5000, 6000),
            seg("p1-system-u5", 6000, 7000),
            seg("u6", 8000, 9000), // no mic turn here: no speaker, not "me"
        ];
        let out = attribute(&segs, &turns, &[], 9000);
        assert_eq!(
            names(&out),
            [
                (Some("system:0"), Some("speaker 1")),
                (Some("mic:0"), Some("me")),
                (Some("mic:1"), Some("speaker 2")),
                (Some("system:1"), Some("speaker 3")),
                (Some("system:0"), Some("speaker 1")),
                (None, None),
            ]
        );
    }

    #[test]
    fn a_mic_line_never_takes_a_system_speaker_and_ties_go_to_the_first_heard() {
        let turns = [
            turn("system:0", 0, 1000),
            turn("mic:1", 0, 500),
            turn("mic:0", 500, 1000),
        ];
        let out = attribute(&[seg("u1", 0, 1000)], &turns, &[], 1000);
        assert_eq!(out[0].speaker.as_deref(), Some("mic:1"));
        // "me" is the mic speaker with the most speech; tie → first heard (mic:1).
        assert_eq!(out[0].speaker_name.as_deref(), Some("me"));
    }

    #[test]
    fn ax_names_system_speakers_never_the_mic() {
        let turns = [turn("mic:0", 0, 1000), turn("system:0", 1000, 2000)];
        let active = [
            ActiveSpeakerEvent {
                at_ms: 0,
                name: "Ada".into(),
            },
            ActiveSpeakerEvent {
                at_ms: 1000,
                name: "Bo".into(),
            },
        ];
        let out = attribute(
            &[seg("u1", 0, 1000), seg("system-u2", 1000, 2000)],
            &turns,
            &active,
            2000,
        );
        assert_eq!(
            names(&out),
            [(Some("mic:0"), Some("me")), (Some("system:0"), Some("Bo"))]
        );
    }

    #[test]
    fn a_rename_survives_finalize_again() {
        let turns = [turn("mic:0", 0, 1000), turn("mic:1", 1000, 1500)];
        let segs = [seg("u1", 0, 1000), seg("u2", 1000, 1500)];
        let first = attribute(&segs, &turns, &[], 1500);
        assert_eq!(names(&first)[0], (Some("mic:0"), Some("me")));
        // note:rename-speaker "me" -> "Jared", "speaker 1" -> "Lee" (it updates speaker_name).
        let renamed: Vec<Segment> = first
            .into_iter()
            .map(|mut s| {
                s.speaker_name = s.speaker_name.map(|n| match n.as_str() {
                    "me" => "Jared".into(),
                    "speaker 1" => "Lee".into(),
                    _ => n,
                });
                s
            })
            .collect();
        let again = attribute(&renamed, &turns, &[], 1500);
        assert_eq!(
            names(&again),
            [(Some("mic:0"), Some("Jared")), (Some("mic:1"), Some("Lee"))]
        );
    }

    #[test]
    fn without_turns_it_is_the_ax_fusion() {
        let active = [ActiveSpeakerEvent {
            at_ms: 0,
            name: "Ada".into(),
        }];
        let segs = [seg("u1", 0, 1000)];
        assert_eq!(
            attribute(&segs, &[], &active, 1000),
            name_segments_from_ax(&segs, &active, 1000)
        );
        assert_eq!(attribute(&segs, &[], &[], 1000), segs.to_vec());
    }
}
