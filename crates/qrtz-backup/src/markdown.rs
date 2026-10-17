//! Deterministic Markdown export of one meeting: a byte-for-byte port of `meetingToMarkdown`, `exportFilename` and
//! `slugify` in `packages/core/src/note/markdown.ts`. The shared vectors in `spec/vectors/export/markdown.json`
//! (written by the TS oracle) pin the output on both sides.
use std::sync::LazyLock;

use qrtz_model::{Meeting, MeetingNote, MeetingSummary, Segment};
use qrtz_storage::{Db, StorageError};
use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// `note:export` response (without `savedTo`, which the shell adds after writing the file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingExport {
    pub filename: String,
    pub markdown: String,
}

/// `note.export`: the meeting's file name and Markdown. `NotFound` for an unknown id.
pub fn export_meeting(db: &Db, id: &str) -> Result<MeetingExport, StorageError> {
    let n = db.notes();
    let m = n.get_meeting(id)?.ok_or_else(|| StorageError::NotFound {
        what: "meeting",
        id: id.to_string(),
    })?;
    let segs = n.get_segments(id)?;
    let summary = n.get_summary(id)?;
    let notes = n.get_notes(id)?;
    Ok(MeetingExport {
        filename: export_filename(&m),
        markdown: meeting_to_markdown(&m, &segs, summary.as_ref(), &notes),
    })
}

/// `fmtClock`: `m:ss` or `h:mm:ss`, negatives clamp to 0.
pub fn fmt_clock(ms: i64) -> String {
    clock(ms.max(0))
}

/// `clockLabel` (notes.ts): like `fmtClock` without the clamp.
fn clock(ms: i64) -> String {
    let t = ms.div_euclid(1000);
    let (h, m, s) = (
        t.div_euclid(3600),
        t.div_euclid(60).rem_euclid(60),
        t.rem_euclid(60),
    );
    if h != 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn platform_label(m: &Meeting) -> String {
    let raw = serde_json::to_value(m.platform)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    match raw.as_str() {
        "zoom" => "Zoom",
        "google-meet" => "Google Meet",
        "teams" => "Microsoft Teams",
        "slack-huddle" => "Slack huddle",
        "webex" => "Webex",
        "other" => "Meeting",
        _ => return raw,
    }
    .to_string()
}

static NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").unwrap());

/// `slugify`: lower case, NFKD, runs of anything but letters/digits become `-`, trimmed, at most 60 UTF-16 units
/// (JS `slice`). Where JS would cut an astral character in half (leaving a lone surrogate, which no file system
/// or JSON reader keeps), the whole character is dropped instead.
pub fn slugify(s: &str) -> String {
    let lower: String = s.to_lowercase().nfkd().collect();
    let dashed = NON_ALNUM.replace_all(&lower, "-");
    let trimmed = dashed.trim_matches('-');
    let mut out = String::new();
    let mut units = 0;
    for c in trimmed.chars() {
        units += c.len_utf16();
        if units > 60 {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        "meeting".to_string()
    } else {
        out
    }
}

/// `exportFilename`: `YYYY-MM-DD-<slug>.md` (UTC date).
pub fn export_filename(m: &Meeting) -> String {
    format!("{}-{}.md", &iso_utc(m.started_at)[..10], slugify(&m.title))
}

/// `esc`: backslash-escape `\ ` * _ [ ] # < > |`.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '#' | '<' | '>' | '|') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

/// `new Date(ms).toISOString()` minus the milliseconds and zone: `YYYY-MM-DDTHH:MM:SS` (years 0..=9999).
fn iso_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}",
        sod / 3600,
        sod / 60 % 60,
        sod % 60
    )
}

/// `meetingToMarkdown`: deterministic, times in UTC.
pub fn meeting_to_markdown(
    m: &Meeting,
    segments: &[Segment],
    summary: Option<&MeetingSummary>,
    notes: &[MeetingNote],
) -> String {
    let mut out: Vec<String> = Vec::new();
    out.push(format!("# {}", esc(&m.title)));
    out.push(String::new());
    out.push(format!(
        "- **When:** {} UTC",
        iso_utc(m.started_at)[..16].replace('T', " ")
    ));
    out.push(format!("- **Platform:** {}", platform_label(m)));
    if let Some(d) = m.duration_ms {
        out.push(format!("- **Duration:** {}", fmt_clock(d)));
    }
    if !m.participants.is_empty() {
        let p: Vec<_> = m.participants.iter().map(|x| esc(x)).collect();
        out.push(format!("- **Participants:** {}", p.join(", ")));
    }
    out.push(String::new());
    if let Some(s) = summary {
        out.extend(["## Summary".into(), String::new(), esc(&s.summary), String::new()]);
        out.extend(["## Action items".into(), String::new()]);
        if s.action_items.is_empty() {
            out.push("_None._".into());
        }
        for a in &s.action_items {
            let owner = a
                .owner
                .as_deref()
                .filter(|o| !o.is_empty())
                .map(|o| format!(" — _{}_", esc(o)))
                .unwrap_or_default();
            out.push(format!("- [ ] {}{owner}", esc(&a.text)));
        }
        out.push(String::new());
        if !s.qa.is_empty() {
            out.extend(["## Q&A".into(), String::new()]);
            for q in &s.qa {
                let by = q
                    .asked_by
                    .as_deref()
                    .filter(|b| !b.is_empty())
                    .map(|b| format!(" ({})", esc(b)))
                    .unwrap_or_default();
                out.push(format!("**Q{by}:** {}  ", esc(&q.question)));
                out.push(format!("**A:** {}", esc(&q.answer)));
                out.push(String::new());
            }
        }
    }
    if !notes.is_empty() {
        out.extend(["## My notes".into(), String::new()]);
        for n in notes {
            out.push(format!("- [{}] {}", clock(n.at_ms), esc(&n.text)));
        }
        out.push(String::new());
    }
    out.extend(["## Transcript".into(), String::new()]);
    for s in segments {
        // `speakerName ?? speaker ?? 'Speaker'`: `??` keeps an empty string.
        let who = s
            .speaker_name
            .as_deref()
            .or(s.speaker.as_deref())
            .unwrap_or("Speaker");
        out.push(format!(
            "**[{}] {}:** {}",
            fmt_clock(s.start_ms),
            esc(who),
            esc(&s.text)
        ));
        out.push(String::new());
    }
    out.push("---".into());
    out.push(format!(
        "_Exported from qrtz note · summary by {}_",
        summary.map(|s| s.provider.as_str()).unwrap_or("n/a")
    ));
    out.push(String::new());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_matches_js() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00");
        assert_eq!(iso_utc(1_760_000_000_000), "2025-10-09T08:53:20");
        assert_eq!(iso_utc(951_782_400_000), "2000-02-29T00:00:00");
        assert_eq!(iso_utc(-1), "1969-12-31T23:59:59");
    }

    #[test]
    fn clocks() {
        assert_eq!(fmt_clock(-5), "0:00");
        assert_eq!(fmt_clock(65_999), "1:05");
        assert_eq!(fmt_clock(3_600_000 + 61_000), "1:01:01");
    }

    #[test]
    fn slug_drops_a_split_astral_char() {
        let t = format!("{}𠀀", "a".repeat(59));
        assert_eq!(slugify(&t), "a".repeat(59));
        assert_eq!(slugify("!!!"), "meeting");
    }
}
