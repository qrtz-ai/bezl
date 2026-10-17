//! The on-device, key-free summarizer (port of packages/core/src/note/extractive.ts). Deterministic, and pinned
//! to the TS output by spec/vectors/llm/extractive.json. JS regex semantics are kept: `\b` and `/i` are ASCII-only
//! (no `u` flag), `\s` / `trim` use the JS whitespace set, `.` stops at line terminators, word length is UTF-16.
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use qrtz_model::{ActionItem, MeetingNote, MeetingSummary, QaItem, Segment};
use regex::Regex;

use crate::js::{is_js_space, js_trim, u16len, JS_SPACE_CLASS};
use crate::templates::template_by_id;

/// N54 (ADR 0048): at most this many action items come from the transcript, then at most
/// [`MAX_NOTE_ACTION_ITEMS`] from the notes; at most [`MAX_QA`] Q&A pairs (TS `MAX_ACTION_ITEMS`, ...).
pub const MAX_ACTION_ITEMS: usize = 50;
pub const MAX_NOTE_ACTION_ITEMS: usize = 20;
pub const MAX_QA: usize = 30;

/// `nextDifferent` (extractive.ts): for every position, the first entry at or after it that passes `ok`, and the
/// first one after that with a different key. [`NextDifferent::at`] is then O(1).
struct NextDifferent<K: PartialEq> {
    keys: Vec<K>,
    first: Vec<Option<usize>>,
    second: Vec<Option<usize>>,
}

impl<K: PartialEq> NextDifferent<K> {
    fn new(oks: &[bool], keys: Vec<K>) -> Self {
        let n = oks.len();
        let mut first = vec![None; n + 1];
        let mut second = vec![None; n + 1];
        for i in (0..n).rev() {
            let f = first[i + 1];
            if !oks[i] {
                first[i] = f;
                second[i] = second[i + 1];
            } else {
                first[i] = Some(i);
                second[i] = match f {
                    Some(f) if keys[f] == keys[i] => second[i + 1],
                    _ => f,
                };
            }
        }
        NextDifferent { keys, first, second }
    }
    /// The first entry at or after `i` that passes `ok` and whose key differs from `k`.
    fn at(&self, i: usize, k: &K) -> Option<usize> {
        let f = *self.first.get(i)?;
        match f {
            Some(f) if self.keys[f] == *k => self.second[i],
            _ => f,
        }
    }
}

const STOP_WORDS: &str = "a an the and or but if of to in on at for with is are was were be been it this that we you i they he she so just like yeah okay ok um uh do does did have has had not no yes our your my me us them from as by about can will would should could there here what when how who which";

static STOP: LazyLock<HashSet<&'static str>> = LazyLock::new(|| STOP_WORDS.split(' ').collect());
static ACTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i-u:\b(i'll|i will|we'll|we will|you'll|will you|can you|could you|let's|need to|needs to|have to|must|action item|todo|to-do|follow up|follow-up|by (monday|tuesday|wednesday|thursday|friday|tomorrow|eod|end of (day|week)|next week))\b)").unwrap()
});
static ASK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u:\b(can you|could you|will you)\b)").unwrap());
static NAMED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\p{Lu}\p{Ll}+),").unwrap());
static WORD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{N}']+").unwrap());
static TASK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?i-u:todo:?|- \[ \]|\[ \]|action:?|ai:)[{JS_SPACE_CLASS}]*([^\n\r\x{{2028}}\x{{2029}}]+)$"
    ))
    .unwrap()
});

