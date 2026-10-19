//! ADR 0199 (echo cycle 60): the cleanup chain's look-around and backreference patterns became hand-written scans,
//! and a dictionary term of plain words is written by a scan instead of the backtracking pattern. A pure speed-up: the text out must be byte for
//! byte the cycle-59 chain's (frozen in `oracle/cleanup_c59.rs`) on every vector and on generated text, and the long
//! dictation must now run in a quarter of the 2 ms budget (it took 1.5-1.8 ms before, so this fails without the fix).
use qrtz_talk::cleanup::{apply_dictionary, cleanup_dictation, rule_cleanup};
use qrtz_talk::DictionaryEntry;
use serde_json::Value;
use std::time::Instant;

#[path = "oracle/cleanup_c59.rs"]
mod c59;

fn entry(term: &str, aliases: &[&str]) -> DictionaryEntry {
    DictionaryEntry {
        term: term.into(),
        aliases: aliases.iter().map(|a| (*a).into()).collect(),
    }
}

/// The budget test's dictionary, a few odd ones (Unicode, regex characters, one term inside another's alias), and
/// none.
fn dictionaries() -> Vec<Vec<DictionaryEntry>> {
    let mut budget = vec![
        entry("qrtz", &["cue arts", "quartz"]),
        entry("Postgres", &["post gres"]),
    ];
    for i in 0..18 {
        budget.push(entry(&format!("Term{i}"), &[&format!("term number {i}")]));
    }
    let odd = vec![
        entry("Straße", &["strasse", "STRASSE road"]),
        entry("K8s", &["kubernetes", "kates"]),
        entry("C++", &["c plus plus"]),
        entry("a.b(c)", &["a b c"]),
        entry("new line", &["newline"]),
        entry("the the", &["thee"]),
        entry("Ünïcödé", &["unicode"]),
        entry("Kelvin", &["\u{212A}elvin"]),
        entry("", &[""]),
        entry("   ", &["  "]),
        entry("3D", &["3d", "three d"]),
        entry("Q", &["q"]),
        entry("cue-arts", &["cue - arts"]),
    ];
    vec![vec![], budget, odd]
}

fn same(raw: &str, dict: &[DictionaryEntry]) {
    assert_eq!(
        rule_cleanup(raw, dict),
        c59::rule_cleanup(raw, dict),
        "rule_cleanup({raw:?})"
    );
    assert_eq!(
        apply_dictionary(raw, dict),
        c59::apply_dictionary(raw, dict),
        "apply_dictionary({raw:?})"
    );
    assert_eq!(
        cleanup_dictation(raw, dict, false).text,
        c59::cleanup_dictation(raw, dict, false).text,
        "cleanup off ({raw:?})"
    );
}

fn load(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Every raw text in cleanup.json and polish.json (polish and keep), with each dictionary and the file's own.
#[test]
fn every_vector_is_byte_identical_to_cycle_59() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/vectors/talk/");
    let mut texts: Vec<String> = Vec::new();
    let mut dicts = dictionaries();
    for (file, sets, field) in [
        (
            "cleanup.json",
            &["rules", "dictionaryOnly"][..],
            &["raw", "text"][..],
        ),
        ("polish.json", &["polish", "keep"][..], &["raw"][..]),
    ] {
        let v = load(&format!("{root}{file}"));
        dicts.push(serde_json::from_value(v["dictionary"].clone()).unwrap());
        for set in sets {
            for case in v[set].as_array().unwrap() {
                for f in field {
                    if let Some(t) = case[f].as_str() {
                        texts.push(t.to_owned());
                    }
                }
            }
        }
    }
    assert!(texts.len() > 400, "{} texts", texts.len());
    for d in &dicts {
        for t in &texts {
            same(t, d);
        }
    }
}

