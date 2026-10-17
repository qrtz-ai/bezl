//! Port of packages/core/src/note/markdown.ts (`note:export`): deterministic Markdown, times in UTC.
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

pub fn fmt_clock(ms: i64) -> String {
    let t = ms.max(0) / 1000;
    let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `clockLabel` (notes.ts): like fmt_clock but floors negative input like JS.
fn clock_label(ms: i64) -> String {
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

/// `YYYY-MM-DDTHH:MM` (UTC) of an epoch ms.
pub fn iso_minutes(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

pub fn slugify(s: &str) -> String {
    let lower: String = s.to_lowercase().nfkd().collect();
    let mut out = String::new();
    let mut dash = false;
    for c in lower.chars() {
        if c.is_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let t: String = out.trim_matches('-').chars().take(60).collect();
    if t.is_empty() {
        "meeting".into()
    } else {
        t
    }
}

pub fn export_filename(title: &str, started_at: i64) -> String {
    format!("{}-{}.md", &iso_minutes(started_at)[..10], slugify(title))
}

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

fn platform(p: &str) -> &str {
    match p {
        "zoom" => "Zoom",
        "google-meet" => "Google Meet",
        "teams" => "Microsoft Teams",
        "slack-huddle" => "Slack huddle",
        "webex" => "Webex",
        "other" => "Meeting",
        o => o,
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// `meetingToMarkdown` over a `note:get` detail (`{meeting, segments, summary?, notes}`).
pub fn meeting_to_markdown(d: &Value) -> String {
    let m = &d["meeting"];
    let mut out: Vec<String> = vec![format!("# {}", esc(s(m, "title"))), String::new()];
    out.push(format!(
        "- **When:** {} UTC",
        iso_minutes(m["startedAt"].as_i64().unwrap_or(0)).replace('T', " ")
    ));
    out.push(format!("- **Platform:** {}", platform(s(m, "platform"))));
    if let Some(ms) = m["durationMs"].as_i64() {
        out.push(format!("- **Duration:** {}", fmt_clock(ms)));
    }
    let parts: Vec<String> = arr(&m["participants"])
        .iter()
        .filter_map(Value::as_str)
        .map(esc)
        .collect();
    if !parts.is_empty() {
        out.push(format!("- **Participants:** {}", parts.join(", ")));
    }
    out.push(String::new());
    let summary = d.get("summary").filter(|v| v.is_object());
    if let Some(sm) = summary {
        out.extend([
            "## Summary".into(),
            String::new(),
            esc(s(sm, "summary")),
            String::new(),
        ]);
        out.extend(["## Action items".into(), String::new()]);
        let items = arr(&sm["actionItems"]);
        if items.is_empty() {
            out.push("_None._".into());
        }
        for a in items {
            let owner = a["owner"]
                .as_str()
                .filter(|o| !o.is_empty())
                .map(|o| format!(" — _{}_", esc(o)))
                .unwrap_or_default();
            out.push(format!("- [ ] {}{owner}", esc(s(a, "text"))));
        }
        out.push(String::new());
        let qa = arr(&sm["qa"]);
        if !qa.is_empty() {
            out.extend(["## Q&A".into(), String::new()]);
            for q in qa {
                let by = q["askedBy"]
                    .as_str()
                    .filter(|o| !o.is_empty())
                    .map(|o| format!(" ({})", esc(o)))
                    .unwrap_or_default();
                out.push(format!("**Q{by}:** {}  ", esc(s(q, "question"))));
                out.push(format!("**A:** {}", esc(s(q, "answer"))));
                out.push(String::new());
            }
        }
    }
    let notes = arr(&d["notes"]);
    if !notes.is_empty() {
        out.extend(["## My notes".into(), String::new()]);
        for n in notes {
            out.push(format!(
                "- [{}] {}",
                clock_label(n["atMs"].as_i64().unwrap_or(0)),
                esc(s(n, "text"))
            ));
        }
        out.push(String::new());
    }
    out.extend(["## Transcript".into(), String::new()]);
    for sg in arr(&d["segments"]) {
        let who = sg["speakerName"]
            .as_str()
            .or(sg["speaker"].as_str())
            .unwrap_or("Speaker");
        out.push(format!(
            "**[{}] {}:** {}",
            fmt_clock(sg["startMs"].as_i64().unwrap_or(0)),
            esc(who),
            esc(s(sg, "text"))
        ));
        out.push(String::new());
    }
    let provider = summary.and_then(|v| v["provider"].as_str()).unwrap_or("n/a");
    out.extend([
        "---".into(),
        format!("_Exported from qrtz note · summary by {provider}_"),
        String::new(),
    ]);
    out.join("\n")
}
