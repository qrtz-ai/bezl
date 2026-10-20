//! The on-device cleanup of a dictation (`packages/core/src/talk/cleanup.ts`'s rules). There is no LLM cleanup:
//! it was removed for speed (ADR 0071), so the text goes in as soon as the final ASR text is in. The regexes are the JS ones with JS semantics spelled out: `\b` and `\w`
//! are ASCII in JS, so they become explicit ASCII lookarounds/classes here. Checked byte for byte against TS by
//! spec/vectors/talk/cleanup.json. ADR 0199: the patterns with look-arounds or a backreference run as hand-written
//! scans of the same patterns (same text out, ~14x faster chain).
use crate::contract::{CleanupMethod, DictionaryEntry};
use fancy_regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

const W: &str = "A-Za-z0-9_";

fn re(s: &str) -> Regex {
    Regex::new(s).expect("static regex")
}

// ADR 0199: the steps whose patterns had look-arounds or a backreference (spoken layout commands, fillers, repeats,
// the continued dot, sentence starts, the lone "i") are hand-written scans of the same patterns; each one's doc
// comment gives the pattern it implements, and so does a dictionary term of plain words ([`replace_plain_term`]).
// fancy-regex ran those in its backtracking VM from every position (~1.5 ms of the 1.6 ms chain on 495 words; 0.11 ms
// now). Same text out, byte for byte: `tests/hardening60.rs` checks the chain against the frozen cycle-59 one, and the
// unit tests at the end check each scan against its old pattern, on generated text.
// `\s` there is Unicode White_Space, which is `char::is_whitespace` and `str::trim_*`; `(?i)` on these letters
// folds ASCII only (no other code point folds to n, e, w, l, i, p, a, g, h, u, m or r), but it makes `[A-Za-z0-9_]`
// take "ſ" (U+017F) and the Kelvin sign (U+212A) too ([`is_w_folded`]).

/// ADR 0101: an immediate repeat of a whole word, as the regex it was. ADR 0199: used only for a text with "ſ" or
/// the Kelvin sign in it, where fancy-regex's case-insensitive backreference has its own rules
/// ([`collapse_repeats`]); the recogniser never writes either.
static REPEATS: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"(?i)(?<![{W}'])([{W}]+)(?:[\s,]+\1(?![{W}']))+")));
static NL_SPACES: LazyLock<Regex> = LazyLock::new(|| re(r" *\n *"));
static SPACE_BEFORE_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"\s+([,.!?;:])"));
// ADR 0101 (N160): only a comma or semicolon gets a space after it. A dot, colon, "!" or "?" with no space after it
// is part of a token the recogniser wrote that way ("qrtz.ai", "config.json", "p.m.", "...", "https://").
static SPACE_AFTER_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r#"([,;])(?=[^\s0-9"')\]])"#));
static COMMAS: LazyLock<Regex> = LazyLock::new(|| re(r",+"));
static EDGES: LazyLock<Regex> = LazyLock::new(|| re(r"^[\s,]+|[\s,]+$"));
static ENDS_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"[\p{L}\p{N}]$"));
/// ADR 0199: an ASCII word character (JS `\w`).
fn is_w(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// ADR 0199: a word character as `(?i)[A-Za-z0-9_]` sees it: also "ſ" and "K" (the Kelvin sign), which fold to
/// "s" and "k".
fn is_w_folded(c: char) -> bool {
    is_w(c) || c == '\u{17F}' || c == '\u{212A}'
}

/// The character before byte `at`.
fn char_before(s: &str, at: usize) -> Option<char> {
    s[..at].chars().next_back()
}

/// The byte length of the run of `keep` characters at the start of `s`.
fn run_len(s: &str, keep: impl Fn(char) -> bool) -> usize {
    s.find(|c: char| !keep(c)).unwrap_or(s.len())
}

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

/// ADR 0205: the subject pronouns whose repeat across a comma collapses only in the one unambiguous form, "<P>, <p>
/// <modal>" where the first <P> opens the sentence, on its own or after one opener ("So we, we can ship it" → "So we
/// can ship it"). Same lists in cleanup.ts.
const COMMA_SUBJECTS: &[&str] = &["we", "they"];
/// ADR 0205 / ADR 0207: the modals after the second pronoun (lowercase, whole word). Plain forms, their
/// negated contractions (ASCII apostrophe; curly U+2019 is folded to ASCII before the lookup), and "cannot".
/// Not "shan't" (thin evidence) and not "cant" without an apostrophe.
const SUBJECT_MODALS: &[&str] = &[
    "can",
    "could",
    "will",
    "would",
    "should",
    "might",
    "must",
    "may",
    "shall",
    "can't",
    "couldn't",
    "won't",
    "wouldn't",
    "shouldn't",
    "mustn't",
    "mightn't",
    "mayn't",
    "cannot",
];
/// ADR 0205: the one opener allowed before the first pronoun, itself at the start of a sentence (an optional comma
/// after it): "So we, we can", "And they, they will". Not "You and we, we can" (a compound subject).
const SUBJECT_OPENERS: &[&str] = &["so", "and", "but", "then", "because"];

/// ADR 0205: does `pre` (the text before a word) end at the start of a sentence: nothing, a line break, or ". ", "!
/// ", "? " before it (after the leading space rule_cleanup adds)?
fn at_sentence_start(pre: &str) -> bool {
    let blank = pre.trim_end_matches([' ', '\t']);
    blank.trim().is_empty() || blank.ends_with(['\n', '.', '!', '?'])
}

/// ADR 0205 / ADR 0207: is `s[p..x]` (a repeat whose word ends at `word_end`) a subject stutter "we, we" / "they,
/// they" that opens its sentence (alone or after one [`SUBJECT_OPENERS`] word) and is followed by a modal (plain or
/// negated, or "cannot")? The repeat is exactly one ", " and the second pronoun is lowercase. A curly apostrophe in
/// the modal is folded to ASCII for the list lookup so both apostrophe forms match the same entry.
fn subject_stutter(s: &str, p: usize, word_end: usize, x: usize) -> bool {
    let word = s[p..word_end].to_ascii_lowercase();
    if !COMMA_SUBJECTS.contains(&word.as_str()) || s[word_end..x] != format!(", {word}") {
        return false;
    }
    let Some(after) = s[x..].strip_prefix(' ') else {
        return false;
    };
    // Letters plus an ASCII or curly apostrophe, so contractions and "cannot" are one word.
    let len = run_len(after, |c| c.is_ascii_lowercase() || c == '\'' || c == '\u{2019}');
    let modal = after[..len].replace('\u{2019}', "'");
    if !SUBJECT_MODALS.contains(&modal.as_str())
        || after[len..].starts_with(|c: char| is_w_folded(c) || c == '\'' || c == '\u{2019}' || !c.is_ascii())
    {
        return false;
    }
    let pre = s[..p].trim_end_matches([' ', '\t']);
    if at_sentence_start(pre) {
        return true;
    }
    let pre = pre.strip_suffix(',').unwrap_or(pre);
    let opener_len = pre.chars().rev().take_while(char::is_ascii_alphabetic).count();
    let (rest, opener) = pre.split_at(pre.len() - opener_len);
    SUBJECT_OPENERS.contains(&opener.to_ascii_lowercase().as_str())
        && !rest.ends_with(|c: char| is_w_folded(c) || c == '\'' || !c.is_ascii())
        && rest.ends_with([' ', '\t', '\n'])
        && at_sentence_start(rest)
}

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

/// The spoken layout commands, in lowercase.
const NEW_PARAGRAPH: &str = "new paragraph";
const NEW_LINE: &str = "new line";

/// A spoken layout command (`phrase`: [`NEW_PARAGRAPH`], [`NEW_LINE`]) becomes `with`, unless it is a noun phrase
/// (ADR 0182). ADR 0199: a scan for the pattern `(?i)\s*(?<![\w])<phrase>(?![\w])[,.]?\s*`, every match replaced
/// left to right. A match is the first case-insensitive occurrence with no word character on either side; it takes
/// the whitespace before it (not reaching into the previous match), then one optional "," or "." and the whitespace
/// after it.
fn layout_command(phrase: &str, s: &str, with: &str) -> String {
    let lower = s.to_ascii_lowercase();
    if !lower.contains(phrase) {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let (mut last, mut from) = (0, 0);
    while let Some(off) = lower[from..].find(phrase) {
        let at = from + off;
        let end = at + phrase.len();
        from = at + 1;
        if char_before(s, at).is_some_and(is_w_folded) || s[end..].chars().next().is_some_and(is_w_folded) {
            continue;
        }
        let start = last + s[last..at].trim_end().len();
        let mut stop = end;
        if s[stop..].starts_with([',', '.']) {
            stop += 1;
        }
        stop += run_len(&s[stop..], char::is_whitespace);
        out.push_str(&s[last..start]);
        if ends_with_determiner(s[..start].trim_end()) {
            out.push_str(&s[start..stop]);
        } else {
            out.push_str(with);
        }
        last = stop;
        from = stop;
    }
    out.push_str(&s[last..]);
    out
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

/// ADR 0192: a filler the recogniser set off with a comma ("should, uh, ship") leaves that comma behind when it goes.
/// It goes with the filler only after a word a comma can't follow in written English. Same lists in cleanup.ts.
/// - [`FILLER_COMMA_TIGHT`]: determiners and the conjunctions "and", "but", "or", "nor", "because" ("the, um, design
///   team", "and, uh, we think"): nothing after them takes a comma but a parenthetical, and the filler was the only
///   one.
/// - [`FILLER_COMMA_VERBS`]: a copula or modal with its subject before it in a plain main clause ("The demo is, um,
///   basically done", "we should, uh, ship"), and the subject pronoun of a question's first two words ("Can you, um,
///   send"); only when the next word is a plain lowercase word that can't open a new clause or be a vocative
///   ([`FILLER_COMMA_NEXT_REFUSE`]: pronouns, determiners, conjunctions, auxiliaries, discourse words, endearments; a
///   capital, digit or apostrophe refuses too), and the sentence before has no word that makes the verb end a clause
///   ([`FILLER_COMMA_CLAUSE_OPENERS`]: "if", "when", "what", "that", "than", ...) and doesn't open with a reply word
///   ([`FILLER_COMMA_REPLY_STARTS`]: "Yes we can, um, Sam").
pub const FILLER_COMMA_TIGHT: &[&str] = &[
    "the", "a", "an", "my", "your", "our", "their", "its", "every", "and", "but", "or", "nor", "because",
];
pub const FILLER_COMMA_VERBS: &[&str] = &[
    "is", "are", "was", "were", "am", "can", "could", "should", "would", "will", "must", "might",
];
/// ADR 0192: a question's first word before its subject pronoun ("Can you, um, send me the deck?").
pub const FILLER_COMMA_QUESTION_AUX: &[&str] = &[
    "can", "could", "will", "would", "should", "shall", "may", "might", "do", "does", "did", "is", "are",
    "was", "were", "have", "has",
];
pub const FILLER_COMMA_PRONOUNS: &[&str] = &["i", "you", "we", "they", "he", "she", "it"];
pub const FILLER_COMMA_CLAUSE_OPENERS: &[&str] = &[
    "if",
    "when",
    "whenever",
    "while",
    "because",
    "since",
    "as",
    "than",
    "unless",
    "until",
    "till",
    "though",
    "although",
    "whether",
    "once",
    "before",
    "after",
    "where",
    "wherever",
    "what",
    "whatever",
    "how",
    "however",
    "who",
    "whoever",
    "which",
    "whichever",
    "why",
    "that",
];
pub const FILLER_COMMA_REPLY_STARTS: &[&str] = &[
    "yes", "yeah", "yep", "no", "nope", "sure", "okay", "ok", "here", "there", "well", "oh",
];
pub const FILLER_COMMA_NEXT_REFUSE: &[&str] = &[
    // pronouns
    "i",
    "you",
    "we",
    "they",
    "he",
    "she",
    "it",
    "this",
    "that",
    "these",
    "those",
    "there",
    "here",
    "me",
    "us",
    "them",
    "him",
    "her",
    "nobody",
    "everyone",
    "everybody",
    "someone",
    "somebody",
    "anyone",
    "anybody",
    "nothing",
    "everything",
    "something",
    "anything",
    "none", // determiners
    "the",
    "a",
    "an",
    "my",
    "your",
    "our",
    "their",
    "his",
    "its",
    "some",
    "any",
    "every",
    "each",
    "no",
    "all",
    "both",
    "either",
    "neither", // conjunctions and clause openers
    "and",
    "but",
    "or",
    "nor",
    "so",
    "yet",
    "because",
    "if",
    "when",
    "while",
    "then",
    "though",
    "although",
    "unless",
    "since",
    "as",
    "which",
    "who",
    "whom",
    "whose",
    "what",
    "where",
    "why",
    "how",
    "whether",
    "than",
    "until",
    "once",
    "after",
    "before", // auxiliaries
    "do",
    "does",
    "did",
    "is",
    "are",
    "was",
    "were",
    "am",
    "be",
    "can",
    "could",
    "will",
    "would",
    "should",
    "shall",
    "may",
    "might",
    "must",
    "have",
    "has",
    "had", // discourse and sequence words
    "well",
    "okay",
    "ok",
    "yes",
    "yeah",
    "yep",
    "nope",
    "sure",
    "right",
    "like",
    "first",
    "second",
    "third",
    "next",
    "finally",
    "also",
    "plus",
    "again",
    "now",
    "anyway",
    "please",
    "thanks",
    "sorry",
    "oh",
    "too",
    // endearments and vocatives
    "honey",
    "babe",
    "baby",
    "dude",
    "man",
    "guys",
    "buddy",
    "sir",
    "bro",
    "mate",
    "folks",
    "sweetie",
    "dear",
    "love",
    "friend",
    "team",
    "boss",
];

/// The whole word is one [`FILLERS`] filler (u+m+, u+h+, e+r+m+, e+r, a+h+, h+m+, m+h*m+), any case. By hand, not
/// a regex: it runs on the words around every filler that had a comma.
fn is_filler_word(w: &str) -> bool {
    let (mut runs, mut len) = ([(0u8, 0usize); 3], 0);
    for b in w.bytes() {
        let b = b.to_ascii_lowercase();
        if len > 0 && runs[len - 1].0 == b {
            runs[len - 1].1 += 1;
        } else if len == 3 {
            return false;
        } else {
            runs[len] = (b, 1);
            len += 1;
        }
    }
    match &runs[..len] {
        [(b'u', _), (b'm' | b'h', _)] | [(b'a', _), (b'h', _)] | [(b'h', _), (b'm', _)] => true,
        [(b'e', _), (b'r', 1)] | [(b'e', _), (b'r', _), (b'm', _)] => true,
        [(b'm', n)] => *n >= 2,
        [(b'm', _), (b'h', _), (b'm', _)] => true,
        _ => false,
    }
}

/// ADR 0192: does the comma before a removed filler go with it? `before` is the text up to that comma, `after` the
/// text after the filler (and its own comma, if any).
fn filler_takes_comma(before: &str, after: &str) -> bool {
    let prev = before.rsplit([' ', '\n']).next().unwrap_or_default();
    if prev.is_empty() || !prev.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    // The next word: on the same line, a letter first, past any more lowercase fillers ("is, um, uh, basically").
    let mut rest = after.trim_start_matches(' ');
    let next = loop {
        if !rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
            return false;
        }
        let tok = rest.split([' ', '\n']).next().unwrap_or_default();
        let word = tok.trim_end_matches([',', '.', '!', '?', ';', ':']);
        let filler = word.bytes().all(|b| b.is_ascii_lowercase())
            && is_filler_word(word)
            && tok.len() - word.len() <= 1
            && tok[word.len()..].bytes().all(|b| b == b',');
        if !filler {
            break word;
        }
        rest = rest[tok.len()..].trim_start_matches(' ');
    };
    // "a" only in lowercase: "Plan A, um," / "Vitamin A, um," are the letter.
    let low = prev.to_ascii_lowercase();
    if prev != "A" && FILLER_COMMA_TIGHT.contains(&low.as_str()) {
        return true;
    }
    let pronoun = FILLER_COMMA_PRONOUNS.contains(&low.as_str());
    if !pronoun && !FILLER_COMMA_VERBS.contains(&low.as_str()) {
        return false;
    }
    if !next.bytes().all(|b| b.is_ascii_lowercase())
        || FILLER_COMMA_NEXT_REFUSE.contains(&next)
        || is_filler_word(next)
    {
        return false;
    }
    // The sentence before the word: after the nearest line break or ". ", "! ", "? ", fillers left out.
    let head = &before[..before.len() - prev.len()];
    let bytes = head.as_bytes();
    let mut start = 0;
    for j in (1..bytes.len()).rev() {
        if bytes[j - 1] == b'\n' || (matches!(bytes[j - 1], b'.' | b'!' | b'?') && bytes[j] == b' ') {
            start = j;
            break;
        }
    }
    let words: Vec<String> = head[start..]
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '\'')
                .to_ascii_lowercase()
        })
        .filter(|w| !w.is_empty() && !is_filler_word(w))
        .collect();
    if pronoun {
        // "Can you, um, send": the pronoun is the second word of a question.
        return words.len() == 1 && FILLER_COMMA_QUESTION_AUX.contains(&words[0].as_str());
    }
    !words.is_empty()
        && !FILLER_COMMA_REPLY_STARTS.contains(&words[0].as_str())
        && !words
            .iter()
            .any(|w| FILLER_COMMA_CLAUSE_OPENERS.contains(&w.as_str()))
}