/// Pieces every rewritten step reacts to, and their near misses: spoken layout commands, fillers and their
/// look-alikes, repeats with commas and apostrophes, dots before lowercase words, abbreviations, lone "i", Unicode
/// whitespace and letters, dictionary words.
const PIECES: &[&str] = &[
    "new line",
    "New Line",
    "NEW LINE",
    "new  line",
    "new\tline",
    "newline",
    "renew line",
    "new liner",
    "new_line",
    "new paragraph",
    "New paragraph.",
    "a new line",
    "the new paragraph",
    "new line,",
    "new line.",
    "new line,,",
    "um",
    "Um,",
    "UM",
    "uh",
    "uhh",
    "umm",
    "er",
    "Er,",
    "err",
    "erm",
    "ermm",
    "ah",
    "ahh",
    "hmm",
    "hm",
    "mm",
    "mhm",
    "mmhmm",
    "mhmhm",
    "mm,",
    "uhm",
    "um.",
    "uh?",
    "um!",
    "hum",
    "summer",
    "35",
    "10",
    "I",
    "i",
    "i.",
    "i,",
    "i'",
    "i?",
    "i.e.",
    "e.g.",
    "the",
    "The",
    "the,",
    "THE",
    "a",
    "A",
    "an",
    "and",
    "to",
    "you",
    "you're",
    "it",
    "it's",
    "it,",
    "very",
    "very,",
    "we",
    "We",
    "so",
    "So.",
    "so.",
    "go.",
    "Mr.",
    "mr.",
    "Dr.",
    "etc.",
    "p.m.",
    "a.m.",
    "U.S.",
    "3",
    "3.",
    "dollars",
    "percent",
    "twenty",
    "five",
    "scratch that",
    "Scratch that,",
    "Scratch that.",
    "question mark",
    "exclamation point",
    "Do",
    "you",
    "want",
    "what",
    "time",
    "is",
    "Is",
    "there",
    ".",
    ",",
    ", ",
    "!",
    "?",
    "...",
    ";",
    ":",
    "\"",
    "(",
    ")",
    "-",
    "'",
    "_",
    "x_y",
    "a.b",
    "foo",
    "bar",
    "Bar",
    "BAR",
    "qrtz",
    "cue arts",
    "cue-arts",
    "Cue  Arts",
    "post gres",
    "Postgres",
    "quartz",
    "term number 3",
    "Term3",
    "strasse",
    "Straße",
    "kubernetes",
    "c plus plus",
    "unicode",
    "\u{212A}elvin",
    "Kelvin",
    "\u{17F}",
    "caf\u{e9}",
    "na\u{ef}ve",
    "\u{e9}a",
    "\u{e9}",
    "日本",
    "\u{a0}",
    "\u{3000}",
    "\u{2003}",
    "\u{85}",
    "\u{200B}",
    "\u{2028}",
    "\n",
    "\n\n",
    "\t",
    "  ",
    " , ",
    "¿",
    "Ⅰ",
    "ⅰ",
    "\u{17F}o",
    "\u{212A}",
    "kk\u{212A}",
    "kkk\u{20AC}",
    "So",
    "SO",
    "xcue arts",
    "cue artsy",
    "cue - arts",
    "cue -arts",
    "cue_arts",
    "cue\narts",
    "cue arts'",
    "CUE ARTS",
    "quartzite",
    "quartz.",
    "post-gres",
    "post\u{a0}gres",
    "kates.",
    "3d",
    "3D print",
    "term number 33",
    "term  number\t3",
    "Q",
];

/// The same small xorshift every run, so a failure names a reproducible text.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn generated(rng: &mut Rng) -> String {
    const GLUE: &[&str] = &[" ", " ", " ", " ", "", ",", ", ", "  ", "\n", ". ", "\t", " ,"];
    let mut s = String::new();
    for _ in 0..1 + rng.below(24) {
        s.push_str(PIECES[rng.below(PIECES.len())]);
        s.push_str(GLUE[rng.below(GLUE.len())]);
    }
    if rng.below(3) == 0 {
        s = s.trim().to_owned();
    }
    s
}

#[test]
fn generated_text_is_byte_identical_to_cycle_59() {
    let dicts = dictionaries();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    // QRTZ_HARDENING60_N=<count> runs a longer search (cycle 60 ran 3,000,000 in release).
    let n = std::env::var("QRTZ_HARDENING60_N")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(if cfg!(debug_assertions) { 6_000 } else { 60_000 });
    for i in 0..n {
        let t = generated(&mut rng);
        same(&t, &dicts[i % dicts.len()]);
    }
}

/// The budget test's long dictation (cleanup_budget.rs), timed the same way.
fn long_dictation() -> String {
    let parts = [
        "Um, so I I think we should, uh, ship cue arts echo on Friday.",
        "The the build is green. and the docs are almost done",
        "I spent the night in the ER, so the review moved, um, to Monday.",
        "Use a 35 mm lens new line bring the charger new paragraph",
        "We launched our new line in May, and the numbers look good, hmm, better than last year.",
        "Email jlgrimes@users.noreply.github.com or go to https://qrtz.ai/docs for details. i.e. the setup guide",
        "Mr. and Mrs. Smith will join at 3 p.m. today with the team, etc. and post gres is migrated.",
        "Book the room for Monday. Scratch that, book it for Tuesday. My back itches. Scratch that itch.",
        "The demo is, um, basically done and, uh, we think the, um, docs are ready. If we can, um, we ship.",
        "It costs twenty five dollars, up fifteen percent, at three thirty p.m. Between ten and twenty dollars.",
        "Thanks for the update. Is the demo still on question mark",
    ];
    let mut s = String::new();
    while s.split_whitespace().count() < 400 {
        for p in parts {
            s.push_str(p);
            s.push(' ');
        }
    }
    s
}

/// A quarter of the 2 ms chain budget in the release build (0.11 ms on the box at cycle 60, 1.5-1.8 ms before). A
/// debug build only checks the text (its timings mean nothing).
#[test]
fn the_long_dictation_runs_in_a_quarter_of_the_budget() {
    let text = long_dictation();
    let dict = dictionaries().swap_remove(1);
    assert_eq!(
        cleanup_dictation(&text, &dict, true).text,
        c59::cleanup_dictation(&text, &dict, true).text
    );
    if cfg!(debug_assertions) {
        return;
    }
    for _ in 0..5 {
        cleanup_dictation(&text, &dict, true);
    }
    let mut ms: Vec<f64> = (0..41)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(cleanup_dictation(std::hint::black_box(&text), &dict, true));
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    let median = ms[ms.len() / 2];
    println!(
        "cleanup on {} words: median {median:.3} ms (ceiling 0.5 ms)",
        text.split_whitespace().count()
    );
    assert!(
        median <= 0.5,
        "cleanup took {median:.3} ms (median); ADR 0199 ceiling 0.5 ms"
    );
}
