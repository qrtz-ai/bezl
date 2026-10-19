//! The on-device cleanup of a dictation (`packages/core/src/talk/cleanup.ts`'s rules). There is no LLM cleanup:
//! it was removed for speed (ADR 0071), so the text goes in as soon as the final ASR text is in. The regexes are the JS ones with JS semantics spelled out: `\b` and `\w`
//! are ASCII in JS, so they become explicit ASCII lookarounds/classes here. Checked byte for byte against TS by
//! spec/vectors/talk/cleanup.json.
use crate::contract::{CleanupMethod, DictionaryEntry};
use fancy_regex::{Captures, Regex};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

const W: &str = "A-Za-z0-9_";

fn re(s: &str) -> Regex {
    Regex::new(s).expect("static regex")
}

static NEW_PARAGRAPH: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"(?i)\s*(?<![{W}])new paragraph(?![{W}])[,.]?\s*")));
static NEW_LINE: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"(?i)\s*(?<![{W}])new line(?![{W}])[,.]?\s*")));
// The filler word is group 2; whether it goes is [`keeps_filler`]'s call (ADR 0182).
static FILLERS: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(^|[\s,])(u+m+|u+h+|e+r+m+|e+r|a+h+|h+m+|m+h*m+)(?=[\s,.!?]|$),?"));
// ADR 0101: an immediate repeat of a whole word. The apostrophe is part of a word on both sides, so "you, you're"
// and "it, it's" are not repeats. Which repeats collapse is [`collapses`]'s call.
static REPEATS: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"(?i)(?<![{W}'])([{W}]+)(?:[\s,]+\1(?![{W}']))+")));
static TABS: LazyLock<Regex> = LazyLock::new(|| re(r"[ \t]+"));
static NL_SPACES: LazyLock<Regex> = LazyLock::new(|| re(r" *\n *"));
static SPACE_BEFORE_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"\s+([,.!?;:])"));
// ADR 0101 (N160): only a comma or semicolon gets a space after it. A dot, colon, "!" or "?" with no space after it
// is part of a token the recogniser wrote that way ("qrtz.ai", "config.json", "p.m.", "...", "https://").
static SPACE_AFTER_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r#"([,;])(?=[^\s0-9"')\]])"#));
static COMMAS: LazyLock<Regex> = LazyLock::new(|| re(r",+"));
static EDGES: LazyLock<Regex> = LazyLock::new(|| re(r"^[\s,]+|[\s,]+$"));
// ADR 0101 (N160): not after a dotted abbreviation ("e.g. the", "3 p.m. today").
static SENTENCE_START: LazyLock<Regex> =
    LazyLock::new(|| re(r"(^|(?<![A-Za-z]\.[A-Za-z])[.!?]\s+|\n)([a-z])"));
// ADR 0101 (N160): "i" before a dot only at a sentence end, not the "i" of "i.e.".
static LONE_I: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"(?<![{W}])i(?![{W}])(?=[\s,!?']|\.(?![A-Za-z])|$)")));
static ENDS_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"[\p{L}\p{N}]$"));
// ADR 0120 (N167): a lone "." on a word with a lowercase word after it. The recogniser lowercased what follows, so
// it is not a sentence end: on a segmented recogniser (ADR 0118) it is the "." each segment's decode writes at its cut.
// The dot is dropped unless the word is an abbreviation ([`ABBREVIATIONS`]); either way the next word stays lowercase.
// Only when the raw text has a capital letter somewhere (the recogniser writes case).
static CONTINUED_DOT: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"(?<![{W}.])([A-Za-z]+)\.(?=\s+[a-z])")));
/// A sentence end inside the text (not a decimal point, not the dots of "p.m.").
static INNER_END: LazyLock<Regex> = LazyLock::new(|| re(r"[.!?]\s"));

/// ADR 0101: the stutter-prone words, the only ones whose immediate repeat collapses ("I I think", "the the").
/// Everything else is kept as said: numbers ("10 10", "1 1 2 3"), grammatical doubles ("had had", "that that",
/// "is is", "what it was was", "her her", "come in in the morning", "turn it on on Friday", "Will will"), and
/// emphasis or idiom ("very very", "no no", "bye bye", "so so", "my my", "there there").
const STUTTERS: &[&str] = &[
    "i", "a", "an", "the", "to", "and", "but", "or", "of", "at", "for", "with", "from", "we", "you", "he",
    "she", "it", "they", "me", "our", "your", "his", "their", "this", "be", "would", "can", "could",
    "should", "did", "have", "has", "if", "when", "what", "just",
];
/// Repeats across a comma ("I love you, you know", "take it, it is yours") are a clause boundary as often as a
/// stutter; only these collapse there ("I, I think", "the, the plan").
const COMMA_STUTTERS: &[&str] = &["i", "a", "an", "the"];

