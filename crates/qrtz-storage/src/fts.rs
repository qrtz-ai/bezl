//! FTS5 query builders, ported from repo.ts. User input never reaches FTS syntax: every word becomes a quoted
//! prefix term. JS order is kept: `toLowerCase()` then `normalize('NFKC')`.

use std::sync::OnceLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

fn norm(input: &str) -> String {
    input.to_lowercase().nfkc().collect()
}

/// `toFtsQuery`: words split on `[^\p{L}\p{N}\p{M}'\u{2019}]+`, a trailing possessive `'s` dropped, words without a
/// letter or digit dropped, at most 8, AND-ed as `"w"*`. `None` when nothing searchable is left.
///
/// Apostrophes and combining marks stay inside the quoted term (N46, ADR 0038): FTS5 tokenizes a quoted string with
/// the index's tokenizer, so `don't` is the phrase `don t*` and an Indic or Thai word stays one phrase.
pub fn to_fts_query(input: &str) -> Option<String> {
    let words = search_words(input);
    if words.is_empty() {
        return None;
    }
    Some(prefix_terms(&words))
}

fn search_words(input: &str) -> Vec<String> {
    static SPLIT: OnceLock<Regex> = OnceLock::new();
    static POSSESSIVE: OnceLock<Regex> = OnceLock::new();
    static WORDY: OnceLock<Regex> = OnceLock::new();
    let split = SPLIT.get_or_init(|| Regex::new(r"[^\p{L}\p{N}\p{M}'\x{2019}]+").unwrap());
    let possessive = POSSESSIVE.get_or_init(|| Regex::new(r"['\x{2019}]s$").unwrap());
    let wordy = WORDY.get_or_init(|| Regex::new(r"[\p{L}\p{N}]").unwrap());
    let n = norm(input);
    split
        .split(&n)
        .map(|w| possessive.replace(w, "").into_owned())
        .filter(|w| wordy.is_match(w))
        .take(8)
        .collect()
}

