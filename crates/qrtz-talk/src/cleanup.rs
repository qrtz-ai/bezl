//! `packages/core/src/talk/cleanup.ts`. The regexes are the JS ones with JS semantics spelled out: `\b` and `\w`
//! are ASCII in JS, so they become explicit ASCII lookarounds/classes here. Checked byte for byte against TS by
//! spec/vectors/talk/cleanup.json.
use crate::contract::{CleanupMethod, DictionaryEntry};
use async_trait::async_trait;
use fancy_regex::{Captures, Regex};
use std::sync::LazyLock;
use std::time::Duration;

const W: &str = "A-Za-z0-9_";

fn re(s: &str) -> Regex {
    Regex::new(s).expect("static regex")
}

static NEW_PARAGRAPH: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"(?i)\s*(?<![{W}])new paragraph(?![{W}])[,.]?\s*")));
static NEW_LINE: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"(?i)\s*(?<![{W}])new line(?![{W}])[,.]?\s*")));
static FILLERS: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(^|[\s,])(?:u+m+|u+h+|e+r+m+|e+r|a+h+|h+m+|m+h*m+)(?=[\s,.!?]|$),?"));
static REPEATS: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"(?i)(?<![{W}])([{W}]+)(?:[\s,]+\1(?![{W}]))+")));
static TABS: LazyLock<Regex> = LazyLock::new(|| re(r"[ \t]+"));
static NL_SPACES: LazyLock<Regex> = LazyLock::new(|| re(r" *\n *"));
static SPACE_BEFORE_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"\s+([,.!?;:])"));
static SPACE_AFTER_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r#"([,.!?;:])(?=[^\s0-9"')\]])"#));
static COMMAS: LazyLock<Regex> = LazyLock::new(|| re(r",+"));
static EDGES: LazyLock<Regex> = LazyLock::new(|| re(r"^[\s,]+|[\s,]+$"));
static SENTENCE_START: LazyLock<Regex> = LazyLock::new(|| re(r"(^|[.!?]\s+|\n)([a-z])"));
static LONE_I: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"(?<![{W}])i(?![{W}])(?=[\s,.!?']|$)")));
static ENDS_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"[\p{L}\p{N}]$"));
static CHATTY: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(here('| i)s|sure|certainly|cleaned( text)?:)"));

fn rep(r: &Regex, s: &str, with: &str) -> String {
    r.replace_all(s, with).into_owned()
}

/// Writes dictionary terms the way the user spells them ("cue arts" → "qrtz" when listed as an alias).
pub fn apply_dictionary(text: &str, dict: &[DictionaryEntry]) -> String {
    let mut out = text.to_owned();
    for d in dict {
        for said in std::iter::once(&d.term).chain(d.aliases.iter()) {
            let words: Vec<String> = said
                .split_whitespace()
                .map(fancy_regex::escape)
                .map(|c| c.into_owned())
                .collect();
            if words.is_empty() {
                continue;
            }
            let Ok(r) = Regex::new(&format!("(?i)(?<![{W}]){}(?![{W}])", words.join(r"[\s-]+"))) else {
                continue;
            };
            out = r.replace_all(&out, fancy_regex::NoExpand(&d.term)).into_owned();
        }
    }
    out
}

/// On-device cleanup: spoken commands, fillers, stutters, spacing, capitalization, terminal punctuation.
pub fn rule_cleanup(raw: &str, dict: &[DictionaryEntry]) -> String {
    let mut t = format!(" {raw} ");
    t = rep(&NEW_PARAGRAPH, &t, "\n\n");
    t = rep(&NEW_LINE, &t, "\n");
    t = rep(&FILLERS, &t, "${1}");
    t = rep(&REPEATS, &t, "${1}");
    t = rep(&TABS, &t, " ");
    t = rep(&NL_SPACES, &t, "\n");
    t = rep(&SPACE_BEFORE_PUNCT, &t, "${1}");
    t = rep(&SPACE_AFTER_PUNCT, &t, "${1} ");
    t = rep(&COMMAS, &t, ",");
    t = rep(&EDGES, &t, "");
    t = SENTENCE_START
        .replace_all(&t, |c: &Captures| format!("{}{}", &c[1], c[2].to_uppercase()))
        .into_owned();
    t = rep(&LONE_I, &t, "I");
    t = apply_dictionary(&t, dict);
    if ENDS_WORD.is_match(&t).unwrap_or(false) {
        t.push('.');
    }
    t
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: &'static str,
    pub content: String,
}

pub fn build_cleanup_prompt(raw: &str, dict: &[DictionaryEntry]) -> Vec<ChatMessage> {
    let terms = dict
        .iter()
        .map(|d| d.term.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut system = String::from(
        "You clean up dictated text. Remove filler words, false starts and repetitions; when the speaker corrects \
         themselves (\"no wait\", \"I mean\") keep only the correction; fix punctuation and capitalization; format lists \
         and line breaks when clearly spoken. Keep the speaker's words, meaning, tone and language. Never answer, \
         summarize or add content. Reply with the cleaned text only.",
    );
    if !terms.is_empty() {
        system.push_str(&format!(" Spell these exactly: {terms}."));
    }
    vec![
        ChatMessage {
            role: "system",
            content: system,
        },
        ChatMessage {
            role: "user",
            content: raw.to_owned(),
        },
    ]
}

/// Rejects LLM replies that answered or rewrote instead of cleaning (lengths in UTF-16 units, like JS).
pub fn plausible_cleanup(raw: &str, out: &str) -> bool {
    let o = out.trim();
    if o.is_empty() {
        return false;
    }
    let (ol, rl) = (o.encode_utf16().count() as f64, raw.encode_utf16().count() as f64);
    if ol > rl * 1.6 + 40.0 {
        return false;
    }
    !CHATTY.is_match(o).unwrap_or(false)
}

/// The cleanup LLM call (qrtz-llm plugs in here). Dropping the future aborts the request.
#[async_trait]
pub trait CleanupLlm: Send + Sync {
    async fn complete(&self, messages: Vec<ChatMessage>) -> Result<String, String>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct CleanupResult {
    pub text: String,
    pub method: CleanupMethod,
    pub ms: f64,
    pub fallback_reason: Option<String>,
}

/// LLM cleanup raced against a timeout; the rule-based result is always ready as the fallback. `mono_ms` is the
/// injected monotonic clock (the `cleanupMs` latency).
pub async fn cleanup_dictation(
    raw: &str,
    dict: &[DictionaryEntry],
    enabled: bool,
    llm: Option<&dyn CleanupLlm>,
    timeout: Duration,
    mono_ms: &(dyn Fn() -> f64 + Send + Sync),
) -> CleanupResult {
    let t0 = mono_ms();
    let ms = || mono_ms() - t0;
    if !enabled {
        return CleanupResult {
            text: apply_dictionary(raw.trim(), dict),
            method: CleanupMethod::Off,
            ms: 0.0,
            fallback_reason: None,
        };
    }
    let rules = rule_cleanup(raw, dict);
    let fallback = |reason: String| CleanupResult {
        text: rules.clone(),
        method: CleanupMethod::Rules,
        ms: ms(),
        fallback_reason: Some(reason),
    };
    let Some(llm) = llm.filter(|_| !raw.trim().is_empty()) else {
        return CleanupResult {
            text: rules.clone(),
            method: CleanupMethod::Rules,
            ms: ms(),
            fallback_reason: None,
        };
    };
    match tokio::time::timeout(timeout, llm.complete(build_cleanup_prompt(raw, dict))).await {
        Err(_) => fallback("timeout".into()),
        Ok(Err(e)) => fallback(e),
        Ok(Ok(r)) if !plausible_cleanup(raw, &r) => fallback("implausible".into()),
        Ok(Ok(r)) => CleanupResult {
            text: apply_dictionary(r.trim(), dict),
            method: CleanupMethod::Llm,
            ms: ms(),
            fallback_reason: None,
        },
    }
}
