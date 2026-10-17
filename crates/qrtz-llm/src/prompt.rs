//! The summary prompt (port of `formatTranscriptForPrompt`, `captureGapsForPrompt`, `captureGapGuidance`,
//! `buildSummaryPrompt`, `formatNotesForPrompt` and `parseSummaryReply` in packages/core/src/note/{llm,notes}.ts).
//! Lengths and cuts are in UTF-16 units like JS, so both sides build byte-identical prompts
//! (spec/vectors/llm/prompts.json).
use qrtz_model::{
    ActionItem, CaptureSource, LifecycleError, MeetingNote, MeetingSummary, QaItem, Segment, SilentSpan,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::js::{cut_utf16, js_trim, u16len};
use crate::templates::template_by_id;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

/// `SummaryOptions` minus the transport bits (strict and the deadline live in the step).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryOptions {
    #[serde(default)]
    pub notes: Vec<MeetingNote>,
    #[serde(default)]
    pub template: Option<String>,
    /// N29: the recording's silent stretches (PipelineState.capture).
    #[serde(default)]
    pub silent_spans: Vec<SilentSpan>,
    /// N39: stretches in which a source was lost (a revoke or a device failure took it away); they count as that
    /// source's silence.
    #[serde(default)]
    pub lost_spans: Vec<SilentSpan>,
    /// Meeting length, for a stretch with no end.
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

/// `[m:ss]` of the transcript (minutes keep counting past an hour).
pub fn clock(ms: i64) -> String {
    let t = ms.max(0) / 1000;
    format!("{}:{:02}", t / 60, t % 60)
}

/// `clockLabel` (notes.ts): `h:mm:ss` from an hour on, JS floor / remainder semantics.
pub fn clock_label(ms: i64) -> String {
    let t = ms.div_euclid(1000);
    let h = t.div_euclid(3600);
    let m = t.div_euclid(60) % 60;
    let s = pad2(t % 60);
    if h != 0 {
        format!("{h}:{}:{s}", pad2(m))
    } else {
        format!("{m}:{s}")
    }
}

/// `String(n).padStart(2, '0')`.
fn pad2(n: i64) -> String {
    let s = n.to_string();
    if s.len() < 2 {
        format!("0{s}")
    } else {
        s
    }
}

pub fn format_notes_for_prompt(notes: &[MeetingNote]) -> String {
    notes
        .iter()
        .map(|n| format!("[{}] {}", clock_label(n.at_ms), n.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The default prompt cap (`formatTranscriptForPrompt(segments)`).
pub const PROMPT_TRANSCRIPT_MAX_CHARS: usize = 60_000;

/// The transcript as prompt text, at most `max_chars` UTF-16 units. Over the budget it is shown as evenly spaced,
/// line-aligned windows across the whole meeting joined by "not shown" markers (N21, cee71d5); a single line longer
/// than its window is cut surrogate-safe (05fb2cd).
pub fn format_transcript_for_prompt(segments: &[Segment], max_chars: usize) -> String {
    let lines: Vec<String> = segments
        .iter()
        .map(|s| {
            let who = s
                .speaker_name
                .as_deref()
                .or(s.speaker.as_deref())
                .unwrap_or("Speaker");
            format!("[{}] {}: {}", clock(s.start_ms), who, s.text)
        })
        .collect();
    let out = lines.join("\n");
    if u16len(&out) <= max_chars {
        return out;
    }
    let len: Vec<usize> = lines.iter().map(|l| u16len(l)).collect();
    let k0 = ((max_chars / 7500) as i64).clamp(2, 8);
    let k = if k0 > 2 && k0 % 2 == 0 { k0 - 1 } else { k0 };
    const MARK: i64 = 40;
    let per = (((max_chars as i64 - (k - 1) * MARK) as f64 / k as f64).floor() as i64).max(1);
    let per_u = per as usize;
    let mut starts = Vec::with_capacity(lines.len());
    let mut total = 0usize;
    for l in &len {
        starts.push(total);
        total += l + 1;
    }
    let n = lines.len();
    let fits = |used: usize, i: usize| used + len[i] < per_u; // `used + len + 1 <= per`
    struct W {
        from: usize,
        to: usize,
        text: String,
    }
    let mut windows: Vec<W> = Vec::new();
    let mut next = 0usize;
    let mut j = 0i64;
    while j < k - 1 && next + 1 < n {
        let a = ((j as f64 / (k - 1) as f64) * total as f64 - per as f64 / 2.0)
            .max(0.0)
            .min((total as f64 - per as f64).max(0.0));
        let mut lo = next;
        while lo + 1 < n && (starts[lo] as f64) < a {
            lo += 1;
        }
        if lo + 1 >= n {
            break; // the rest is the last line: the end window takes it
        }
        let mut i = lo;
        let mut used = 0;
        while i + 1 < n && fits(used, i) {
            used += len[i] + 1;
            i += 1;
        }
        if i == lo {
            i = lo + 1; // one line longer than the window: its start, cut
        }
        let text = if i == lo + 1 && !fits(0, lo) {
            cut_utf16(&lines[lo], per_u).to_string()
        } else {
            lines[lo..i].join("\n")
        };
        windows.push(W {
            from: lo,
            to: i,
            text,
        });
        next = i;
        j += 1;
    }
    let mut i = n;
    let mut used = 0;
    while i > next && fits(used, i - 1) {
        i -= 1;
        used += len[i] + 1;
    }
    windows.push(if i == n {
        W {
            from: n - 1,
            to: n,
            text: cut_utf16(&lines[n - 1], per_u).to_string(),
        }
    } else {
        W {
            from: i,
            to: n,
            text: lines[i..].join("\n"),
        }
    });
    let mut r = String::new();
    let mut prev_to = 0usize;
    for w in &windows {
        if !r.is_empty() {
            if w.from > prev_to {
                r.push_str(&format!(
                    "\n[… {}–{} not shown …]\n",
                    clock(segments[prev_to].start_ms),
                    clock(segments[w.from - 1].start_ms)
                ));
            } else {
                r.push('\n');
            }
        }
        r.push_str(&w.text);
        prev_to = w.to;
    }
    r
}

/// N29: silent stretches shorter than this are not worth a prompt line.
pub const LARGE_CAPTURE_GAP_MS: i64 = 60_000;
const MAX_GAP_LINES: usize = 20;

type Iv = (i64, i64);

fn merge_iv(xs: &[Iv]) -> Vec<Iv> {
    let mut v = xs.to_vec();
    v.sort_by_key(|x| x.0);
    let mut out: Vec<Iv> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn intersect_iv(xs: &[Iv], ys: &[Iv]) -> Vec<Iv> {
    xs.iter()
        .flat_map(|&(a, b)| {
            ys.iter()
                .map(move |&(c, d)| (a.max(c), b.min(d)))
                .filter(|(x, y)| y > x)
        })
        .collect()
}

fn subtract_iv(xs: &[Iv], cut: &[Iv]) -> Vec<Iv> {
    xs.iter()
        .flat_map(|&(a, b)| {
            let mut parts = vec![(a, b)];
            for &(c, d) in cut {
                parts = parts
                    .into_iter()
                    .flat_map(|(x, y)| {
                        if d <= x || c >= y {
                            vec![(x, y)]
                        } else {
                            [(x, c), (d, y)].into_iter().filter(|(p, q)| q > p).collect()
                        }
                    })
                    .collect();
            }
            parts
        })
        .collect()
}

/// What each capture-gap line says. Only `GAP_BOTH` is a stretch where nothing at all is known (N34, c6922bc).
pub const GAP_BOTH: &str = "no audio at all (nothing was captured)";
pub const GAP_SYSTEM: &str = "no audio from the other participants (the computer's sound)";
pub const GAP_MIC: &str = "no audio from this user's microphone";

/// N29: the large capture gaps as prompt lines, in meeting order: stretches where both sources were silent, then
/// what is left of each source's own silence. `lost` stretches count as that source's silence (N39, 94e5f59). A
/// stretch with no end runs to `duration_ms` (dropped when that is unknown). At most 20 lines.
pub fn capture_gaps_for_prompt(
    spans: &[SilentSpan],
    duration_ms: Option<i64>,
    lost: &[SilentSpan],
) -> Vec<String> {
    let of = |src: CaptureSource| {
        let ivs: Vec<Iv> = spans
            .iter()
            .chain(lost)
            .filter(|x| x.source == src)
            .filter_map(|x| {
                let end = x.end_ms.or(duration_ms)?;
                (end > x.start_ms).then_some((x.start_ms, end))
            })
            .collect();
        merge_iv(&ivs)
    };
    let sys = of(CaptureSource::System);
    let mic = of(CaptureSource::Mic);
    let both = merge_iv(&intersect_iv(&sys, &mic));
    let big = |iv: &Iv| iv.1 - iv.0 >= LARGE_CAPTURE_GAP_MS;
    let mut lines: Vec<(Iv, &str)> = both.iter().filter(|i| big(i)).map(|&i| (i, GAP_BOTH)).collect();
    lines.extend(
        subtract_iv(&sys, &both)
            .into_iter()
            .filter(big)
            .map(|i| (i, GAP_SYSTEM)),
    );
    lines.extend(
        subtract_iv(&mic, &both)
            .into_iter()
            .filter(big)
            .map(|i| (i, GAP_MIC)),
    );
    lines.sort_by_key(|l| l.0 .0);
    lines
        .into_iter()
        .take(MAX_GAP_LINES)
        .map(|((a, b), what)| format!("- {}–{}: {what}", clock(a), clock(b)))
        .collect()
}

/// N34: what the model is told about the capture gaps.
pub fn capture_gap_guidance(gaps: &[String]) -> String {
    if gaps.is_empty() {
        return String::new();
    }
    let none = gaps.iter().any(|l| l.ends_with(GAP_BOTH));
    let one_sided = gaps.iter().any(|l| !l.ends_with(GAP_BOTH));
    let mut s = String::from(
        " Some stretches of the recording are listed under Capture gaps (timestamps match the transcript).",
    );
    if none {
        s.push_str(" Where it says no audio at all, nothing is known about what was said: never summarize, guess or attribute anything to that stretch; you may say that audio is missing for it.");
    }
    if one_sided {
        s.push_str(" Where only one source had no audio, the transcript lines in that stretch are real and come from the other source: summarize them as usual; just never guess what the silent side said.");
    }
    s
}

pub fn build_summary_prompt(title: &str, segments: &[Segment], opts: &SummaryOptions) -> Vec<ChatMessage> {
    let tpl = template_by_id(opts.template.as_deref());
    let notes: Vec<MeetingNote> = opts
        .notes
        .iter()
        .filter(|n| !js_trim(&n.text).is_empty())
        .cloned()
        .collect();
    let gaps = capture_gaps_for_prompt(&opts.silent_spans, opts.duration_ms, &opts.lost_spans);
    let mut system = String::from("You summarize meeting transcripts. Reply with ONLY a JSON object: {\"summary\": string (3-6 sentences), \"actionItems\": [{\"text\": string, \"owner\"?: string}], \"qa\": [{\"question\": string, \"answer\": string, \"askedBy\"?: string}]}. Use speaker names from the transcript. Do not invent facts. ");
    system.push_str(tpl.guidance);
    if !notes.is_empty() {
        system.push_str(" The user took their own notes during the meeting (timestamps match the transcript). Treat them as the user's priorities: build the summary around them, expand each with details from the transcript, keep any to-dos from the notes as action items, and never contradict the transcript.");
    }
    system.push_str(&capture_gap_guidance(&gaps));
    let mut user = format!("Meeting: {title}\n\n");
    if !notes.is_empty() {
        user.push_str(&format!("My notes:\n{}\n\n", format_notes_for_prompt(&notes)));
    }
    if !gaps.is_empty() {
        user.push_str(&format!("Capture gaps:\n{}\n\n", gaps.join("\n")));
    }
    user.push_str("Transcript:\n");
    user.push_str(&format_transcript_for_prompt(
        segments,
        PROMPT_TRANSCRIPT_MAX_CHARS,
    ));
    vec![
        ChatMessage {
            role: Role::System,
            content: system,
        },
        ChatMessage {
            role: Role::User,
            content: user,
        },
    ]
}

/// The fields a model reply supplies (`MeetingSummary` minus provider and llmError).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryReply {
    pub summary: String,
    pub action_items: Vec<ActionItem>,
    pub qa: Vec<QaItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

impl SummaryReply {
    /// `{...reply, provider, template}`.
    pub fn into_summary(self, provider: &str, template: &str) -> MeetingSummary {
        MeetingSummary {
            summary: self.summary,
            action_items: self.action_items,
            qa: self.qa,
            provider: provider.to_string(),
            template: Some(template.to_string()),
            llm_error: None,
        }
    }
}

fn provider_err(message: String) -> LifecycleError {
    LifecycleError {
        code: "E_PROVIDER".into(),
        message,
        retryable: true,
    }
}

type Obj = serde_json::Map<String, Value>;

fn req_str(o: &Obj, k: &str, path: &str) -> Result<String, String> {
    o.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("expected string at {path}{k}"))
}

fn opt_str(o: &Obj, k: &str, path: &str) -> Result<Option<String>, String> {
    match o.get(k) {
        None => Ok(None),
        Some(Value::String(x)) => Ok(Some(x.clone())),
        Some(_) => Err(format!("expected string at {path}{k}")),
    }
}

/// The zod `LlmSummaryJson` parse: required strings and arrays, optional strings (never null), unknown keys
/// dropped. `Err` names the first path that failed.
fn validate(v: &Value) -> Result<SummaryReply, String> {
    let o = v.as_object().ok_or("expected object at <root>")?;
    let arr = |k: &str| -> Result<&Vec<Value>, String> {
        o.get(k)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("expected array at {k}"))
    };
    let summary = req_str(o, "summary", "")?;
    let mut action_items = Vec::new();
    for (i, it) in arr("actionItems")?.iter().enumerate() {
        let p = format!("actionItems.{i}.");
        let it = it
            .as_object()
            .ok_or(format!("expected object at actionItems.{i}"))?;
        action_items.push(ActionItem {
            text: req_str(it, "text", &p)?,
            owner: opt_str(it, "owner", &p)?,
        });
    }
    let mut qa = Vec::new();
    for (i, it) in arr("qa")?.iter().enumerate() {
        let p = format!("qa.{i}.");
        let it = it.as_object().ok_or(format!("expected object at qa.{i}"))?;
        qa.push(QaItem {
            question: req_str(it, "question", &p)?,
            answer: req_str(it, "answer", &p)?,
            asked_by: opt_str(it, "askedBy", &p)?,
        });
    }
    Ok(SummaryReply {
        summary,
        action_items,
        qa,
        template: opt_str(o, "template", "")?,
    })
}

/// Parses a model reply that should be JSON (tolerates code fences / leading prose). Every failure is a
/// retryable E_PROVIDER: a malformed reply is a sampling accident (N8, b917847).
pub fn parse_summary_reply(text: &str) -> Result<SummaryReply, LifecycleError> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Err(provider_err("LLM reply had no JSON object".into()));
    };
    if end <= start {
        return Err(provider_err("LLM reply had no JSON object".into()));
    }
    let shape = |m: String| {
        provider_err(format!(
            "LLM reply did not match the summary shape: {}",
            cut_utf16(&m, 200)
        ))
    };
    let v: Value = serde_json::from_str(&text[start..=end]).map_err(|e| shape(e.to_string()))?;
    validate(&v).map_err(shape)
}
