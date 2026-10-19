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

/// The terminal punctuation (ADR 0101): a short dictation ends as said, without a period; a longer one gets a "."
/// when it ends on a word. ADR 0188: a list of short lines ends like a short dictation: no "." is added, and a "."
/// the recogniser ended it with is dropped only when it is a clean sentence end (not "p.m.", "etc.", "...").
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

/// On-device cleanup: spoken commands, fillers, stutters, spacing, "Scratch that" (ADR 0186), a final spoken "question
/// mark" / "exclamation point" (ADR 0189), capitalization, terminal punctuation (ADR 0101: which repeats collapse, and
/// no "." on a short dictation; ADR 0188: nor on a list of short lines).
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
    t = scratch_that(t);
    t = final_mark(t);
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