/// ADR 0120: words whose "." is an abbreviation's, never a sentence end before a lowercase word ("Mr. and Mrs.",
/// "etc. and so on", "vs. them"). Their dot is kept and the next word is not capitalized. Same list in cleanup.ts.
pub const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "approx", "inc", "ltd", "corp", "mt",
    "dept", "ave", "blvd",
];

fn is_abbreviation(word: &str) -> bool {
    ABBREVIATIONS.contains(&word.to_ascii_lowercase().as_str())
}

/// Does `before` (the text up to a ". ") end with a whole abbreviation word ("... Mr", not "... Amr")?
fn ends_with_abbreviation(before: &str) -> bool {
    let word: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let rest = &before[..before.len() - word.len()];
    let whole = !rest.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    let word: String = word.chars().rev().collect();
    !word.is_empty() && whole && is_abbreviation(&word)
}

/// ADR 0182 (N186): "new line" / "new paragraph" after one of these is a noun phrase ("a new line of products",
/// "the new line at the store", "write a new paragraph"), never the command, so the words stay. Only words that can't
/// end a clause: "this", "that", "her", "one", "some", "each" can ("call her new line then ..."), so they don't count.
/// Same list in cleanup.ts.
pub const COMMAND_DETERMINERS: &[&str] = &["a", "an", "the", "my", "your", "our", "their", "its", "every"];

/// Does `before` (the text up to a spoken command) end with a whole determiner word?
fn ends_with_determiner(before: &str) -> bool {
    let word: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let rest = &before[..before.len() - word.len()];
    let whole = !rest.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '\'');
    let word: String = word.chars().rev().collect::<String>().to_ascii_lowercase();
    whole && COMMAND_DETERMINERS.contains(&word.as_str())
}

/// A spoken layout command (`NEW_PARAGRAPH`, `NEW_LINE`) becomes `with`, unless it is a noun phrase (ADR 0182).
fn layout_command(r: &Regex, s: &str, with: &str) -> String {
    r.replace_all(s, |c: &Captures| {
        let start = c.get(0).map_or(0, |m| m.start());
        if ends_with_determiner(s[..start].trim_end()) {
            c[0].to_owned()
        } else {
            with.to_owned()
        }
    })
    .into_owned()
}

/// ADR 0182 (N185): a filler-shaped word that is a real word here, so it stays. Only unambiguous forms:
/// - written in capitals ("the ER", "UM beat Ohio State", "the AH team"): the recogniser writes a filler in lowercase
///   (or with one capital at a sentence start), never in capitals, so a capitalized one is an acronym;
/// - "mm" right after a number ("a 35 mm lens", "10 mm wide"): the unit.
///
/// ("err" never matches: the pattern's `e+r` is one "r", so "to err is human" was always kept.) `all` is the whole
/// match (separator, word, optional comma), `before` the text before it.
fn keeps_filler(word: &str, all: &str, before: &str) -> bool {
    if word.len() >= 2 && word.chars().all(|c| c.is_ascii_uppercase()) {
        return true;
    }
    word.eq_ignore_ascii_case("mm")
        && all.starts_with(' ')
        && !all.ends_with(',')
        && before.ends_with(|c: char| c.is_ascii_digit())
}

/// ADR 0101: does this repeat (the whole match `all`, its word `word`) collapse to one word?
fn collapses(word: &str, all: &str) -> bool {
    let w = word.to_ascii_lowercase();
    let list = if all.contains(',') {
        COMMA_STUTTERS
    } else {
        STUTTERS
    };
    list.contains(&w.as_str())
}

/// ADR 0101: up to this many words is a short dictation (a reply, a search, a field): no "." is added, and a lone
/// "." the recogniser ended it with is dropped ("Yes", "Sounds good", "Friday at 3"). "?" and "!" stay.
pub const SHORT_DICTATION_WORDS: usize = 3;

fn is_short(t: &str) -> bool {
    !t.contains('\n')
        && t.split_whitespace().count() <= SHORT_DICTATION_WORDS
        && !INNER_END.is_match(t).unwrap_or(true)
}

/// The terminal punctuation (ADR 0101): a short dictation ends as said, without a period; a longer one gets a "."
/// when it ends on a word.
fn terminal_punctuation(mut t: String) -> String {
    if is_short(&t) {
        // A lone final "." only: not "...", not the last dot of "p.m." or "U.S.".
        let last = t.split_whitespace().last().unwrap_or_default();
        if let Some(body) = last.strip_suffix('.') {
            if !body.is_empty() && !body.contains('.') {
                t.pop();
            }
        }
    } else if ENDS_WORD.is_match(&t).unwrap_or(false) {
        t.push('.');
    }
    t
}