/// ADR 0186: words that, right after "Scratch that," or "Scratch that.", make it a literal request ("Scratch that,
/// please.", "Scratch that, and you might win."), not the correction. Same list in cleanup.ts.
pub const SCRATCH_LITERAL_NEXT: &[&str] = &[
    "please", "would", "will", "can", "could", "and", "or", "but", "too", "again", "now", "for", "gently",
    "here", "there", "okay", "ok",
];

/// Is the sentence mark that ends `p` (".", "!" or "?") an unambiguous sentence end? The word before it is plain
/// ASCII letters or digits (an apostrophe inside is fine), at least two of them, with no dot of its own ("today.",
/// "don't." yes; "p.m.", "qrtz.ai.", "Wait...", "Mr.", "F.", "5." no: a dotted token, an abbreviation, an initial or a
/// lone digit can't be told from a sentence end with certainty).
fn clean_sentence_end(p: &str) -> bool {
    let Some(mark) = p.chars().last() else {
        return false;
    };
    if !matches!(mark, '.' | '!' | '?') {
        return false;
    }
    let body = &p[..p.len() - 1];
    let token = body.rsplit([' ', '\n']).next().unwrap_or_default();
    if token.len() < 2 || !token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'\'') {
        return false;
    }
    if !token.ends_with(|c: char| c.is_ascii_alphanumeric()) {
        return false;
    }
    !(mark == '.' && ends_with_abbreviation(body))
}

/// ADR 0186: the explicit self-correction "Scratch that". Only its unambiguous form: its own sentence, right after
/// a clean sentence end (see [`clean_sentence_end`]), followed by "," or "." and the words that replace it. The
/// sentence before it (back to the previous clean sentence end or line break) goes, with the command: "Let's meet on
/// Tuesday. Scratch that, let's meet on Wednesday." → "Let's meet on Wednesday." Left exactly as spoken when anything
/// is unclear: mid-sentence ("scratch that off my list", "don't scratch that"), no punctuation after it ("Scratch
/// that itch"), nothing before or after it, a literal request ([`SCRATCH_LITERAL_NEXT`]), or a dotted word,
/// abbreviation or initial anywhere at the sentence edges.
fn scratch_that(t: String) -> String {
    const CMD: &str = "scratch that";
    let mut t = t;
    // One lowercase copy, redone only after a rewrite (rare): a dictation with no "Scratch that" pays one scan.
    let mut lower = t.to_ascii_lowercase();
    let mut from = 0;
    while let Some(off) = lower[from..].find(CMD) {
        let at = from + off;
        from = at + CMD.len();
        if let Some(out) = scratch_at(&t, at) {
            t = out;
            lower = t.to_ascii_lowercase();
            from = 0;
        }
    }
    t
}

/// The text with the "Scratch that" at byte `at` applied, or `None` to leave it as spoken.
fn scratch_at(t: &str, at: usize) -> Option<String> {
    let before = &t[..at];
    let p = before.trim_end_matches(' ');
    if p.len() == before.len() || !clean_sentence_end(p) {
        return None;
    }
    let after = &t[at + "scratch that".len()..];
    let after = after.strip_prefix(',').or_else(|| after.strip_prefix('.'))?;
    let rest = after.trim_start_matches(' ');
    if rest.len() == after.len() || !rest.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return None;
    }
    let next: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    if SCRATCH_LITERAL_NEXT.contains(&next.as_str()) {
        return None;
    }
    // The start of the sentence being scratched: after the nearest line break or clean sentence end before it.
    let body = &p[..p.len() - 1];
    let bytes = body.as_bytes();
    let mut start = 0;
    for j in (1..=bytes.len()).rev() {
        let c = bytes[j - 1];
        if c == b'\n' {
            start = j;
            break;
        }
        if matches!(c, b'.' | b'!' | b'?') && bytes.get(j) == Some(&b' ') {
            if !clean_sentence_end(&body[..j]) {
                return None;
            }
            start = j + body[j..].len() - body[j..].trim_start_matches(' ').len();
            break;
        }
    }
    Some(format!("{}{rest}", &t[..start]))
}

/// ADR 0189: the spoken marks a dictation can end with, and what they become.
const FINAL_MARKS: &[(&str, char)] = &[
    ("question mark", '?'),
    ("exclamation point", '!'),
    ("exclamation mark", '!'),
];

/// ADR 0189: a sentence that ends in a spoken "question mark" must start with one of these (a question by its form),
/// so "That's still a big question mark" stays. Same list in cleanup.ts.
pub const QUESTION_STARTS: &[&str] = &[
    "is",
    "are",
    "was",
    "were",
    "am",
    "do",
    "does",
    "did",
    "can",
    "could",
    "will",
    "would",
    "should",
    "shall",
    "may",
    "might",
    "must",
    "have",
    "has",
    "had",
    "what",
    "what's",
    "when",
    "when's",
    "where",
    "where's",
    "why",
    "who",
    "who's",
    "whom",
    "whose",
    "which",
    "how",
    "how's",
    "isn't",
    "aren't",
    "wasn't",
    "weren't",
    "don't",
    "doesn't",
    "didn't",
    "can't",
    "couldn't",
    "won't",
    "wouldn't",
    "shouldn't",
    "haven't",
    "hasn't",
];

/// ADR 0189: right before a spoken mark, these make it the noun ("Is there a question mark", "Which key is question
/// mark", "How do I type question mark", "Shift plus one types exclamation point"), even with the recogniser
/// having dropped an article ("Does the title need question mark"): determiners and demonstratives, prepositions,
/// conjunctions, copulas, and verbs that take the symbol as an object. Same list in cleanup.ts.
pub const MARK_NOUN_BEFORE: &[&str] = &[
    // determiners, demonstratives, possessives, quantifiers
    "a", "an", "the", "this", "that", "these", "those", "my", "your", "his", "its", "our", "their", "no",
    "another", "any", "each", "every", "some", "one", "which", "whose", "what", "same", "single", "extra",
    "double", "final", "last", "first", "big", "little", "small", "huge", "giant", "lone", "stray",
    "missing", "trailing", "closing", "inverted", // prepositions and conjunctions
    "with", "without", "of", "for", "to", "from", "by", "after", "before", "than", "like", "as", "about",
    "into", "onto", "at", "plus", "minus", "and", "or", "nor", "versus", "vs", "via", "per", "between",
    // copulas and naming
    "is", "are", "was", "were", "be", "been", "being", "isn't", "wasn't", "it's", "that's", "what's",
    "there's", "here's", "become", "becomes", "became", "call", "called", "name", "named", "label",
    "labeled", "labelled", "mean", "means", "meant", "spell", "spelled",
    // verbs that take the symbol as an object
    "say", "says", "said", "type", "types", "typed", "typing", "press", "presses", "pressed", "pressing",
    "hit", "hits", "put", "puts", "putting", "add", "adds", "added", "adding", "use", "uses", "used",
    "using", "insert", "inserts", "inserted", "remove", "removes", "removed", "delete", "deletes", "deleted",
    "need", "needs", "needed", "want", "wants", "wanted", "have", "has", "had", "having", "get", "gets",
    "got", "give", "gives", "gave", "make", "makes", "made", "include", "includes", "included", "keep",
    "keeps", "kept", "drop", "drops", "dropped", "lose", "loses", "lost", "miss", "misses", "missed", "see",
    "sees", "saw", "seen", "show", "shows", "showed", "showing", "print", "prints", "printed", "display",
    "displays", "return", "returns", "returned", "write", "writes", "wrote", "end", "ends", "ended",
    "ending", "likes", "love", "loves", "hate", "hates", "prefer", "prefers", "try", "tried", "place",
    "append", "follow", "follows", "followed", "deserve", "deserves", "deserved", "forget", "forgets",
    "forgot", "leave", "left", "change", "changed", "replace", "replaced", "find", "found", "click", "tap",
    "select", "produce", "produces", "draw", "drew", "read", "reads", "count", "overuse", "overuses",
];

/// ADR 0189: "there" / "here" right after one of these, before a spoken mark, is "is there [a] question mark".
const COPULAS: &[&str] = &["is", "are", "was", "were", "isn't", "aren't", "wasn't", "weren't"];

/// ADR 0189: an article, possessive or quantifier this many words or fewer before a spoken mark makes it the noun
/// ("Is it a big question mark", "Never end it with an emphatic exclamation point").
const MARK_DETERMINER_WINDOW: usize = 3;
const MARK_DETERMINERS: &[&str] = &[
    "a", "an", "the", "my", "your", "his", "its", "our", "their", "no", "another", "every", "each", "any",
    "some", "one",
];

/// ADR 0189: a spoken "question mark" / "exclamation point" / "exclamation mark" that ends the dictation becomes "?" /
/// "!". Only its unambiguous form: written in lowercase, the very last words (a "." "?" or "!" the recogniser put
/// after them aside), after one space and a plain word (ASCII letters, digits, an apostrophe; not a bare number; no
/// comma, no dot, no sentence end before it), that word not one of [`MARK_NOUN_BEFORE`], no [`MARK_DETERMINERS`] word
/// in the last [`MARK_DETERMINER_WINDOW`] words, and for "question mark" a sentence that starts like a question
/// ([`QUESTION_STARTS`]). "Is the meeting still on question mark" → "Is the meeting still on?"; "Put a question mark",
/// "Which key is question mark", "The band was called Question Mark" stay.
fn final_mark(t: String) -> String {
    let s = t.strip_suffix(['.', '?', '!']).unwrap_or(&t);
    for &(phrase, mark) in FINAL_MARKS {
        let Some(before) = s.strip_suffix(phrase) else {
            continue;
        };
        let Some(body) = before.strip_suffix(' ') else {
            return t;
        };
        if !body.ends_with(|c: char| c.is_ascii_alphanumeric()) {
            return t;
        }
        // The sentence: after the nearest line break or ". ", "! ", "? " before it.
        let bytes = body.as_bytes();
        let mut start = 0;
        for j in (1..bytes.len()).rev() {
            if bytes[j - 1] == b'\n' || (matches!(bytes[j - 1], b'.' | b'!' | b'?') && bytes[j] == b' ') {
                start = j;
                break;
            }
        }
        let words: Vec<String> = body[start..]
            .split_whitespace()
            .map(|w| w.trim_end_matches(',').to_ascii_lowercase())
            .collect();
        let Some(last) = words.last() else {
            return t;
        };
        // A plain word: not a number ("Shift 1 exclamation point", "error 404 question mark" name a key or code).
        if !last.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'\'')
            || last.bytes().all(|b| b.is_ascii_digit())
            || MARK_NOUN_BEFORE.contains(&last.as_str())
        {
            return t;
        }
        // "Is there question mark" (the noun, its article dropped), not "Are you there question mark".
        let prev = words.len().checked_sub(2).map(|i| words[i].as_str());
        if matches!(last.as_str(), "there" | "here") && prev.is_some_and(|p| COPULAS.contains(&p)) {
            return t;
        }
        let window = &words[words.len().saturating_sub(MARK_DETERMINER_WINDOW)..];
        if window.iter().any(|w| MARK_DETERMINERS.contains(&w.as_str())) {
            return t;
        }
        if mark == '?' && !QUESTION_STARTS.contains(&words[0].as_str()) {
            return t;
        }
        return format!("{body}{mark}");
    }
    t
}

