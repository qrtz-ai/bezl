//! The on-device cleanup of a dictation (`packages/core/src/talk/cleanup.ts`'s rules). There is no LLM cleanup:
//! it was removed for speed (ADR 0071), so the text goes in as soon as the final ASR text is in. The regexes are the JS ones with JS semantics spelled out: `\b` and `\w`
//! are ASCII in JS, so they become explicit ASCII lookarounds/classes here. Checked byte for byte against TS by
//! spec/vectors/talk/cleanup.json.
use crate::contract::{CleanupMethod, DictionaryEntry};
use fancy_regex::{Captures, Regex};
use std::sync::LazyLock;

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
pub struct CleanupResult {
    pub text: String,
    pub method: CleanupMethod,
}

/// The text a dictation inserts: the rules ([`rule_cleanup`]) with `talk.autoCleanup` on, else the trimmed ASR
/// text with only the dictionary spellings. Synchronous and local: no network, no model, no timeout (ADR 0071).
pub fn cleanup_dictation(raw: &str, dict: &[DictionaryEntry], enabled: bool) -> CleanupResult {
    if enabled {
        CleanupResult {
            text: rule_cleanup(raw, dict),
            method: CleanupMethod::Rules,
        }
    } else {
        CleanupResult {
            text: apply_dictionary(raw.trim(), dict),
            method: CleanupMethod::Off,
        }
    }
}