fn prefix_terms(words: &[String]) -> String {
    words
        .iter()
        .map(|w| format!("\"{w}\"*"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Scripts written without spaces between words (Thai, Lao, Myanmar, Khmer, kana, Han): unicode61 indexes a run of
/// them as one token (N48). Fixed code point ranges, the same class as TS `UNSEGMENTED`, so both runtimes agree
/// whatever Unicode version each ships.
fn unsegmented(w: &str) -> bool {
    w.chars().any(|c| {
        matches!(c as u32,
            0x0E00..=0x0EFF
                | 0x1000..=0x109F
                | 0x1780..=0x17FF
                | 0x3005..=0x3007
                | 0x3040..=0x30FF
                | 0x31F0..=0x31FF
                | 0x3400..=0x4DBF
                | 0x4E00..=0x9FFF
                | 0xF900..=0xFAFF
                | 0x20000..=0x3134F)
    })
}

/// `SearchPlan` of repo.ts (N48, ADR 0039).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPlan {
    /// MATCH for the space-separated terms, or `None` when every term is a substring term.
    pub fts: Option<String>,
    /// Terms with a character from an unsegmented script, matched against the ASCII-lowercased text.
    pub substrings: Vec<String>,
}

/// `searchPlan`: `to_fts_query`'s words; a word with a Han, kana, Thai, Lao, Khmer or Myanmar character becomes a
/// substring term, the rest stay prefix terms. With no substring term, `fts` equals `to_fts_query(input)`.
pub fn search_plan(input: &str) -> Option<SearchPlan> {
    let words = search_words(input);
    if words.is_empty() {
        return None;
    }
    let (subs, rest): (Vec<String>, Vec<String>) = words.into_iter().partition(|w| unsegmented(w));
    Some(SearchPlan {
        fts: if rest.is_empty() {
            None
        } else {
            Some(prefix_terms(&rest))
        },
        substrings: subs,
    })
}

/// `substringSnippet`: up to 10 code points before the earliest term, the term in `[` `]`, up to 20 after, `…` where
/// the text was cut; the first 30 code points when no term occurs. Matching is on the ASCII-lowercased text, like
/// SQLite `lower()` in the substring SQL.
pub fn substring_snippet(text: &str, terms: &[String]) -> String {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let mut at: Option<usize> = None;
    let mut len = 0usize;
    for t in terms {
        let tc: Vec<char> = t.chars().collect();
        let mut i = 0usize;
        while i + tc.len() <= lower.len() {
            if matches!(at, Some(a) if i > a) {
                break;
            }
            if lower[i..i + tc.len()] == tc[..] {
                match at {
                    Some(a) if i >= a && tc.len() <= len => {}
                    _ => {
                        at = Some(i);
                        len = tc.len();
                    }
                }
                break;
            }
            i += 1;
        }
    }
    let Some(at) = at else {
        return if chars.len() > 30 {
            chars[..30].iter().collect::<String>() + "…"
        } else {
            text.to_string()
        };
    };
    let from = at.saturating_sub(10);
    let to = (at + len + 20).min(chars.len());
    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    out.extend(&chars[from..at]);
    out.push('[');
    out.extend(&chars[at..at + len]);
    out.push(']');
    out.extend(&chars[at + len..to]);
    if to < chars.len() {
        out.push('…');
    }
    out
}

/// The chat-retrieval query of `searchSegments`: words of 2+ letters/digits, at most 12, OR-ed as `"w"*`.
pub fn to_chat_fts_query(input: &str) -> Option<String> {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let word = WORD.get_or_init(|| Regex::new(r"[\p{L}\p{N}]{2,}").unwrap());
    let n = norm(input);
    let words: Vec<&str> = word.find_iter(&n).map(|m| m.as_str()).take(12).collect();
    if words.is_empty() {
        return None;
    }
    Some(
        words
            .iter()
            .map(|w| format!("\"{w}\"*"))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

/// At most this many bigrams per chat question (`CHAT_MAX_BIGRAMS`): each is a substring test per line in the scan.
pub const CHAT_MAX_BIGRAMS: usize = 16;

/// `bigrams`: character bigrams of a word from an unsegmented script, by code point (N49, ADR 0047). A two-character
/// word is its own bigram.
pub fn bigrams(word: &str) -> Vec<String> {
    let cs: Vec<char> = word.chars().collect();
    if cs.len() <= 2 {
        return vec![word.to_string()];
    }
    cs.windows(2).map(|w| w.iter().collect()).collect()
}

/// `ChatPlan` of repo.ts (N49, ADR 0047).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatPlan {
    /// OR-ed prefix terms for the other words, or `None` when there are none.
    pub fts: Option<String>,
    /// Distinct bigrams of the unsegmented words, at most [`CHAT_MAX_BIGRAMS`], matched as substrings.
    pub substrings: Vec<String>,
}

/// `chatPlan`: words of 2+ letters, digits or combining marks with a letter or digit, at most 12. A word with a Han,
/// kana, Thai, Lao, Khmer or Myanmar character becomes its bigrams; the others stay OR-ed `"w"*` terms. With neither
/// such a word nor a mark, `fts` equals the old `to_chat_fts_query(input)`.
pub fn chat_plan(input: &str) -> Option<ChatPlan> {
    static WORD: OnceLock<Regex> = OnceLock::new();
    static WORDY: OnceLock<Regex> = OnceLock::new();
    let word = WORD.get_or_init(|| Regex::new(r"[\p{L}\p{N}\p{M}]{2,}").unwrap());
    let wordy = WORDY.get_or_init(|| Regex::new(r"[\p{L}\p{N}]").unwrap());
    let n = norm(input);
    let words: Vec<&str> = word
        .find_iter(&n)
        .map(|m| m.as_str())
        .filter(|w| wordy.is_match(w))
        .take(12)
        .collect();
    if words.is_empty() {
        return None;
    }
    let (subs, rest): (Vec<&str>, Vec<&str>) = words.into_iter().partition(|w| unsegmented(w));
    let mut substrings: Vec<String> = Vec::new();
    for g in subs.iter().flat_map(|w| bigrams(w)) {
        if substrings.len() == CHAT_MAX_BIGRAMS {
            break;
        }
        if !substrings.contains(&g) {
            substrings.push(g);
        }
    }
    Some(ChatPlan {
        fts: if rest.is_empty() {
            None
        } else {
            Some(
                rest.iter()
                    .map(|w| format!("\"{w}\"*"))
                    .collect::<Vec<_>>()
                    .join(" OR "),
            )
        },
        substrings,
    })
}

/// JS `String.prototype.trim` (White_Space plus U+FEFF).
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}
