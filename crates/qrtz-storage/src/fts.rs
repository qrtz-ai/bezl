//! FTS5 query builders, ported from repo.ts. User input never reaches FTS syntax: every word becomes a quoted
//! prefix term. JS order is kept: `toLowerCase()` then `normalize('NFKC')`.

use std::sync::OnceLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

fn norm(input: &str) -> String {
    input.to_lowercase().nfkc().collect()
}

/// `toFtsQuery`: words split on `[^\p{L}\p{N}']+`, apostrophes dropped, at most 8, AND-ed as `"w"*`.
/// `None` when nothing searchable is left.
pub fn to_fts_query(input: &str) -> Option<String> {
    static SPLIT: OnceLock<Regex> = OnceLock::new();
    let split = SPLIT.get_or_init(|| Regex::new(r"[^\p{L}\p{N}']+").unwrap());
    let n = norm(input);
    let words: Vec<String> = split
        .split(&n)
        .map(|w| w.replace('\'', ""))
        .filter(|w| !w.is_empty())
        .take(8)
        .collect();
    if words.is_empty() {
        return None;
    }
    Some(
        words
            .iter()
            .map(|w| format!("\"{w}\"*"))
            .collect::<Vec<_>>()
            .join(" "),
    )
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

/// JS `String.prototype.trim` (White_Space plus U+FEFF).
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}