/// ADR 0193: number words, for spoken amounts. Values 1-9, 10-19 and the tens; "hundred" and "thousand" scale.
const NUMBER_UNITS: &[&str] = &[
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];
const NUMBER_TEENS: &[&str] = &[
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const NUMBER_TENS: &[&str] = &[
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
/// ADR 0193: any of these (or a digit) near a spoken amount makes it part of a range, a decimal, a sum or a bigger
/// number ("between ten and twenty dollars", "two point five percent", "five dollars and fifty cents", "two million
/// dollars"): the words stay as spoken. Same list in cleanup.ts.
pub const AMOUNT_NEAR_REFUSE: &[&str] = &[
    "hundred", "thousand", "million", "billion", "trillion", "dozen", "point", "minus", "negative", "plus",
    "dot", "cent", "cents", "oh",
];
/// ADR 0193: how many words on each side of a spoken amount are checked against [`AMOUNT_NEAR_REFUSE`].
const AMOUNT_WINDOW: usize = 3;

fn unit_value(w: &str) -> Option<u32> {
    NUMBER_UNITS.iter().position(|u| *u == w).map(|i| i as u32)
}

/// 1-99 from `w[i..]`: "seven", "fifteen", "forty", "forty two". The value and the words used.
fn under_100(w: &[&str], i: usize) -> Option<(u32, usize)> {
    let first = *w.get(i)?;
    if let Some(t) = NUMBER_TEENS.iter().position(|t| *t == first) {
        return Some((10 + t as u32, 1));
    }
    if let Some(t) = NUMBER_TENS.iter().position(|t| *t == first) {
        let tens = 20 + 10 * t as u32;
        return match w.get(i + 1).and_then(|n| unit_value(n)) {
            Some(u) if u > 0 => Some((tens + u, 2)),
            _ => Some((tens, 1)),
        };
    }
    unit_value(first).filter(|u| *u > 0).map(|u| (u, 1))
}

/// 1-999: "seven", "forty two", "three hundred", "three hundred and five", "three hundred twenty".
fn under_1000(w: &[&str], i: usize) -> Option<(u32, usize)> {
    if let (Some(u), Some(&"hundred")) = (w.get(i).and_then(|n| unit_value(n)), w.get(i + 1)) {
        if u == 0 {
            return None;
        }
        let mut used = 2;
        if w.get(i + used) == Some(&"and") {
            let (v, n) = under_100(w, i + used + 1)?;
            return Some((u * 100 + v, used + 1 + n));
        }
        if let Some((v, n)) = under_100(w, i + used) {
            used += n;
            return Some((u * 100 + v, used));
        }
        return Some((u * 100, used));
    }
    under_100(w, i)
}

/// ADR 0193: the whole run of words as one number, 0-999,999, or `None` when any word is left over ("nineteen
/// ninety nine", "fifteen hundred", "a hundred" and "two five" are not one number by these rules).
fn spoken_number(w: &[&str]) -> Option<u32> {
    if w == ["zero"] {
        return Some(0);
    }
    let (a, mut i) = under_1000(w, 0)?;
    if i == w.len() {
        return Some(a);
    }
    if w[i] != "thousand" {
        return None;
    }
    i += 1;
    if i == w.len() {
        return Some(a * 1000);
    }
    if w[i] == "and" {
        i += 1;
    }
    let (b, n) = under_1000(w, i)?;
    (i + n == w.len()).then_some(a * 1000 + b)
}

fn is_number_word(w: &str) -> bool {
    unit_value(w).is_some() || NUMBER_TEENS.contains(&w) || NUMBER_TENS.contains(&w)
}

/// "2500" → "2,500".
fn grouped(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A word as the amount rules see it: punctuation off its edges, lowercase.
fn bare(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_ascii_lowercase()
}

/// ADR 0193: spoken amounts in their unambiguous forms, written the way they are read: "twenty five dollars" →
/// "$25", "fifteen percent" / "15 percent" → "15%", "three thirty p.m." → "3:30 p.m.". The run of number words right
/// before the lowercase unit ("dollars", "percent", "a.m.", "p.m.") must be one number by [`spoken_number`] (a clock
/// time for "a.m." / "p.m.": hour 1-12 and "oh five" / 10-59 minutes), all in lowercase (so never at a sentence
/// start: "Twenty five dollars is a lot" stays), on one line, with no digit, number word or [`AMOUNT_NEAR_REFUSE`] word within [`AMOUNT_WINDOW`] words on either
/// side. "dollars" takes words only ("2025 dollars" is a year's dollars), never the singular ("twenty five dollar
/// bills" are fives). Runs last, after the terminal punctuation (whose word counts are the spoken words). Anything
/// else stays as spoken: "a hundred percent", "a couple of dollars", "one or two dollars", "Ten Percent Happier".
fn spoken_amounts(t: String) -> String {
    if !(t.contains("dollars") || t.contains("percent") || t.contains(".m.")) {
        return t;
    }
    // Word spans on the text, and the line each is on.
    let mut toks: Vec<(usize, usize, usize)> = Vec::new();
    let (mut line, mut start) = (0, None);
    for (i, c) in t.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                toks.push((s, i, line));
            }
            if c == '\n' {
                line += 1;
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        toks.push((s, t.len(), line));
    }
    let word = |k: usize| &t[toks[k].0..toks[k].1];
    let near_number = |k: usize| {
        let b = bare(word(k));
        b.bytes().any(|c| c.is_ascii_digit())
            || is_number_word(&b)
            || AMOUNT_NEAR_REFUSE.contains(&b.as_str())
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for k in 0..toks.len() {
        let w = word(k);
        let (unit, tail) = if let Some(r) = w.strip_prefix("dollars") {
            ("$", r)
        } else if let Some(r) = w.strip_prefix("percent") {
            ("%", r)
        } else if let Some(r) = w.strip_prefix("a.m.").or_else(|| w.strip_prefix("p.m.")) {
            (&w[..4], r)
        } else {
            continue;
        };
        if !tail
            .bytes()
            .all(|b| matches!(b, b'.' | b',' | b'!' | b'?' | b';' | b':'))
        {
            continue;
        }
        // The run of plain words before the unit, on its line.
        let mut j = k;
        while j > 0 && toks[j - 1].2 == toks[k].2 {
            let p = word(j - 1);
            let plain = p.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
                || (unit == "%" && j == k && is_decimal(p));
            let low = p.to_ascii_lowercase();
            let numberish = low
                .split('-')
                .all(|q| is_number_word(q) || q == "hundred" || q == "thousand" || q == "and" || q == "oh")
                || (unit == "%" && j == k && is_decimal(p));
            if !plain || !numberish {
                break;
            }
            j -= 1;
        }
        // A leading "and" is the sentence's ("salt and fifteen percent"), not the number's.
        while j < k && word(j).eq_ignore_ascii_case("and") {
            j += 1;
        }
        if j == k {
            continue;
        }
        // A capitalized number word before the run: the sentence or a title starts there ("Twenty five dollars is
        // a lot", "Ten Percent Happier"); a sentence doesn't start with a figure, so the words stay.
        if j > 0
            && toks[j - 1].2 == toks[j].2
            && word(j - 1).starts_with(|c: char| c.is_ascii_uppercase())
            && is_number_word(&bare(word(j - 1)))
        {
            continue;
        }
        let before = j.saturating_sub(AMOUNT_WINDOW)..j;
        let after = k + 1..(k + 1 + AMOUNT_WINDOW).min(toks.len());
        if before.chain(after).any(near_number) {
            continue;
        }
        let run: Vec<String> = (j..k)
            .flat_map(|q| {
                word(q)
                    .to_ascii_lowercase()
                    .split('-')
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        let run: Vec<&str> = run.iter().map(String::as_str).collect();
        let text = match unit {
            "$" => spoken_number(&run).map(|n| format!("${}", grouped(n))),
            "%" if run.len() == 1 && is_decimal(run[0]) => Some(format!("{}%", run[0])),
            "%" => spoken_number(&run).map(|n| format!("{}%", grouped(n))),
            m => spoken_clock(&run).map(|c| format!("{c} {m}")),
        };
        if let Some(text) = text {
            edits.push((toks[j].0, toks[k].1 - tail.len(), text));
        }
    }
    let mut out = t;
    for (a, b, text) in edits.into_iter().rev() {
        out.replace_range(a..b, &text);
    }
    out
}

/// "15", "3.5": a plain decimal number written in digits.
fn is_decimal(w: &str) -> bool {
    let mut parts = w.split('.');
    let int = parts.next().unwrap_or_default();
    let frac = parts.next();
    parts.next().is_none()
        && !int.is_empty()
        && int.len() <= 3
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}

/// ADR 0193: a clock time before "a.m." / "p.m.": an hour 1-12, then nothing, "oh" and 1-9, or 10-59. "three" →
/// "3", "three thirty" → "3:30", "ten oh five" → "10:05".
fn spoken_clock(w: &[&str]) -> Option<String> {
    let hour = match under_100(w, 0)? {
        (h, 1) if (1..=12).contains(&h) => h,
        _ => return None,
    };
    let rest = &w[1..];
    if rest.is_empty() {
        return Some(hour.to_string());
    }
    let minutes = if rest[0] == "oh" {
        match under_100(rest, 1)? {
            (m, 1) if (1..=9).contains(&m) && rest.len() == 2 => m,
            _ => return None,
        }
    } else {
        match under_100(rest, 0)? {
            (m, n) if (10..=59).contains(&m) && n == rest.len() => m,
            _ => return None,
        }
    };
    Some(format!("{hour}:{minutes:02}"))
}

/// ADR 0230: the month names a spoken date can follow, as the recogniser writes them (capitalised).
pub const DATE_MONTHS: &[&str] = &[
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
/// ADR 0230: months that are also first names ("Give it to April first", "You can go after June first"); they take
/// only [`DATE_BEFORE_ANY`] or a weekday before them.
pub const DATE_NAME_MONTHS: &[&str] = &["April", "May", "June", "August"];
/// ADR 0230: the words right before a month that make it a date for any month ("on October fifth", "by May first").
pub const DATE_BEFORE_ANY: &[&str] = &["on", "by", "until", "till", "since", "starting"];
/// ADR 0230: the words right before a month that make it a date only for a month that isn't a first name ("move it
/// to October fifth", "the week of March third", "the deadline is November first").
pub const DATE_BEFORE_MONTH_ONLY: &[&str] = &[
    "through", "before", "after", "to", "for", "from", "of", "is", "was",
];
/// ADR 0230: the words of [`DATE_BEFORE_ANY`] / [`DATE_BEFORE_MONTH_ONLY`] that don't make a bare "first" a day:
/// after them "first" can be the adverb ("Let's focus on October first, then November", "My priority is March
/// first", "Let's get to October first"). "twenty first" / "thirty first" and every other day still convert.
pub const DATE_FIRST_NOT_AFTER: &[&str] = &["on", "to", "for", "from", "of", "is", "was"];
/// ADR 0230: a weekday right before the month (with or without its comma) also makes it a date ("Tuesday, August
/// eighteenth").
pub const DATE_WEEKDAYS: &[&str] = &[
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
/// ADR 0230: the words that may follow the day when it has no mark of its own: prepositions, conjunctions that open a
/// clause, pronouns and auxiliaries, none of which an ordinal can modify ("on April fourth graders" is refused, "on
/// April fourth at noon" is a date). Not "and" / "or" ("on April third and fourth graders").
pub const DATE_AFTER: &[&str] = &[
    "at", "in", "to", "through", "until", "till", "from", "for", "by", "is", "was", "works", "would", "will",
    "should", "could", "so", "because", "if", "when", "then", "we", "i", "you", "they", "he", "she", "it",
    "instead",
];
const ORDINAL_UNITS: &[&str] = &[
    "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth",
];
const ORDINAL_TEENS: &[&str] = &[
    "tenth",
    "eleventh",
    "twelfth",
    "thirteenth",
    "fourteenth",
    "fifteenth",
    "sixteenth",
    "seventeenth",
    "eighteenth",
    "nineteenth",
];
/// Ordinal words that can't be a day but still mark a list or a number nearby ("fortieth", "hundredth").
const ORDINAL_OTHER: &[&str] = &[
    "twentieth",
    "thirtieth",
    "fortieth",
    "fiftieth",
    "sixtieth",
    "seventieth",
    "eightieth",
    "ninetieth",
    "hundredth",
    "thousandth",
];

/// The marks a spoken day may carry at its end.
const DATE_MARKS: &[char] = &['.', ',', '!', '?', ';', ':'];

fn is_ordinal_word(w: &str) -> bool {
    ORDINAL_UNITS.contains(&w) || ORDINAL_TEENS.contains(&w) || ORDINAL_OTHER.contains(&w)
}

/// ADR 0230: a day of the month, 1-31, spoken as an ordinal: "fifth", "eighteenth", "twentieth", "twenty first" /
/// "twenty-first", "thirtieth", "thirty first". `w` is the run of lowercase words after the month, hyphens split;
/// the value and the words used.
fn spoken_day(w: &[&str]) -> Option<(u32, usize)> {
    let first = *w.first()?;
    if let Some(u) = ORDINAL_UNITS.iter().position(|o| *o == first) {
        return Some((u as u32 + 1, 1));
    }
    if let Some(t) = ORDINAL_TEENS.iter().position(|o| *o == first) {
        return Some((t as u32 + 10, 1));
    }
    match first {
        "twentieth" => return Some((20, 1)),
        "thirtieth" => return Some((30, 1)),
        _ => {}
    }
    let unit = ORDINAL_UNITS.iter().position(|o| Some(o) == w.get(1))? as u32 + 1;
    match first {
        "twenty" => Some((20 + unit, 2)),
        "thirty" if unit == 1 => Some((31, 2)),
        _ => None,
    }
}

/// "21" → "21st", "12" → "12th".
fn ordinal_figure(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// ADR 0230: a spoken date's day as a figure, the way the recogniser writes it when it writes digits ("Saturday,
/// August 15th"): "on October twenty first" → "on October 21st", "by May fifth." → "by May 5th.". The day is an
/// ordinal 1-31 in lowercase words right after a capitalised month (no mark between them), the month right after
/// one of [`DATE_BEFORE_ANY`], one of [`DATE_BEFORE_MONTH_ONLY`] (not for [`DATE_NAME_MONTHS`]) or a weekday, all
/// on one line. After the day: its own mark, the end of the text or line, or one of [`DATE_AFTER`]; neither of the
/// next two words is a number or ordinal word, the third is no ordinal, and the next one is no figure (a year,
/// "fifth, sixth", "fifth and sixth", "fifth, or the sixth"). A bare "first" after one of [`DATE_FIRST_NOT_AFTER`]
/// stays (the adverb). A month spelled like a dictionary term or alias is never touched. Runs last, after the
/// amounts. Anything else stays as spoken: "in March first we", "on April fourth graders", "October twenty one",
/// "the fifth of May", "October fifth, 2026". Only the words around each month are read (ADR 0199's budget).
fn spoken_dates(t: String, dict: &[DictionaryEntry]) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    // One pass for a month's capital (an ASCII byte is always a char boundary), then the months it can start.
    for (s, &c) in t.as_bytes().iter().enumerate() {
        if s == 0 || !matches!(c, b'J' | b'F' | b'M' | b'A' | b'S' | b'O' | b'N' | b'D') {
            continue;
        }
        for month in DATE_MONTHS {
            if month.as_bytes()[0] == c && t[s..].starts_with(month) {
                if let Some(edit) = date_at(&t, s, s + month.len(), dict) {
                    edits.push(edit);
                }
            }
        }
    }
    if edits.is_empty() {
        return t;
    }
    let mut out = t;
    for (a, b, text) in edits.into_iter().rev() {
        out.replace_range(a..b, &text);
    }
    out
}

/// ADR 0230: the next run of non-whitespace at or after byte `from`, and whether a line break comes before it.
fn next_word(t: &str, from: usize) -> Option<(&str, usize, bool)> {
    let rest = &t[from..];
    let lead = rest.find(|c: char| !c.is_whitespace())?;
    let at = from + lead;
    let len = t[at..].find(char::is_whitespace).unwrap_or(t.len() - at);
    Some((&t[at..at + len], at, rest[..lead].contains('\n')))
}

/// ADR 0230: the edit [`spoken_dates`] makes for the month at `s..e`, if it is one: (day start, day end, figure).
fn date_at(t: &str, s: usize, e: usize, dict: &[DictionaryEntry]) -> Option<(usize, usize, String)> {
    let month = &t[s..e];
    // The month is a whole word, after a word on its own line.
    if t[e..].chars().next().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let head = &t[..s];
    let pe = head.trim_end_matches(char::is_whitespace).len();
    if pe == s || pe == 0 || head[pe..].contains('\n') {
        return None;
    }
    let ps = head[..pe]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let before = &head[ps..pe];
    let in_dict = dict.iter().any(|d| {
        d.term.trim().eq_ignore_ascii_case(month)
            || d.aliases.iter().any(|a| a.trim().eq_ignore_ascii_case(month))
    });
    if in_dict {
        return None;
    }
    let low = before.to_ascii_lowercase();
    let ok_before = if before.bytes().all(|b| b.is_ascii_alphabetic()) {
        DATE_BEFORE_ANY.contains(&low.as_str())
            || (!DATE_NAME_MONTHS.contains(&month) && DATE_BEFORE_MONTH_ONLY.contains(&low.as_str()))
            || DATE_WEEKDAYS.contains(&before)
    } else {
        DATE_WEEKDAYS.contains(&before.strip_suffix(',').unwrap_or_default())
    };
    if !ok_before {
        return None;
    }
    // The day: one word ("fifth", "twenty-first") or "twenty" / "thirty" and a unit ordinal, lowercase, on the
    // month's line, with a mark allowed only after its last word.
    let on_line = |from: usize| next_word(t, from).filter(|w| !w.2);
    let body = |w: &str| w.trim_end_matches(DATE_MARKS).len();
    let lower = |w: &str| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
    let (w1, s1, _) = on_line(e)?;
    let b1 = &w1[..body(w1)];
    if !lower(b1) {
        return None;
    }
    let p1: Vec<&str> = b1.split('-').collect();
    let (day, last, last_at) = match spoken_day(&p1) {
        Some((d, n)) if n == p1.len() => (d, w1, s1),
        _ if b1.len() == w1.len() && p1.len() == 1 => {
            let (w2, s2, _) = on_line(s1 + w1.len())?;
            let b2 = &w2[..body(w2)];
            if !lower(b2) || b2.contains('-') {
                return None;
            }
            match spoken_day(&[b1, b2]) {
                Some((d, 2)) => (d, w2, s2),
                _ => return None,
            }
        }
        _ => return None,
    };
    if last_at == s1 && b1 == "first" && DATE_FIRST_NOT_AFTER.contains(&low.as_str()) {
        return None;
    }
    let body_len = body(last);
    let numberish = |w: &str| {
        let b = bare(w);
        is_number_word(&b) || is_ordinal_word(&b) || AMOUNT_NEAR_REFUSE.contains(&b.as_str())
    };
    if let Some((n, ns, _)) = on_line(last_at + last.len()) {
        if body_len == last.len() && !DATE_AFTER.contains(&bare(n).as_str()) {
            return None;
        }
        let n2 = on_line(ns + n.len());
        let n3 = n2.and_then(|(w, at, _)| on_line(at + w.len())).map(|w| w.0);
        let n2 = n2.map(|w| w.0);
        if n.bytes().any(|b| b.is_ascii_digit())
            || numberish(n)
            || n2.is_some_and(numberish)
            || n3.is_some_and(|w| is_ordinal_word(&bare(w)))
        {
            return None;
        }
    }
    Some((s1, last_at + body_len, ordinal_figure(day)))
}

/// ADR 0234: spoken titles the recogniser writes out ("Mister Hopkins", "Missus Poyser"; other decodes of the same
/// clips write "Mr. Hopkins", "Mrs. Poyser") and their written abbreviations.
pub const SPOKEN_TITLES: &[(&str, &str)] = &[
    ("Mister", "Mr."),
    ("mister", "Mr."),
    ("Missus", "Mrs."),
    ("missus", "Mrs."),
];
/// ADR 0234: "Mister and Mrs. Lee" / "Missus and Mr. Lee": the title's partner after "and" (then a name).
const TITLE_PARTNERS: &[(&str, &[&str])] = &[
    ("Mr.", &["Missus", "missus", "Mrs."]),
    ("Mrs.", &["Mister", "mister", "Mr."]),
];
/// ADR 0234: capitalised words that are not a name after a title: a sentence that starts right after a vocative
/// "mister" with no mark between them ("Excuse me mister Can you …", "Thanks mister See you"), another title, a
/// month or weekday. The title stays as spoken. Space-separated; same list in cleanup.ts.
pub const TITLE_NOT_NAMES: &str = concat!(
    "The An This That These Those There Here Then Than Thanks Thank Please Sorry Yes Yeah Yep Yup No Nope ",
    "Not Now Okay Ok Oh Hey Hi Hello Bye Goodbye Well So And But Or Nor Yet If When While Where What Why ",
    "Who Whom Whose Which How Because Since Although Though Unless Until After Before As At In On Of To ",
    "For From With Without By About Into Over Under Up Down Out Off Is Are Was Were Be Been Am Do Does ",
    "Did Done Can Could Will Would Shall Should May Might Must Have Has Had Get Got Go Let Lets It Its We ",
    "You Your Yours They Them Their He Him His She Her Hers Me My Mine Us Our Ours Some Any All Every ",
    "Each Both Either Neither None Nothing Nobody Everyone Everybody Someone Somebody Anyone Anybody Just ",
    "Also Maybe Perhaps Really Very Actually Sure Right Like Look Listen Wait Stop Come See Take Hold ",
    "Keep Give Tell Ask Make Try Call Send Put Bring Turn Watch Check Hurry Move Leave Sit Stand Follow ",
    "Help Enjoy Hope Need Want Think Know Guess Mind Excuse Pardon Remember Forget Good Great Nice Sir ",
    "Madam Mister Missus Miss Mr Mrs Ms Dr",
);
static TITLE_NOT_NAME_SET: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| TITLE_NOT_NAMES.split(' ').collect());
/// ADR 0234: names written with the spelled-out title ("Mister Rogers' Neighborhood", "Mister Ed", "Mister Softee",
/// "Mister Donut"): the title stays as spoken.
pub const TITLE_KEPT: &[&str] = &["Rogers", "Ed", "Softee", "Donut"];

/// ADR 0234: a name right after a title: one capital, then a lowercase letter (or "O'" and a capital: "O'Brien"),
/// ASCII letters with a hyphen only before a capital ("Rodriguez-Lopez"), a possessive "'s" / "s'" and a mark
/// allowed at its end, and not one of [`TITLE_NOT_NAMES`], [`DATE_MONTHS`], [`DATE_WEEKDAYS`] or [`TITLE_KEPT`].
fn title_name(w: &str) -> bool {
    let body = w.trim_end_matches(DATE_MARKS);
    let base = body
        .strip_suffix("'s")
        .or_else(|| body.strip_suffix("\u{2019}s"))
        .or_else(|| body.strip_suffix(['\'', '\u{2019}']).filter(|b| b.ends_with('s')))
        .unwrap_or(body);
    let mut c = base.chars();
    let (Some(first), Some(second)) = (c.next(), c.next()) else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    if !second.is_ascii_lowercase() {
        // "O'Brien": an apostrophe, a capital and at least one more letter.
        let third = c.next();
        if !(matches!(second, '\'' | '\u{2019}')
            && third.is_some_and(|t| t.is_ascii_uppercase())
            && c.clone().next().is_some())
        {
            return false;
        }
    }
    let mut c = c.peekable();
    while let Some(ch) = c.next() {
        let ok = ch.is_ascii_alphabetic() || (ch == '-' && c.peek().is_some_and(|n| n.is_ascii_uppercase()));
        if !ok {
            return false;
        }
    }
    !(TITLE_NOT_NAME_SET.contains(base)
        || DATE_MONTHS.contains(&base)
        || DATE_WEEKDAYS.contains(&base)
        || TITLE_KEPT.contains(&base))
}

/// ADR 0234: a spoken title before a name written as its abbreviation: "Mister Hopkins" → "Mr. Hopkins", "missus
/// Chen" → "Mrs. Chen", "Mister and Missus Lee" → "Mr. and Mrs. Lee". The title is a whole word, "Mister" /
/// "mister" / "Missus" / "missus" with nothing attached, and one space (no line break, no mark) separates it from
/// a [`title_name`], or from "and", its partner title and a name. Off when a dictionary term or alias holds
/// "mister" or "missus" (the user's own spelling). Runs last, after the dates, so no earlier step sees the new
/// dot. Anything else stays as spoken: "Hey mister, wait", "Thanks mister. That was kind", "Excuse me mister I
/// think", "Mister Rogers", "the missus", "Doctor Who", "Mr Smith".
fn spoken_titles(t: String, dict: &[DictionaryEntry]) -> String {
    if !(t.contains("ister ") || t.contains("issus ")) {
        return t;
    }
    titles_at(t, dict)
}

/// [`spoken_titles`] past its prefilter, out of line so a text with no title runs only the two searches.
#[inline(never)]
fn titles_at(t: String, dict: &[DictionaryEntry]) -> String {
    let spelled = |s: &str| {
        let l = s.to_ascii_lowercase();
        l.contains("mister") || l.contains("missus")
    };
    if dict
        .iter()
        .any(|d| spelled(&d.term) || d.aliases.iter().any(|a| spelled(a)))
    {
        return t;
    }
    // Only the words around each "ister " / "issus " are read (ADR 0199's budget). Words end at ASCII whitespace,
    // as in cleanup.ts, so the twins split alike.
    let word_end = |s: usize| {
        t[s..]
            .find(|c: char| c.is_ascii_whitespace())
            .map_or(t.len(), |i| s + i)
    };
    // The word after the one ending at byte `e`, when exactly one space separates them.
    let after = |e: usize| {
        (t[e..].starts_with(' ') && t[e + 1..].starts_with(|c: char| !c.is_ascii_whitespace()))
            .then(|| (e + 1, word_end(e + 1)))
    };
    let mut edits: Vec<(usize, usize, &str)> = Vec::new();
    for (i, _) in t.match_indices("ister ").chain(t.match_indices("issus ")) {
        // The title is a whole word: "M" / "m" right before, then the text's start or ASCII whitespace.
        let bytes = t.as_bytes();
        if i == 0 || !matches!(bytes[i - 1], b'M' | b'm') {
            continue;
        }
        let (s, e) = (i - 1, i + 5);
        if s > 0 && !bytes[s - 1].is_ascii_whitespace() {
            continue;
        }
        let Some(&(_, abbr)) = SPOKEN_TITLES.iter().find(|(w, _)| *w == &t[s..e]) else {
            continue;
        };
        let Some((n1, n1e)) = after(e) else {
            continue;
        };
        let named = title_name(&t[n1..n1e])
            || (&t[n1..n1e] == "and"
                && after(n1e).is_some_and(|(p, pe)| {
                    TITLE_PARTNERS
                        .iter()
                        .any(|(a, partners)| *a == abbr && partners.contains(&&t[p..pe]))
                        && after(pe).is_some_and(|(n, ne)| title_name(&t[n..ne]))
                }));
        if named {
            edits.push((s, e, abbr));
        }
    }
    edits.sort_unstable();
    if edits.is_empty() {
        return t;
    }
    let mut out = t;
    for (a, b, text) in edits.into_iter().rev() {
        out.replace_range(a..b, text);
    }
    out
}

/// ADR 0101: immediate repeats of a whole word, collapsed where [`collapses`] says so. ADR 0199: a scan for the pattern
/// `(?i)(?<![\w'])(\w+)(?:[\s,]+\1(?![\w']))+` (ASCII `\w`). The apostrophe is part of a word on both sides, so
/// "you, you're" and "it, it's" are not repeats. The word is a whole run of word characters (a shorter one would be
/// followed by a word character, never by `[\s,]`); each repeat is a run of whitespace or commas and the same word,
/// in any ASCII case, with no word character or apostrophe after it. A text with "ſ" or the Kelvin sign (word
/// characters to `(?i)`, with fancy-regex's non-ASCII backreference rules) takes the regex itself.
fn collapse_repeats(s: &str) -> String {
    if s.contains(['\u{17F}', '\u{212A}']) {
        return replace_all_or_keep(&REPEATS, s, |c: &fancy_regex::Captures<str>| {
            let (all, word) = (c.get(0).unwrap(), c.get(1).unwrap());
            if collapses(&c[1], &c[0]) || subject_stutter(s, all.start(), word.end(), all.end()) {
                c[1].to_owned()
            } else {
                c[0].to_owned()
            }
        });
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut last, mut p) = (0, 0);
    while p < b.len() {
        let c = b[p];
        let starts = (c.is_ascii_alphanumeric() || c == b'_')
            && !char_before(s, p).is_some_and(|q| is_w(q) || q == '\'');
        if !starts {
            p += s[p..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let word_end = p + run_len(&s[p..], is_w);
        let word = &b[p..word_end];
        let mut x = word_end;
        loop {
            let y = x + run_len(&s[x..], |c| c == ',' || c.is_whitespace());
            let z = y + word.len();
            if y == x || z > b.len() || !b[y..z].eq_ignore_ascii_case(word) {
                break;
            }
            if s[z..].chars().next().is_some_and(|q| is_w(q) || q == '\'') {
                break;
            }
            x = z;
        }
        if x > word_end {
            let all = &s[p..x];
            out.push_str(&s[last..p]);
            out.push_str(
                if collapses(&s[p..word_end], all) || subject_stutter(s, p, word_end, x) {
                    &s[p..word_end]
                } else {
                    all
                },
            );
            last = x;
            p = x;
        } else {
            p = word_end;
        }
    }
    out.push_str(&s[last..]);
    out
}

/// Runs of spaces and tabs as one space (the pattern `[ \t]+`).
fn collapse_blanks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = false;
    for c in s.chars() {
        if c == ' ' || c == '\t' {
            if !blank {
                out.push(' ');
            }
            blank = true;
        } else {
            out.push(c);
            blank = false;
        }
    }
    out
}

/// ADR 0120 (N167): a lone "." on a word with a lowercase word after it. The recogniser lowercased what follows, so
/// it is not a sentence end: on a segmented recogniser (ADR 0118) it is the "." each segment's decode writes at its cut.
/// The dot is dropped unless the word is an abbreviation ([`ABBREVIATIONS`]); either way the next word stays lowercase.
/// ADR 0199: a scan for the pattern `(?<![\w.])([A-Za-z]+)\.(?=\s+[a-z])`.
fn continued_dot(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut last, mut p) = (0, 0);
    while p < b.len() {
        if !b[p].is_ascii_alphabetic() || char_before(s, p).is_some_and(|q| is_w(q) || q == '.') {
            p += s[p..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let word_end = p + run_len(&s[p..], |c| c.is_ascii_alphabetic());
        if b.get(word_end) == Some(&b'.') {
            let gap = run_len(&s[word_end + 1..], char::is_whitespace);
            if gap > 0 && b.get(word_end + 1 + gap).is_some_and(u8::is_ascii_lowercase) {
                let word = &s[p..word_end];
                out.push_str(&s[last..p]);
                out.push_str(if is_abbreviation(word) {
                    &s[p..=word_end]
                } else {
                    word
                });
                last = word_end + 1;
            }
        }
        p = word_end;
    }
    out.push_str(&s[last..]);
    out
}

/// A sentence's first lowercase letter, capitalized. ADR 0101 (N160): not after a dotted abbreviation ("e.g. the",
/// "3 p.m. today") or an abbreviation word ([`ends_with_abbreviation`]). ADR 0199: a scan for the pattern
/// `(^|(?<![A-Za-z]\.[A-Za-z])[.!?]\s+|\n)([a-z])`.
fn sentence_starts(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut last, mut p) = (0, 0);
    while p < b.len() {
        // (opener end, letter) of a match at p.
        let hit = if p == 0 && b[0].is_ascii_lowercase() {
            Some(0)
        } else if matches!(b[p], b'.' | b'!' | b'?')
            && !(p >= 3
                && b[p - 3].is_ascii_alphabetic()
                && b[p - 2] == b'.'
                && b[p - 1].is_ascii_alphabetic())
        {
            let gap = run_len(&s[p + 1..], char::is_whitespace);
            let at = p + 1 + gap;
            (gap > 0 && b.get(at).is_some_and(u8::is_ascii_lowercase)).then_some(at)
        } else if b[p] == b'\n' && b.get(p + 1).is_some_and(u8::is_ascii_lowercase) {
            Some(p + 1)
        } else {
            None
        };
        let Some(at) = hit else {
            p += s[p..].chars().next().map_or(1, char::len_utf8);
            continue;
        };
        out.push_str(&s[last..p]);
        if b[p] == b'.' && ends_with_abbreviation(&s[..p]) {
            out.push_str(&s[p..=at]);
        } else {
            out.push_str(&s[p..at]);
            out.push(b[at].to_ascii_uppercase() as char);
        }
        last = at + 1;
        p = at + 1;
    }
    out.push_str(&s[last..]);
    out
}

/// A lone "i" as "I". ADR 0101 (N160): before a dot only at a sentence end, not the "i" of "i.e.". ADR 0199: a scan
/// for the pattern `(?<![\w])i(?![\w])(?=[\s,!?']|\.(?![A-Za-z])|$)`.
fn lone_i(s: &str) -> String {
    if !s.contains('i') {
        return s.to_owned();
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (p, _) in s.match_indices('i') {
        if char_before(s, p).is_some_and(is_w) {
            continue;
        }
        let lone = match s[p + 1..].chars().next() {
            None => true,
            Some(c) if c.is_whitespace() || matches!(c, ',' | '!' | '?' | '\'') => true,
            Some('.') => !b.get(p + 2).is_some_and(u8::is_ascii_alphabetic),
            Some(_) => false,
        };
        if lone {
            out.push_str(&s[last..p]);
            out.push('I');
            last = p + 1;
        }
    }
    out.push_str(&s[last..]);
    out
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

/// ADR 0188: a list of short lines ("Groceries⏎Eggs⏎Milk", an address, a sign-off): at least this many lines.
pub const SHORT_LIST_LINES: usize = 3;

/// ADR 0188: is the text a list of short lines? At least [`SHORT_LIST_LINES`] lines, no blank line between them (a
/// "new paragraph" makes it a letter), each line 1 to [`SHORT_DICTATION_WORDS`] words with no sentence mark inside it,
/// and none but the last ending in ".", "!" or "?". Two lines ("Dear team,⏎Quick update.", "Call her⏎Then email
/// Sam.") are not a list.
fn is_short_list(t: &str) -> bool {
    if t.contains("\n\n") {
        return false;
    }
    let lines: Vec<&str> = t.split('\n').collect();
    if lines.len() < SHORT_LIST_LINES {
        return false;
    }
    let last = lines.len() - 1;
    lines.iter().enumerate().all(|(i, l)| {
        let words = l.split_whitespace().count();
        (1..=SHORT_DICTATION_WORDS).contains(&words)
            && !INNER_END.is_match(l).unwrap_or(true)
            && (i == last || !l.ends_with(['.', '!', '?']))
    })
}

/// ADR 0201: a list line takes its comma off only when it has at most this many words; "Roses are red,⏎violets are
/// blue,⏎sugar is sweet" (3-word verse lines) keeps its commas.
pub const LIST_COMMA_MAX_WORDS: usize = 2;

/// ADR 0201: a line with one of these words is a greeting or a sign-off ("Dear team,", "Thanks,", "Love,", "Talk
/// soon,"), whose comma is meant, so the list keeps every comma. Same list in cleanup.ts.
pub const LETTER_WORDS: &[&str] = &[
    "dear",
    "hi",
    "hello",
    "hey",
    "hiya",
    "greetings",
    "morning",
    "afternoon",
    "evening",
    "thanks",
    "thank",
    "thx",
    "cheers",
    "best",
    "regards",
    "sincerely",
    "faithfully",
    "cordially",
    "respectfully",
    "warmly",
    "love",
    "yours",
    "xo",
    "xoxo",
    "hugs",
    "care",
    "soon",
    "bye",
    "goodbye",
    "later",
];

/// ADR 0201: the comma the recogniser wrote before each spoken "new line" of a dictated list ("Groceries, new line,
/// eggs, new line, milk." → "Groceries⏎Eggs⏎Milk"), dropped only when the whole text is such a list: at least
/// [`SHORT_LIST_LINES`] lines and no blank line; every line but the last ends in "," and the last in a word or ".";
/// every line is 1 to [`LIST_COMMA_MAX_WORDS`] words of ASCII letters only (no digits, apostrophes, inner commas or
/// marks); every line after the first starts lowercase (the recogniser wrote a common word, not a name, so an address
/// or a list of names keeps its commas); no line has a [`LETTER_WORDS`] word. Only for a recogniser that writes
/// capitals (`caps`), since only then is the lowercase start a signal. Runs before capitalization.
fn list_line_commas(t: String, caps: bool) -> String {
    if !caps || !t.contains(",\n") || t.contains("\n\n") {
        return t;
    }
    let lines: Vec<&str> = t.split('\n').collect();
    if lines.len() < SHORT_LIST_LINES {
        return t;
    }
    let last = lines.len() - 1;
    let list = lines.iter().enumerate().all(|(i, l)| {
        let body = if i == last {
            l.strip_suffix('.').unwrap_or(l)
        } else {
            match l.strip_suffix(',') {
                Some(b) => b,
                None => return false,
            }
        };
        let words: Vec<&str> = body.split(' ').collect();
        (1..=LIST_COMMA_MAX_WORDS).contains(&words.len())
            && words
                .iter()
                .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphabetic()))
            && (i == 0 || body.as_bytes()[0].is_ascii_lowercase())
            && !words
                .iter()
                .any(|w| LETTER_WORDS.contains(&w.to_ascii_lowercase().as_str()))
    });
    if list {
        t.replace(",\n", "\n")
    } else {
        t
    }
}

/// ADR 0196: the most words the last sentence may have for [`unmarked_question`]; a longer unpunctuated run is as
/// likely a question run into a statement as one question.
pub const QUESTION_MAX_WORDS: usize = 12;

/// ADR 0196: may a sentence that opens with this auxiliary and this subject only be a yes/no question? Personal
/// pronouns, with the verb agreeing, plus "this" / "that" / "there" after a few verbs. Left out on purpose: "should",
/// "had", "may", "might", "must" (conditional or wishing inversion: "Should you need anything", "Had I known", "May you
/// both be happy"), "am I" and "was I" (exclamations: "Am I glad you called", "Was I surprised"), "did it" and "can it"
/// ("Did it again" drops its subject; "Can it" is slang), any noun or "the"/"a"/"my" after the verb ("Do the dishes",
/// "Have a good one", "Does the trick", "Was the best day ever" are commands or dropped subjects), and negative
/// contractions ("Don't you dare").
fn yes_no_opening(aux: &str, subject: &str, next: &str) -> bool {
    const PLURAL: &[&str] = &["i", "you", "we", "they"];
    const SINGULAR: &[&str] = &["he", "she", "it"];
    const THERE_NEXT: &[&str] = &[
        "any",
        "anything",
        "anyone",
        "anybody",
        "a",
        "an",
        "another",
        "something",
        "someone",
        "somebody",
        "more",
        "enough",
        "still",
        "no",
    ];
    let personal = |extra: &[&str]| PLURAL.contains(&subject) || extra.contains(&subject);
    match aux {
        "do" | "have" => PLURAL.contains(&subject),
        "does" | "has" => SINGULAR.contains(&subject),
        "did" | "can" => personal(&["he", "she"]),
        "is" => {
            SINGULAR.contains(&subject)
                || matches!(subject, "this" | "that")
                || (subject == "there" && THERE_NEXT.contains(&next))
        }
        "are" => {
            matches!(subject, "you" | "we" | "they") || (subject == "there" && THERE_NEXT.contains(&next))
        }
        "was" => SINGULAR.contains(&subject),
        "were" => matches!(subject, "you" | "we" | "they"),
        "will" | "could" => personal(&["he", "she", "it", "this", "that"]),
        "would" => personal(&["he", "she", "it"]),
        "shall" => matches!(subject, "i" | "we"),
        _ => false,
    }
}

/// ADR 0196: a wh-question's opening, "What time does the store close", "Where do you", "How long will it": the
/// wh-word, at most one of these words, an auxiliary, then a pronoun or a determiner.
const WH_WORDS: &[&str] = &["what", "where", "when", "why", "how", "who"];
const WH_MODIFIERS: &[&str] = &[
    "time", "day", "size", "long", "much", "often", "far", "old", "soon",
];
const WH_AUX: &[&str] = &[
    "do", "does", "did", "is", "are", "am", "was", "were", "can", "could", "will", "would", "should", "have",
    "has",
];
const WH_SUBJECTS: &[&str] = &[
    "i", "you", "we", "they", "he", "she", "it", "the", "this", "that", "my", "your", "our", "their",
];
/// ADR 0196: after the opening, any of these means a second clause ("Do you know where the keys are", "Can you send it
/// we need it"): the sentence is left as before.
const LATER_CLAUSE: &[&str] = &[
    "is", "are", "was", "were", "am", "can", "could", "will", "would", "should", "shall", "may", "might",
    "must", "does", "did", "had", "i", "we", "they", "he", "she", "thank", "thanks",
];

/// ADR 0203: a discourse word that may stand before a question's opening, with or without its own comma: "So do you
/// want to grab lunch", "Okay, can you send me the file", "Hey are you free tomorrow". Left out on purpose: "well",
/// "oh", "boy", "man" (exclamatory inversion: "Well do I remember that day", "Oh do I love that song", "Boy do I need
/// a break") and "now" ("Now would you look at that"). Two openers ("Okay so do you") are refused. After "and" /
/// "but" a yes/no question needs "you" as its subject: a story's exclamatory inversion ("And was he mad", "And do
/// I miss it", "And were we surprised") takes the others.
pub const QUESTION_OPENERS: &[&str] = &["so", "okay", "ok", "hey", "and", "but", "also", "alright"];
/// ADR 0203: "So" with no comma also opens the agreeing ellipsis ("So did we last year", "So do I at work", "So can
/// we in the summer"), so after a bare "So" only these auxiliaries, and only when a plain verb from
/// [`SO_QUESTION_VERBS`] follows the subject (a verb there means the verb phrase is said, so it can't be the ellipsis).
const SO_QUESTION_AUX: &[&str] = &["do", "does", "did", "can", "could", "will", "would"];
const SO_QUESTION_VERBS: &[&str] = &[
    "want",
    "wanna",
    "need",
    "know",
    "think",
    "have",
    "mind",
    "remember",
    "see",
    "mean",
    "get",
    "feel",
    "agree",
    "understand",
    "send",
    "make",
    "help",
    "check",
    "take",
    "give",
    "tell",
    "call",
    "look",
    "come",
    "go",
];

/// ADR 0232: a wh-word with its auxiliary contracted, as the first word of a question: the wh-word, the auxiliary
/// it is read as, and the subjects that agree with it ("What's the plan", "Where're you headed", "How'd it go",
/// "When'll it be ready", "What've you got"). "'s" takes only singular subjects and determiners (not "I" / "you" /
/// "we" / "they"); "'re" only plural ones; "'ve" only "I" / "you" / "we" / "they". "'s" is read as "is" and "'d" as
/// "would" for the later checks ("ever" refuses after both). Not here: "his" / "her" after "What's" (not in
/// [`WH_SUBJECTS`]: "What's his name called me" is the placeholder name), "'d" + "you know", and any other
/// contraction or apostrophe. A modifier before the contraction ("What time's") is ADR 0233's
/// [`WH_MODIFIER_CONTRACTIONS`]. Same table in cleanup.ts.
pub const WH_CONTRACTIONS: &[(&str, &str, &[&str])] = &[
    (
        "s",
        "is",
        &[
            "he", "she", "it", "the", "this", "that", "my", "your", "our", "their",
        ],
    ),
    (
        "re",
        "are",
        &["you", "we", "they", "the", "my", "your", "our", "their"],
    ),
    ("ll", "will", WH_SUBJECTS),
    ("d", "would", WH_SUBJECTS),
    ("ve", "have", &["i", "you", "we", "they"]),
];

/// ADR 0232: after a wh-contraction, a verb that can take the whole wh-clause as its subject ("What's the right
/// answer depends on the context", "When's the launch matters less") makes the sentence a statement; the
/// contraction is refused when any of these follows it. (LATER_CLAUSE already covers "is", "will", "should", ...)
const EMBEDDED_PREDICATES: &[&str] = &[
    "depends",
    "matters",
    "varies",
    "remains",
    "seems",
    "determines",
    "decides",
    "counts",
    "changes",
    "has",
    "makes",
    "means",
    "tells",
    "shows",
    "stays",
];

/// ADR 0232: `word` (lowercased) as a wh-contraction: "what's" / "how\u{2019}d" → (wh-word, auxiliary, subjects).
fn wh_contraction(word: &str) -> Option<(&'static str, &'static str, &'static [&'static str])> {
    let (wh, tail) = word.split_once(['\'', '\u{2019}'])?;
    let wh = WH_WORDS.iter().find(|w| **w == wh)?;
    let (_, aux, agree) = WH_CONTRACTIONS.iter().find(|(t, _, _)| *t == tail)?;
    Some((wh, aux, agree))
}

/// ADR 0233: the modifiers a contraction may sit on, per wh-word ("What time's the meeting", "How long'll it take",
/// "How long've you been here"): the [`WH_MODIFIERS`] that make a wh-question opening with "what" or "how". Same table
/// in cleanup.ts.
pub const WH_MODIFIER_CONTRACTIONS: &[(&str, &[&str])] = &[
    ("what", &["time", "day", "size"]),
    ("how", &["long", "much", "often", "far", "old", "soon"]),
];

/// ADR 0233: `wh` + `word` (lowercased) as a wh-word and its modifier with the auxiliary contracted onto the modifier:
/// ("how", "long'll") → (wh-word, modifier, auxiliary, subjects). The tails and their subjects are
/// [`WH_CONTRACTIONS`]'s.
fn modifier_contraction(
    wh: &str,
    word: &str,
) -> Option<(&'static str, &'static str, &'static str, &'static [&'static str])> {
    let (wh, modifiers) = WH_MODIFIER_CONTRACTIONS.iter().find(|(w, _)| *w == wh)?;
    let (modifier, tail) = word.split_once(['\'', '\u{2019}'])?;
    let modifier = modifiers.iter().find(|m| **m == modifier)?;
    let (_, aux, agree) = WH_CONTRACTIONS.iter().find(|(t, _, _)| *t == tail)?;
    Some((wh, modifier, aux, agree))
}

/// ADR 0196: does the dictation end in an unpunctuated sentence that can only be a question? Its opening is a yes/no
/// question's ([`yes_no_opening`], the auxiliary capitalised as a sentence start) or a wh-question's, it has 3 to
/// [`QUESTION_MAX_WORDS`] words, no comma, colon, semicolon, quote, bracket or apostrophe, and nothing after the
/// opening that starts another clause ([`LATER_CLAUSE`]). Also refused: "ever" unless the verb is "have"/"has"
/// ("Is it ever hot", "Did I ever love that car" are exclamations), "were" with a "to" ("Were we to lose"), "just
/// say" ("Can I just say"), "What do you know", "Have I got" ("Have I got news for you"), and the spoken name of a
/// mark ("question mark", "exclamation"): ADR 0189's sentences stay exactly as that rule leaves them. ADR 0203: the
/// sentence may start with one of [`QUESTION_OPENERS`] (its comma, if any, is the only comma allowed); the checks
/// then run on the words after it, which must still be 3 or more. ADR 0232: that first word (after any opener) may
/// be a wh-word with its auxiliary contracted ([`wh_contraction`]: "What's the plan for tonight", "How'd it go
/// with the client"), the only apostrophe allowed; it is read as the two words, its subject checked against the
/// contraction's own list. ADR 0233: or that second word, after "what" / "how", may be a modifier with the
/// auxiliary contracted onto it ([`modifier_contraction`]: "What time's the meeting tomorrow", "How long'll it
/// take"), read as the three words and checked the same way.
fn unmarked_question(t: &str) -> bool {
    let line = &t[t.rfind('\n').map_or(0, |i| i + 1)..];
    let s = match INNER_END.find_iter(line).flatten().last() {
        Some(m) => &line[m.end()..],
        None => line,
    };
    let apostrophes = s.matches(['\'', '\u{2019}']).count();
    if s.contains([';', ':', '"', '(', ')']) || apostrophes > 1 {
        return false;
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    if !(3..=QUESTION_MAX_WORDS).contains(&words.len())
        || !words[0].starts_with(|c: char| c.is_ascii_uppercase())
    {
        return false;
    }
    let w: Vec<String> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    let first = w[0].strip_suffix(',');
    let opener = QUESTION_OPENERS.contains(&first.unwrap_or(w[0]));
    if s.matches(',').count() != usize::from(first.is_some()) || (first.is_some() && !opener) {
        return false;
    }
    let bare_so = w[0] == "so";
    let you_only = matches!(first.unwrap_or(w[0]), "and" | "but");
    let w = if opener { &w[1..] } else { &w[..] };
    if w.len() < 3 {
        return false;
    }
    // ADR 0232: the one apostrophe must be a wh-contraction's, and the sentence reads as its two words.
    let mut subjects: &[&str] = WH_SUBJECTS;
    let spelled: Vec<&str>;
    let w = if apostrophes == 1 {
        if let Some((wh, aux, agree)) = wh_contraction(w[0]) {
            if (wh == "what" && aux == "would" && w[1..] == ["you", "know"])
                || w[1..].iter().any(|x| EMBEDDED_PREDICATES.contains(x))
            {
                return false;
            }
            subjects = agree;
            spelled = [wh, aux].into_iter().chain(w[1..].iter().copied()).collect();
        } else {
            // ADR 0233: "What time's the meeting" reads as "what time is the meeting".
            let Some((wh, modifier, aux, agree)) = modifier_contraction(w[0], w[1]) else {
                return false;
            };
            if w[2..].iter().any(|x| EMBEDDED_PREDICATES.contains(x)) {
                return false;
            }
            subjects = agree;
            spelled = [wh, modifier, aux]
                .into_iter()
                .chain(w[2..].iter().copied())
                .collect();
        }
        &spelled[..]
    } else {
        w
    };
    let (aux_at, opening) = if WH_WORDS.contains(&w[0]) {
        let aux_at = if WH_MODIFIERS.contains(&w[1]) { 2 } else { 1 };
        let ok = aux_at + 1 < w.len()
            && WH_AUX.contains(&w[aux_at])
            && subjects.contains(&w[aux_at + 1])
            && !(w[aux_at] == "did" && w[aux_at + 1] == "it")
            && w[..] != ["what", "do", "you", "know"];
        (aux_at, ok)
    } else {
        let ok = yes_no_opening(w[0], w[1], w.get(2).copied().unwrap_or(""))
            && (!bare_so || (SO_QUESTION_AUX.contains(&w[0]) && SO_QUESTION_VERBS.contains(&w[2])))
            && (!opener || !you_only || w[1] == "you");
        (0, ok)
    };
    let aux = w[aux_at];
    let rest = &w[(aux_at + 2).min(w.len())..];
    if !opening || rest.iter().any(|x| LATER_CLAUSE.contains(x)) {
        return false;
    }
    let ever = w.contains(&"ever") && !matches!(aux, "have" | "has");
    let were_to = aux == "were" && w.contains(&"to");
    let refused_pair = w
        .windows(2)
        .any(|p| p == ["just", "say"] || p == ["question", "mark"]);
    !ever && !were_to && !refused_pair && !w.contains(&"exclamation") && w[..3] != ["have", "i", "got"]
}

/// The terminal punctuation (ADR 0101): a short dictation ends as said, without a period; a longer one gets a "."
/// when it ends on a word. ADR 0188: a list of short lines ends like a short dictation: no "." is added, and a "."
/// the recogniser ended it with is dropped only when it is a clean sentence end (not "p.m.", "etc.", "..."). ADR 0196:
/// the mark added is "?" when the last sentence can only be a question ([`unmarked_question`]). ADR 0206: also when it
/// is "Hey <dictionary name> <question>" ([`vocative_question`]), with a comma after the name. ADR 0227: also when it
/// ends in a tag question ([`tag_question`]: "..., isn't it").
fn terminal_punctuation(mut t: String, dict: &[DictionaryEntry]) -> String {
    if is_short_list(&t) {
        if t.ends_with('.') && clean_sentence_end(&t) {
            t.pop();
        }
    } else if is_short(&t) {
        // A lone final "." only: not "...", not the last dot of "p.m." or "U.S.".
        let last = t.split_whitespace().last().unwrap_or_default();
        if let Some(body) = last.strip_suffix('.') {
            if !body.is_empty() && !body.contains('.') {
                t.pop();
            }
        }
    } else if ENDS_WORD.is_match(&t).unwrap_or(false) {
        if unmarked_question(&t) {
            t = opener_comma(t);
            t.push('?');
        } else if let Some(at) = vocative_question(&t, dict) {
            t.insert(at, ',');
            t.push('?');
        } else if tag_question(&t) {
            // ADR 0227: "It's a nice day, isn't it" → "It's a nice day, isn't it?".
            t.push('?');
        } else {
            t.push('.');
        }
    }
    t
}

/// ADR 0204: the interjections among [`QUESTION_OPENERS`] that take a comma before the question they open ("Hey,
/// are you free tomorrow?", "OK, can we move it?"). Left out: "so", "and", "but", "also" (conjunctions and adverbs,
/// written with or without a comma: "So do you want to grab lunch?" stays as said). Same list in cleanup.ts.
pub const QUESTION_COMMA_OPENERS: &[&str] = &["hey", "okay", "ok", "alright"];

/// ADR 0204: called only when [`unmarked_question`] has said the last sentence is a question; if that sentence opens
/// with one of [`QUESTION_COMMA_OPENERS`] with no comma of its own ("Hey are you free"), the comma goes after it.
/// Anything else is returned as it is.
fn opener_comma(mut t: String) -> String {
    let line = t.rfind('\n').map_or(0, |i| i + 1);
    let start = match INNER_END.find_iter(&t[line..]).flatten().last() {
        Some(m) => line + m.end(),
        None => line,
    };
    let s = &t[start..];
    let lead = s.len() - s.trim_start().len();
    let rest = &s[lead..];
    let Some(len) = rest.find(char::is_whitespace) else {
        return t;
    };
    if QUESTION_COMMA_OPENERS.contains(&rest[..len].to_ascii_lowercase().as_str()) {
        t.insert(start + lead + len, ',');
    }
    t
}

/// ADR 0206: the subjects a question may have after a vocative name: the nominative personal pronouns. Left out:
/// "it", "this", "that", "there", which after a name read as a statement's object or predicate ("Hey, Sam is it",
/// "Okay Sam is that kind of guy", "Sam has it covered", "Sam is there a lot").
pub const VOCATIVE_SUBJECTS: &[&str] = &["i", "you", "we", "they", "he", "she"];

/// ADR 0208: after "Hey <name> did you", these verbs also open the question, besides [`SO_QUESTION_VERBS`] ("Hey
/// Jared did you finish the report"). "did you" is the one opening where "you" can also be the object of a statement
/// about the name ("Jared did you a favor", "did you good", "did you wrong", "did you proud", "did you justice",
/// "did you credit"), so the list is closed and holds only plain base-form verbs that never stand bare as that
/// complement. Left out on purpose: "like" ("did you like a pro"), "turn" ("did you a good turn"), "forward", "save",
/// "close", "clean", "open", "fine", "miss", "love", "harm", "damage", "service", "honor", "right", "wrong", "good",
/// "well". Same list in cleanup.ts.
pub const VOCATIVE_DID_VERBS: &[&str] = &[
    "finish", "hear", "read", "try", "find", "eat", "sign", "pick", "bring", "buy", "pay", "book", "order",
    "review", "fix", "push", "merge", "ship", "submit", "reply", "talk", "speak", "ask", "watch", "catch",
    "grab", "reach", "leave", "notice", "forget", "decide", "start", "test", "update", "install", "email",
    "text", "invite", "confirm", "cancel", "print", "meet", "move", "share", "write", "say", "run", "put",
    "play", "plan", "visit", "enjoy", "sleep",
];

/// ADR 0206: words the question test reads; a dictionary term spelled like one of them is never taken for a name.
const VOCATIVE_NOT_NAMES: &[&str] = &[
    "there",
    "had",
    "may",
    "might",
    "must",
    "shall",
    "should",
    "ever",
    "just",
    "question",
    "exclamation",
];

/// ADR 0206: can `word` be a vocative name: a term of the user's dictionary, written exactly as the term, one ASCII
/// word of 2+ letters with a capital first ("Jared", "Priya", "McKenzie"), and not a word the question test reads
/// (an opener, wh-word, auxiliary, pronoun, plain verb: a term "Will" or "Who" is refused).
fn vocative_name(word: &str, dict: &[DictionaryEntry]) -> bool {
    let b = word.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_uppercase() || !b.iter().all(u8::is_ascii_alphabetic) {
        return false;
    }
    let lower = word.to_ascii_lowercase();
    let reads: [&[&str]; 11] = [
        QUESTION_OPENERS,
        WH_WORDS,
        WH_MODIFIERS,
        WH_AUX,
        WH_SUBJECTS,
        LATER_CLAUSE,
        VOCATIVE_SUBJECTS,
        SO_QUESTION_VERBS,
        VOCATIVE_DID_VERBS,
        SUBJECT_MODALS,
        VOCATIVE_NOT_NAMES,
    ];
    !reads.iter().any(|l| l.contains(&lower.as_str())) && dict.iter().any(|d| d.term == word)
}

/// ADR 0206: the byte where the last sentence starts: after the last line break or the last ".", "!" or "?" that a
/// whitespace character follows (the same place [`INNER_END`]'s last match ends), found from the end.
fn last_sentence_start(t: &str) -> usize {
    let mut next: Option<(usize, char)> = None;
    for (i, c) in t.char_indices().rev() {
        if c == '\n' {
            return i + 1;
        }
        if let Some((j, n)) = next {
            if matches!(c, '.' | '!' | '?') && n.is_whitespace() {
                return j + n.len_utf8();
            }
        }
        next = Some((i, c));
    }
    0
}

/// ADR 0206: a question to someone named from the user's dictionary, "Hey Jared are you free tomorrow" → "Hey Jared,
/// are you free tomorrow?". Called only where [`unmarked_question`] found no question. The last sentence must be
/// exactly "Hey" (no comma), one [`vocative_name`], then words that [`unmarked_question`] takes as a question after
/// "Hey" ("Hey are you free tomorrow"), with no comma anywhere, at most [`QUESTION_MAX_WORDS`] words counting the
/// name, a [`VOCATIVE_SUBJECTS`] subject for a yes/no question, after "did you" one of [`SO_QUESTION_VERBS`] or
/// (ADR 0208) [`VOCATIVE_DID_VERBS`] ("Hey, Sam did you a favor" is a statement), and no "who" ("Hey Sam who is our lead" can be a relative clause). Returns
/// the byte after the name, where the comma goes. Only "hey": after "okay"/"ok"/"alright" a term is as often a topic
/// or a subject ("Okay Teams have you covered").
fn vocative_question(t: &str, dict: &[DictionaryEntry]) -> Option<usize> {
    if dict.is_empty() {
        return None;
    }
    let start = last_sentence_start(t);
    let s = &t[start..];
    let lead = s.len() - s.trim_start().len();
    let rest = &s[lead..];
    // ADR 0232: no apostrophe, so a wh-contraction after a name stays as ADR 0206 left it.
    if !rest[..run_len(rest, |c| !c.is_whitespace())].eq_ignore_ascii_case("hey")
        || s.contains([',', '\'', '\u{2019}'])
    {
        return None;
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() < 5
        || words.len() > QUESTION_MAX_WORDS
        || !words[0].eq_ignore_ascii_case("hey")
        || !vocative_name(words[1], dict)
    {
        return None;
    }
    let w: Vec<String> = words[2..].iter().map(|w| w.to_ascii_lowercase()).collect();
    // ADR 0206 / 0208: after "did you" only a listed verb ("Hey, Sam did you a favor" is a statement).
    let did_you_verb = w[0] != "did"
        || w[1] != "you"
        || SO_QUESTION_VERBS.contains(&w[2].as_str())
        || VOCATIVE_DID_VERBS.contains(&w[2].as_str());
    let subject_ok = if WH_WORDS.contains(&w[0].as_str()) {
        w[0] != "who"
    } else {
        VOCATIVE_SUBJECTS.contains(&w[1].as_str()) && did_you_verb
    };
    if !subject_ok || !unmarked_question(&format!("{} {}", words[0], words[2..].join(" "))) {
        return None;
    }
    let after_hey = lead + words[0].len();
    let gap = run_len(&s[after_hey..], char::is_whitespace);
    Some(start + after_hey + gap + words[1].len())
}

/// ADR 0227: a tag question's tag, the closed list: a negative contraction and the pronouns it takes as a tag ("It's a
/// nice day, isn't it", "You sent it, didn't you", "There's time, isn't there", "I was right, wasn't I"). Agreement
/// is checked, so "isn't they" or "don't it" refuse. Left out: positive tags ("is it", "do you", "will you": a
/// cut-off question start or "Close the door, will you" need their own evidence), "shan't" and "ain't" (thin
/// evidence), "amn't", and "this" / "that" ("isn't that" is as often an unfinished "isn't that the one"). Same table
/// in cleanup.ts.
pub const TAG_QUESTIONS: &[(&str, &[&str])] = &[
    ("isn't", &["he", "she", "it", "there"]),
    ("wasn't", &["he", "she", "it", "there", "I"]),
    ("doesn't", &["he", "she", "it"]),
    ("hasn't", &["he", "she", "it", "there"]),
    ("aren't", &["I", "you", "we", "they", "there"]),
    ("weren't", &["you", "we", "they", "there"]),
    ("don't", &["I", "you", "we", "they"]),
    ("haven't", &["I", "you", "we", "they", "there"]),
    ("hadn't", &["I", "you", "we", "they", "he", "she", "it", "there"]),
    ("didn't", &["I", "you", "we", "they", "he", "she", "it"]),
    ("won't", &["I", "you", "we", "they", "he", "she", "it", "there"]),
    (
        "wouldn't",
        &["I", "you", "we", "they", "he", "she", "it", "there"],
    ),
    ("can't", &["I", "you", "we", "they", "he", "she", "it"]),
    ("couldn't", &["I", "you", "we", "they", "he", "she", "it"]),
    ("shouldn't", &["I", "you", "we", "they", "he", "she", "it"]),
    ("mustn't", &["I", "you", "we", "they", "he", "she", "it"]),
];

/// ADR 0227: the one three-word tag form, "don't you think" / "don't you agree" / "wouldn't you say" ("It's too late
/// now, don't you think"). Any other word after the pronoun is a clause ("Whatever happens, don't you worry").
const TAG_THIRD_WORDS: &[(&str, &[&str])] = &[
    ("don't", &["think", "agree"]),
    ("wouldn't", &["think", "agree", "say"]),
];

/// ADR 0227: does the dictation end in a tag question with no mark of its own? Called only where a "." would be
/// added and [`unmarked_question`] and [`vocative_question`] said no. The last sentence ends in ", " then exactly one
/// [`TAG_QUESTIONS`] pair (or a [`TAG_THIRD_WORDS`] form after "you"), written as the recogniser writes a tag mid-sentence:
/// the contraction lowercase (ASCII or curly apostrophe), the pronoun lowercase or "I"; with at least one word
/// before the comma, and no `; : " ( )` in the sentence. Short dictations (≤ 3 words) never get here (ADR 0101).
fn tag_question(t: &str) -> bool {
    let s = &t[last_sentence_start(t)..];
    if s.contains([';', ':', '"', '(', ')']) {
        return false;
    }
    let Some(comma) = s.rfind(',') else {
        return false;
    };
    if s[..comma].split_whitespace().next().is_none() || !s[comma + 1..].starts_with(' ') {
        return false;
    }
    let tag: Vec<String> = s[comma + 1..]
        .split_whitespace()
        .map(|w| w.replace('\u{2019}', "'"))
        .collect();
    let (aux, pronoun, third) = match tag.as_slice() {
        [a, p] => (a, p, None),
        [a, p, x] => (a, p, Some(x)),
        _ => return false,
    };
    if third.is_none() && imperative_tag(&s[..comma], aux, pronoun) {
        // ADR 0229: "Let's grab lunch, shall we" / "Please close the door, will you".
        return true;
    }
    let agrees = TAG_QUESTIONS
        .iter()
        .any(|(a, ps)| a == aux && ps.contains(&pronoun.as_str()));
    let third_ok = third.is_none_or(|x| {
        pronoun == "you"
            && TAG_THIRD_WORDS
                .iter()
                .any(|(a, xs)| a == aux && xs.contains(&x.as_str()))
    });
    agrees && third_ok
}

/// ADR 0229: the positive tags an imperative takes, by the word its sentence opens with: "Let's grab lunch, shall
/// we", "Please close the door, will you / would you / can you / could you". The opener is what makes the sentence an
/// imperative, so the inverted pair after its last comma can only ask. Left out: any other opener ("Close the door,
/// will you" stays as said: an imperative can't be told from a statement or a cut-off question start by its first
/// word alone), "let me" / "let us", "shall I", "won't you" (already a [`TAG_QUESTIONS`] pair), "do it" / "have it"
/// (imperatives, not tags: "Okay, do it"). Same table in cleanup.ts.
pub const IMPERATIVE_TAGS: &[(&str, &[&str])] = &[
    ("let's", &["shall we"]),
    ("please", &["will you", "would you", "can you", "could you"]),
];

/// ADR 0229: does `head`, the last sentence up to its last comma, open with an [`IMPERATIVE_TAGS`] word ("Let's" or
/// "let's", ASCII or curly apostrophe; "Please" or "please", with or without its own comma) and hold at least one more
/// word, with `aux pronoun` (as written, lowercase) one of the tags that opener takes?
fn imperative_tag(head: &str, aux: &str, pronoun: &str) -> bool {
    let mut words = head.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    if words.next().is_none() {
        return false;
    }
    let first = first.replace('\u{2019}', "'");
    let first = first.strip_suffix(',').unwrap_or(&first);
    let opener = match first {
        "Let's" | "let's" => "let's",
        "Please" | "please" => "please",
        _ => return false,
    };
    let tag = format!("{aux} {pronoun}");
    IMPERATIVE_TAGS
        .iter()
        .any(|(o, tags)| *o == opener && tags.contains(&tag.as_str()))
}

fn rep(r: &Regex, s: &str, with: &str) -> String {
    replace_all_or_keep(r, s, with)
}

/// Bozniak (dependency review, b): fancy-regex's `replace_all` panics on a runtime error (the backtrack limit);
/// on transcript text that must never take Talk down, so a pattern that errors leaves the text as it was.
fn replace_all_or_keep<R: fancy_regex::Replacer>(r: &Regex, s: &str, with: R) -> String {
    match r.try_replacen(s, 0, with) {
        Ok(out) => out.into_owned(),
        Err(e) => {
            tracing::warn!(error = %e, "cleanup: a pattern failed at run time; text kept");
            s.to_owned()
        }
    }
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
    /// ADR 0199: the lowercased words when they are all ASCII letters and digits ([`replace_plain_term`]).
    plain: Option<Vec<String>>,
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
        let words: Vec<String> = core.split(r"[\s-]+").map(str::to_ascii_lowercase).collect();
        let plain = words
            .iter()
            .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric()))
            .then_some(words);
        let core = regex::Regex::new(&format!("(?i){core}")).ok()?;
        Some(Arc::new(DictionaryRegex { core, full, plain }))
    });
    cache.insert(core, r.clone());
    r
}

/// ADR 0199: a term whose spoken words are all plain ASCII letters and digits ("cue arts", "term number 3"): the
/// pattern `(?i)(?<![\w])w1[\s-]+w2…(?![\w])` then has one way to match at each place (a word starts with a letter or
/// digit, so the `[\s-]+` before it takes the whole run), and it is found by a scan of the lowercased text for the first
/// word. Not for a text with "ſ" or the Kelvin sign, which `(?i)` folds into "s", "k" and the word characters: there,
/// and for every other term, the full pattern runs. `words` are the lowercased words, `term` the replacement.
fn replace_plain_term(text: &str, words: &[String], term: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let (mut last, mut from) = (0, 0);
    while let Some(off) = lower[from..].find(words[0].as_str()) {
        let at = from + off;
        from = at + 1;
        if char_before(text, at).is_some_and(is_w) {
            continue;
        }
        let mut end = at + words[0].len();
        let mut whole = true;
        for w in &words[1..] {
            let gap = run_len(&text[end..], |c| c.is_whitespace() || c == '-');
            if gap == 0 || !lower[end + gap..].starts_with(w.as_str()) {
                whole = false;
                break;
            }
            end += gap + w.len();
        }
        if !whole || text[end..].chars().next().is_some_and(is_w) {
            continue;
        }
        out.push_str(&text[last..at]);
        out.push_str(term);
        last = end;
        from = end;
    }
    out.push_str(&text[last..]);
    out
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
                out = match &r.plain {
                    Some(words) if !out.contains(['\u{17F}', '\u{212A}']) => {
                        replace_plain_term(&out, words, &d.term)
                    }
                    _ => replace_all_or_keep(&r.full, &out, fancy_regex::NoExpand(&d.term)),
                };
            }
        }
    }
    out
}

/// ADR 0199: the next filler match at or after byte `from`, for the pattern
/// `(?i)(^|[\s,])(u+m+|u+h+|e+r+m+|e+r|a+h+|h+m+|m+h*m+)(?=[\s,.!?]|$),?`: (match start, filler start, filler end,
/// match end). The filler's letters are none of `\s,.!?`, so a filler is the whole run up to the next of those (or the
/// end) and that run must be one [`is_filler_word`]; the separator is nothing only at the very start.
fn next_filler(s: &str, from: usize) -> Option<(usize, usize, usize, usize)> {
    let token = |i: usize| {
        let j = i + run_len(&s[i..], |c| {
            !(c.is_whitespace() || matches!(c, ',' | '.' | '!' | '?'))
        });
        (j > i && is_filler_word(&s[i..j])).then_some(j)
    };
    let hit = |start: usize, i: usize, j: usize| {
        let end = if s[j..].starts_with(',') { j + 1 } else { j };
        Some((start, i, j, end))
    };
    if from == 0 {
        if let Some(j) = token(0) {
            return hit(0, 0, j);
        }
    }
    for (k, c) in s[from..].char_indices() {
        if c == ',' || c.is_whitespace() {
            let (start, i) = (from + k, from + k + c.len_utf8());
            if let Some(j) = token(i) {
                return hit(start, i, j);
            }
        }
    }
    None
}

/// Fillers out (ADR 0182 keeps the real words). ADR 0192: a comma right before a filler ("should, uh, ship", the
/// comma as the separator or before its space) goes with it when [`filler_takes_comma`] says so; otherwise the text
/// comes out exactly as the plain replacement (separator kept) would make it.
fn remove_fillers(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    while let Some((start, word_start, word_end, end)) = next_filler(src, last) {
        let sep = &src[start..word_start];
        let all = &src[start..end];
        out.push_str(&src[last..start]);
        if keeps_filler(&src[word_start..word_end], all, &src[..start]) {
            out.push_str(all);
        } else {
            // A ", " before the filler that no earlier match took.
            let lead = sep == " " && start > last && src[..start].ends_with(',');
            let at = if lead { start - 1 } else { start };
            if (lead || sep == ",") && filler_takes_comma(&src[..at], &src[end..]) {
                if lead {
                    out.pop();
                }
                out.push(' ');
            } else {
                out.push_str(sep);
            }
        }
        last = end;
    }
    out.push_str(&src[last..]);
    out
}

/// ADR 0235: a filler said as its own sentence ("Thanks. Um. Can you send it", "Um. Hello") loses its mark with it.
/// The filler goes anyway; its own "." / "?" / "!" (or run of them) used to stay and land on the sentence mark before it
/// ("Thanks.. Can you send it?", "Really?. I doubt it.") or open the text (". Hello."). Only when the filler starts the
/// text or follows a sentence mark and ASCII blanks, is a whole run of ASCII letters that [`is_filler_word`] takes,
/// is not a kept capital filler ([`keeps_filler`]: "UM.") and its mark run ends the text or meets ASCII blanks and
/// then, unless the filler opens the text, no lowercase letter (after "Done. Um. can you" the recogniser's lowercase
/// makes the sentence end unclear, ADR 0120). A
/// filler after a comma ("So, um. Next") and one with a comma after it ("Thanks. Um, can you") are left as they were
/// (the second is a segment cut as often as a sentence; ADR 0235 records why). The pattern
/// `(?<=^|[.!?])([ \t\n\r]+)(<filler>)[.!?]+(?=[ \t\n\r]|$)` in cleanup.ts (no lowercase letter next), the mark
/// run dropped.
fn opening_filler_marks(s: &str) -> String {
    let b = s.as_bytes();
    let blank = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r');
    let mark = |c: u8| matches!(c, b'.' | b'!' | b'?');
    let mut out = String::new();
    let (mut last, mut p) = (0, 0);
    while p < b.len() {
        if !blank(b[p]) || (p > 0 && !mark(b[p - 1])) {
            p += 1;
            continue;
        }
        let i = p + run_len(&s[p..], |c| c.is_ascii() && blank(c as u8));
        let j = i + run_len(&s[i..], |c| c.is_ascii_alphabetic());
        let k = j + run_len(&s[j..], |c| c.is_ascii() && mark(c as u8));
        let next = k + run_len(&s[k..], |c| c.is_ascii() && blank(c as u8));
        if j > i
            && k > j
            && b.get(k).is_none_or(|&c| blank(c))
            && (p == 0 || !b.get(next).is_some_and(u8::is_ascii_lowercase))
            && is_filler_word(&s[i..j])
            && !keeps_filler(&s[i..j], &s[p..j], &s[..p])
        {
            if out.is_empty() {
                out.reserve(s.len());
            }
            out.push_str(&s[last..j]);
            last = k;
            p = k;
        } else {
            p = i;
        }
    }
    if last == 0 {
        return s.to_string();
    }
    out.push_str(&s[last..]);
    out
}

/// On-device cleanup: spoken commands, fillers (ADR 0192: with the comma before them where none belongs), stutters,
/// spacing, "Scratch that" (ADR 0186), a final spoken "question mark" / "exclamation point" (ADR 0189),
/// capitalization, terminal punctuation (ADR 0101: which repeats collapse, and no "." on a short dictation; ADR 0188:
/// nor on a list of short lines; ADR 0201: whose line commas go; ADR 0206: "Hey <dictionary name>, <question>?"; ADR
/// 0227: "..., isn't it?"), then
/// spoken amounts as figures (ADR 0193: "$25", "15%", "3:30 p.m."), and a spoken date's day as a figure (ADR 0230:
/// "on October 21st").
pub fn rule_cleanup(raw: &str, dict: &[DictionaryEntry]) -> String {
    let mut t = format!(" {raw} ");
    t = layout_command(NEW_PARAGRAPH, &t, "\n\n");
    t = layout_command(NEW_LINE, &t, "\n");
    t = opening_filler_marks(&t);
    t = remove_fillers(&t);
    t = collapse_repeats(&t);
    t = collapse_blanks(&t);
    t = rep(&NL_SPACES, &t, "\n");
    t = rep(&SPACE_BEFORE_PUNCT, &t, "${1}");
    t = rep(&SPACE_AFTER_PUNCT, &t, "${1} ");
    t = rep(&COMMAS, &t, ",");
    t = rep(&EDGES, &t, "");
    // Only for a recogniser that writes capitals: an all-lowercase text's "so. i can" is its own sentence end.
    if raw.chars().any(|c| c.is_ascii_uppercase()) {
        t = continued_dot(&t);
    }
    t = scratch_that(t);
    t = final_mark(t);
    t = list_line_commas(t, raw.chars().any(|c| c.is_ascii_uppercase()));
    t = sentence_starts(&t);
    t = lone_i(&t);
    t = apply_dictionary(&t, dict);
    spoken_titles(
        spoken_dates(spoken_amounts(terminal_punctuation(t, dict)), dict),
        dict,
    )
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

/// ADR 0199: each hand-written scan against the pattern it replaced, step by step, on generated text (the chain as a
/// whole is checked against cycle 59's in tests/hardening60.rs).
#[cfg(test)]
mod scans_match_their_patterns {
    #[test]
    fn a_pattern_that_errors_at_run_time_keeps_the_text() {
        // Bozniak (dependency review, b): a backtrack-limit error is not a panic; the transcript comes back as is.
        let r = fancy_regex::RegexBuilder::new(r"(a*)*\1b")
            .backtrack_limit(10)
            .build()
            .unwrap();
        let t = "aaaaaaaaaaaaaaaaaaaaaaaaaaaac";
        assert!(
            r.try_replacen(t, 0, "x").is_err(),
            "the pattern must hit the limit"
        );
        assert_eq!(super::replace_all_or_keep(&r, t, "x"), t);
        assert_eq!(super::rep(&r, t, "x"), t);
    }

    use super::*;
    use fancy_regex::Captures;

    const PIECES: &[&str] = &[
        "new line",
        "New Line",
        "new  line",
        "renew line",
        "new liner",
        "new paragraph",
        "a new line",
        "new line,",
        "new line.",
        "um",
        "Um,",
        "UM",
        "uh",
        "er",
        "err",
        "erm",
        "hmm",
        "mm",
        "mhm",
        "mhmhm",
        "35",
        "i",
        "I",
        "i.",
        "i.e.",
        "e.g.",
        "i'",
        "the",
        "The",
        "the,",
        "you",
        "you're",
        "it,",
        "Mr.",
        "etc.",
        "p.m.",
        "so.",
        "So.",
        "go.",
        "a.b",
        "x_y",
        ".",
        ",",
        "!",
        "?",
        "'",
        "_",
        "\n",
        "\t",
        "  ",
        "\u{a0}",
        "\u{3000}",
        "\u{85}",
        "\u{17F}",
        "\u{212A}",
        "caf\u{e9}",
        "\u{e9}a",
        "日本",
        "ab",
        "AB",
        "ab-",
        "-",
        "cue arts",
        "Cue-Arts",
        "cue  - arts",
        "cue artsy",
        "the new",
        "The New",
    ];

    fn texts() -> Vec<String> {
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as usize
        };
        (0..20_000)
            .map(|_| {
                let mut s = String::new();
                for _ in 0..1 + next() % 16 {
                    s.push_str(PIECES[next() % PIECES.len()]);
                    s.push_str([" ", " ", "", ",", ", ", "\n", ". "][next() % 7]);
                }
                s
            })
            .collect()
    }

    fn check(name: &str, new: impl Fn(&str) -> String, old: impl Fn(&str) -> String) {
        for t in texts() {
            assert_eq!(new(&t), old(&t), "{name}({t:?})");
        }
    }

    #[test]
    fn layout_commands() {
        for (phrase, with) in [(NEW_PARAGRAPH, "\n\n"), (NEW_LINE, "\n")] {
            let r = re(&format!(r"(?i)\s*(?<![{W}]){phrase}(?![{W}])[,.]?\s*"));
            check(
                phrase,
                |s| layout_command(phrase, s, with),
                |s| {
                    r.replace_all(s, |c: &Captures<str>| {
                        let start = c.get(0).map_or(0, |m| m.start());
                        if ends_with_determiner(s[..start].trim_end()) {
                            c[0].to_owned()
                        } else {
                            with.to_owned()
                        }
                    })
                    .into_owned()
                },
            );
        }
    }

    #[test]
    fn fillers() {
        let r = re(r"(?i)(^|[\s,])(u+m+|u+h+|e+r+m+|e+r|a+h+|h+m+|m+h*m+)(?=[\s,.!?]|$),?");
        check("fillers", remove_fillers, |src| {
            let mut out = String::new();
            let mut last = 0;
            for c in r.captures_iter(src).flatten() {
                let m = c.get(0).unwrap();
                let sep = c.get(1).map_or("", |g| g.as_str());
                out.push_str(&src[last..m.start()]);
                if keeps_filler(&c[2], &c[0], &src[..m.start()]) {
                    out.push_str(&c[0]);
                } else {
                    let lead = sep == " " && m.start() > last && src[..m.start()].ends_with(',');
                    let at = if lead { m.start() - 1 } else { m.start() };
                    if (lead || sep == ",") && filler_takes_comma(&src[..at], &src[m.end()..]) {
                        if lead {
                            out.pop();
                        }
                        out.push(' ');
                    } else {
                        out.push_str(sep);
                    }
                }
                last = m.end();
            }
            out.push_str(&src[last..]);
            out
        });
    }

    #[test]
    fn repeats_blanks_dots_starts_and_lone_i() {
        let repeats = |s: &str| {
            REPEATS
                .replace_all(s, |c: &Captures<str>| {
                    if collapses(&c[1], &c[0]) {
                        c[1].to_owned()
                    } else {
                        c[0].to_owned()
                    }
                })
                .into_owned()
        };
        check("repeats", collapse_repeats, repeats);
        let tabs = re(r"[ \t]+");
        check("blanks", collapse_blanks, |s| {
            tabs.replace_all(s, " ").into_owned()
        });
        let dot = re(&format!(r"(?<![{W}.])([A-Za-z]+)\.(?=\s+[a-z])"));
        check("continued dot", continued_dot, |s| {
            dot.replace_all(s, |c: &Captures<str>| {
                if is_abbreviation(&c[1]) {
                    c[0].to_owned()
                } else {
                    c[1].to_owned()
                }
            })
            .into_owned()
        });
        let start = re(r"(^|(?<![A-Za-z]\.[A-Za-z])[.!?]\s+|\n)([a-z])");
        check("sentence starts", sentence_starts, |src| {
            start
                .replace_all(src, |c: &Captures<str>| {
                    let at = c.get(0).map_or(0, |m| m.start());
                    if c[1].starts_with('.') && ends_with_abbreviation(&src[..at]) {
                        c[0].to_owned()
                    } else {
                        format!("{}{}", &c[1], c[2].to_uppercase())
                    }
                })
                .into_owned()
        });
        let i = re(&format!(r"(?<![{W}])i(?![{W}])(?=[\s,!?']|\.(?![A-Za-z])|$)"));
        check("lone i", lone_i, |s| i.replace_all(s, "I").into_owned());
    }

    #[test]
    fn plain_dictionary_terms() {
        for (said, term) in [
            ("cue arts", "qrtz"),
            ("i", "I"),
            ("the new", "THE NEW"),
            ("um uh", "x"),
            ("ab", "ab ab"),
        ] {
            let words: Vec<String> = said.split(' ').map(str::to_owned).collect();
            let r = re(&format!("(?i)(?<![{W}]){}(?![{W}])", words.join(r"[\s-]+")));
            check(
                said,
                |s| {
                    if s.contains(['\u{17F}', '\u{212A}']) {
                        r.replace_all(s, fancy_regex::NoExpand(term)).into_owned()
                    } else {
                        replace_plain_term(s, &words, term)
                    }
                },
                |s| r.replace_all(s, fancy_regex::NoExpand(term)).into_owned(),
            );
        }
    }
}