/// `text.split(/(?<=[.!?])\s+/)`, trimmed, empties dropped.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut prev: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if is_js_space(c) && matches!(prev, Some('.' | '!' | '?')) {
            while chars.peek().is_some_and(|c| is_js_space(*c)) {
                chars.next();
            }
            parts.push(std::mem::take(&mut cur));
            prev = None;
            continue;
        }
        cur.push(c);
        prev = Some(c);
    }
    parts.push(cur);
    parts
        .iter()
        .map(|s| js_trim(s).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn words(s: &str) -> Vec<String> {
    let lower = s.to_lowercase();
    WORD_RE
        .find_iter(&lower)
        .map(|m| m.as_str().to_string())
        .collect()
}

fn content(s: &str) -> Vec<String> {
    words(s)
        .into_iter()
        .filter(|w| !STOP.contains(w.as_str()) && u16len(w) > 2)
        .collect()
}

struct Sent {
    text: String,
    speaker: Option<String>,
    idx: usize,
    seg: usize,
}

/// `extractiveSummary(segments, {maxSentences, notes, template})`.
pub fn extractive_summary(
    segments: &[Segment],
    max_sentences: Option<usize>,
    notes: &[MeetingNote],
    template: Option<&str>,
) -> MeetingSummary {
    let tpl = template_by_id(template);
    let focus: HashSet<&str> = tpl.focus.iter().copied().collect();
    let mut sents: Vec<Sent> = Vec::new();
    for (seg, s) in segments.iter().enumerate() {
        // `s.speakerName ?? s.speaker`, then dropped when empty (falsy).
        let speaker = s
            .speaker_name
            .as_ref()
            .or(s.speaker.as_ref())
            .filter(|x| !x.is_empty())
            .cloned();
        for text in split_sentences(&s.text) {
            let idx = sents.len();
            sents.push(Sent {
                text,
                speaker: speaker.clone(),
                idx,
                seg,
            });
        }
    }
    let notes: Vec<&MeetingNote> = notes.iter().filter(|n| !js_trim(&n.text).is_empty()).collect();
    let note_words: HashSet<String> = notes.iter().flat_map(|n| content(&n.text)).collect();
    let mut freq: HashMap<String, f64> = HashMap::new();
    for s in &sents {
        for w in content(&s.text) {
            *freq.entry(w).or_insert(0.0) += 1.0;
        }
    }
    let score = |s: &Sent| -> f64 {
        let ws = content(&s.text);
        if ws.len() < 3 {
            return 0.0;
        }
        let boost = ws
            .iter()
            .filter(|w| focus.contains(w.as_str()) || note_words.contains(*w))
            .count() as f64;
        let sum = ws
            .iter()
            .fold(0.0, |a, w| a + freq.get(w).copied().unwrap_or(0.0));
        (sum + boost * 3.0) / (ws.len() as f64).sqrt()
    };
    let max = max_sentences.unwrap_or(3);
    let mut ranked: Vec<(&Sent, f64)> = sents
        .iter()
        .filter(|s| !s.text.ends_with('?'))
        .map(|s| (s, score(s)))
        .collect();
    // `score(b) - score(a) || a.idx - b.idx`: scores are finite, so this is a total order.
    ranked.sort_by(|a, b| {
        let d = b.1 - a.1;
        if d != 0.0 {
            d.partial_cmp(&0.0).unwrap()
        } else {
            a.0.idx.cmp(&b.0.idx)
        }
    });
    let mut top: Vec<&Sent> = ranked.into_iter().take(max).map(|x| x.0).collect();
    top.sort_by_key(|s| s.idx);
    let transcript_part = top
        .iter()
        .map(|s| match &s.speaker {
            Some(sp) => format!("{sp}: {}", s.text),
            None => s.text.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    let notes_part = if notes.is_empty() {
        String::new()
    } else {
        let items: Vec<&str> = notes
            .iter()
            .map(|n| js_trim(&n.text).trim_end_matches(['.', ';']))
            .collect();
        format!("Your notes: {}.", items.join("; "))
    };
    let summary = [notes_part, transcript_part]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    // The addressee of "Ana, can you …": the first speaker whose first name is Ana.
    let mut by_first_name: HashMap<&str, &str> = HashMap::new();
    for x in &sents {
        if let Some(sp) = x.speaker.as_deref() {
            by_first_name
                .entry(sp.split(' ').next().unwrap_or(""))
                .or_insert(sp);
        }
    }
    let speaker_set: Vec<bool> = sents.iter().map(|x| x.speaker.is_some()).collect();
    let next_speaker = NextDifferent::new(&speaker_set, sents.iter().map(|x| x.speaker.as_deref()).collect());
    let mut action_items = Vec::new();
    for s in &sents {
        if action_items.len() >= MAX_ACTION_ITEMS {
            break;
        }
        if !ACTION_RE.is_match(&s.text) {
            continue;
        }
        let mut owner = s.speaker.clone();
        if ASK_RE.is_match(&s.text) {
            let full = NAMED_RE.captures(&s.text).map(|c| {
                by_first_name
                    .get(&c[1])
                    .map_or_else(|| c[1].to_string(), |f| f.to_string())
            });
            owner = full.or_else(|| {
                next_speaker
                    .at(s.idx + 1, &s.speaker.as_deref())
                    .and_then(|i| sents[i].speaker.clone())
            });
        }
        action_items.push(ActionItem {
            text: s.text.clone(),
            owner,
        });
    }
    // Notes phrased as tasks ("TODO …", "- [ ] …", or commitment phrasing) become action items too.
    let from_transcript = action_items.len();
    for n in &notes {
        if action_items.len() - from_transcript >= MAX_NOTE_ACTION_ITEMS {
            break;
        }
        let t = js_trim(&n.text);
        if let Some(c) = TASK_RE.captures(t) {
            action_items.push(ActionItem {
                text: js_trim(&c[1]).to_string(),
                owner: None,
            });
        } else if ACTION_RE.is_match(t) {
            action_items.push(ActionItem {
                text: t.to_string(),
                owner: None,
            });
        }
    }

    // The answer to a question: the first non-question of a later segment from another speaker (or none).
    let mut seg_start = vec![sents.len(); segments.len() + 1];
    for (i, x) in sents.iter().enumerate().rev() {
        seg_start[x.seg] = i;
    }
    for g in (0..segments.len()).rev() {
        seg_start[g] = seg_start[g].min(seg_start[g + 1]);
    }
    let not_question: Vec<bool> = sents.iter().map(|x| !x.text.ends_with('?')).collect();
    let next_answer = NextDifferent::new(
        &not_question,
        sents.iter().map(|x| x.speaker.as_deref()).collect(),
    );
    let mut qa = Vec::new();
    for s in &sents {
        if qa.len() >= MAX_QA {
            break;
        }
        if !s.text.ends_with('?') {
            continue;
        }
        if let Some(a) = next_answer.at(seg_start[s.seg + 1], &s.speaker.as_deref()) {
            qa.push(QaItem {
                question: s.text.clone(),
                answer: sents[a].text.clone(),
                asked_by: s.speaker.clone(),
            });
        }
    }
    MeetingSummary {
        summary: if summary.is_empty() {
            "No speech was transcribed.".into()
        } else {
            summary
        },
        action_items,
        qa,
        provider: "extractive".into(),
        template: Some(tpl.id.to_string()),
        llm_error: None,
    }
}
