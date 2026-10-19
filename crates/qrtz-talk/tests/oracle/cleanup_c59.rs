//! ADR 0199: the cleanup chain exactly as it was at d409ede (cycle 59), frozen as the oracle that the faster chain
//! must match byte for byte (tests/hardening60.rs). Not built into the app; do not edit except to keep it compiling.
//!
//! The on-device cleanup of a dictation (`packages/core/src/talk/cleanup.ts`'s rules). There is no LLM cleanup:
//! it was removed for speed (ADR 0071), so the text goes in as soon as the final ASR text is in. The regexes are the JS ones with JS semantics spelled out: `\b` and `\w`
//! are ASCII in JS, so they become explicit ASCII lookarounds/classes here. Checked byte for byte against TS by
//! spec/vectors/talk/cleanup.json.
#![allow(dead_code)]
use fancy_regex::{Captures, Regex};
use qrtz_talk::contract::{CleanupMethod, DictionaryEntry};
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
// The filler word is group 2; whether it goes is [`keeps_filler`]'s call (ADR 0182), and whether a comma before it
// goes too is [`filler_takes_comma`]'s (ADR 0192).
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
    r.replace_all(s, |c: &Captures<str>| {
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

/// ADR 0196: does the dictation end in an unpunctuated sentence that can only be a question? Its opening is a yes/no
/// question's ([`yes_no_opening`], the auxiliary capitalised as a sentence start) or a wh-question's, it has 3 to
/// [`QUESTION_MAX_WORDS`] words, no comma, colon, semicolon, quote, bracket or apostrophe, and nothing after the
/// opening that starts another clause ([`LATER_CLAUSE`]). Also refused: "ever" unless the verb is "have"/"has"
/// ("Is it ever hot", "Did I ever love that car" are exclamations), "were" with a "to" ("Were we to lose"), "just
/// say" ("Can I just say"), "What do you know", "Have I got" ("Have I got news for you"), and the spoken name of a
/// mark ("question mark", "exclamation"): ADR 0189's sentences stay exactly as that rule leaves them.
fn unmarked_question(t: &str) -> bool {
    let line = &t[t.rfind('\n').map_or(0, |i| i + 1)..];
    let s = match INNER_END.find_iter(line).flatten().last() {
        Some(m) => &line[m.end()..],
        None => line,
    };
    if s.contains([',', ';', ':', '"', '(', ')', '\'', '\u{2019}']) {
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
    let (aux_at, opening) = if WH_WORDS.contains(&w[0]) {
        let aux_at = if WH_MODIFIERS.contains(&w[1]) { 2 } else { 1 };
        let ok = aux_at + 1 < w.len()
            && WH_AUX.contains(&w[aux_at])
            && WH_SUBJECTS.contains(&w[aux_at + 1])
            && !(w[aux_at] == "did" && w[aux_at + 1] == "it")
            && w[..] != ["what", "do", "you", "know"];
        (aux_at, ok)
    } else {
        (0, yes_no_opening(w[0], w[1], w.get(2).copied().unwrap_or("")))
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
/// the mark added is "?" when the last sentence can only be a question ([`unmarked_question`]).
fn terminal_punctuation(mut t: String) -> String {
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
        let mark = if unmarked_question(&t) { '?' } else { '.' };
        t.push(mark);
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

/// Fillers out (ADR 0182 keeps the real words). ADR 0192: a comma right before a filler ("should, uh, ship", the
/// comma as the separator or before its space) goes with it when [`filler_takes_comma`] says so; otherwise the text
/// comes out exactly as the plain replacement (separator kept) would make it.
fn remove_fillers(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    for c in FILLERS.captures_iter(src).flatten() {
        let m = c.get(0).expect("whole match");
        let sep = c.get(1).map_or("", |g| g.as_str());
        out.push_str(&src[last..m.start()]);
        if keeps_filler(&c[2], &c[0], &src[..m.start()]) {
            out.push_str(&c[0]);
        } else {
            // A ", " before the filler that no earlier match took.
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
}

/// On-device cleanup: spoken commands, fillers (ADR 0192: with the comma before them where none belongs), stutters,
/// spacing, "Scratch that" (ADR 0186), a final spoken "question mark" / "exclamation point" (ADR 0189),
/// capitalization, terminal punctuation (ADR 0101: which repeats collapse, and no "." on a short dictation; ADR 0188:
/// nor on a list of short lines), then spoken amounts as figures (ADR 0193: "$25", "15%", "3:30 p.m.").
pub fn rule_cleanup(raw: &str, dict: &[DictionaryEntry]) -> String {
    let mut t = format!(" {raw} ");
    t = layout_command(&NEW_PARAGRAPH, &t, "\n\n");
    t = layout_command(&NEW_LINE, &t, "\n");
    t = remove_fillers(&t);
    t = REPEATS
        .replace_all(&t, |c: &Captures<str>| {
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
            .replace_all(&t, |c: &Captures<str>| {
                if is_abbreviation(&c[1]) {
                    c[0].to_owned()
                } else {
                    c[1].to_owned()
                }
            })
            .into_owned();
    }
    t = scratch_that(t);
    t = final_mark(t);
    let src = t.clone();
    t = SENTENCE_START
        .replace_all(&src, |c: &Captures<str>| {
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
    spoken_amounts(terminal_punctuation(t))
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