fn rep(r: &Regex, s: &str, with: &str) -> String {
    r.replace_all(s, with).into_owned()
}

/// ADR 0182 (N187): compiled dictionary patterns, by pattern. Compiling one case-insensitive pattern costs about 1 ms
/// (Unicode case folding), and it used to happen for every term and alias on every dictation, after release: a
/// 20-term dictionary added ~19 ms to release-to-text even on a one-line dictation. Now each pattern compiles once per
/// launch, and the full pattern (with its word-edge lookarounds, which need fancy-regex's backtracking scan) runs only
/// when its core, the same words without the edge checks, matches somewhere: any full match is a core match, so the
/// text out is byte for byte the same. Bounded: cleared past [`DICTIONARY_CACHE_MAX`] patterns.
struct DictionaryRegex {
    core: regex::Regex,
    full: Regex,
}
static DICTIONARY_RES: LazyLock<Mutex<HashMap<String, Option<Arc<DictionaryRegex>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const DICTIONARY_CACHE_MAX: usize = 4096;

fn dictionary_regex(core: String) -> Option<Arc<DictionaryRegex>> {
    let mut cache = DICTIONARY_RES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(r) = cache.get(&core) {
        return r.clone();
    }
    if cache.len() >= DICTIONARY_CACHE_MAX {
        cache.clear();
    }
    let full = Regex::new(&format!("(?i)(?<![{W}]){core}(?![{W}])")).ok();
    let r = full.and_then(|full| {
        let core = regex::Regex::new(&format!("(?i){core}")).ok()?;
        Some(Arc::new(DictionaryRegex { core, full }))
    });
    cache.insert(core, r.clone());
    r
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
            let Some(r) = dictionary_regex(words.join(r"[\s-]+")) else {
                continue;
            };
            // The core alone (no word-edge checks) is a DFA scan: no match there means no match at all.
            if r.core.is_match(&out) {
                out = r
                    .full
                    .replace_all(&out, fancy_regex::NoExpand(&d.term))
                    .into_owned();
            }
        }
    }
    out
}

/// On-device cleanup: spoken commands, fillers, stutters, spacing, capitalization, terminal punctuation (ADR 0101:
/// which repeats collapse, and no "." on a short dictation).
pub fn rule_cleanup(raw: &str, dict: &[DictionaryEntry]) -> String {
    let mut t = format!(" {raw} ");
    t = layout_command(&NEW_PARAGRAPH, &t, "\n\n");
    t = layout_command(&NEW_LINE, &t, "\n");
    let src = t.clone();
    t = FILLERS
        .replace_all(&src, |c: &Captures| {
            let m = c.get(0).expect("whole match");
            if keeps_filler(&c[2], &c[0], &src[..m.start()]) {
                c[0].to_owned()
            } else {
                c[1].to_owned()
            }
        })
        .into_owned();
    t = REPEATS
        .replace_all(&t, |c: &Captures| {
            if collapses(&c[1], &c[0]) {
                c[1].to_owned()
            } else {
                c[0].to_owned()
            }
        })
        .into_owned();
    t = rep(&TABS, &t, " ");
    t = rep(&NL_SPACES, &t, "\n");
    t = rep(&SPACE_BEFORE_PUNCT, &t, "${1}");
    t = rep(&SPACE_AFTER_PUNCT, &t, "${1} ");
    t = rep(&COMMAS, &t, ",");
    t = rep(&EDGES, &t, "");
    // Only for a recogniser that writes capitals: an all-lowercase text's "so. i can" is its own sentence end.
    if raw.chars().any(|c| c.is_ascii_uppercase()) {
        t = CONTINUED_DOT
            .replace_all(&t, |c: &Captures| {
                if is_abbreviation(&c[1]) {
                    c[0].to_owned()
                } else {
                    c[1].to_owned()
                }
            })
            .into_owned();
    }
    let src = t.clone();
    t = SENTENCE_START
        .replace_all(&src, |c: &Captures| {
            let start = c.get(0).map_or(0, |m| m.start());
            if c[1].starts_with('.') && ends_with_abbreviation(&src[..start]) {
                c[0].to_owned()
            } else {
                format!("{}{}", &c[1], c[2].to_uppercase())
            }
        })
        .into_owned();
    t = rep(&LONE_I, &t, "I");
    t = apply_dictionary(&t, dict);
    terminal_punctuation(t)
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
