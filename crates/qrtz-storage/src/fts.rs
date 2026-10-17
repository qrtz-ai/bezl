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
    static SPLIT: OnceLock<Regex> = OnceLock::new();
    static POSSESSIVE: OnceLock<Regex> = OnceLock::new();
    static WORDY: OnceLock<Regex> = OnceLock::new();
    let split = SPLIT.get_or_init(|| Regex::new(r"[^\p{L}\p{N}\p{M}'\x{2019}]+").unwrap());
    let possessive = POSSESSIVE.get_or_init(|| Regex::new(r"['\x{2019}]s$").unwrap());
    let wordy = WORDY.get_or_init(|| Regex::new(r"[\p{L}\p{N}]").unwrap());
    let n = norm(input);
    let words: Vec<String> = split
        .split(&n)
        .map(|w| possessive.replace(w, "").into_owned())
        .filter(|w| wordy.is_match(w))
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
