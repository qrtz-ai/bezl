//! ADR 0101, echo accuracy (docs/echo/dictation-tuning.md §4 item 4): what the rules cleanup does to what was said.
//! Each pair is the recogniser's raw text and the text that should be typed. Written by hand (not by the TS oracle),
//! so a rule change that breaks one of them fails here; spec/vectors/talk/cleanup.json carries the same raws for the
//! TS ↔ Rust parity check.
use qrtz_talk::cleanup::{cleanup_dictation, rule_cleanup, SHORT_DICTATION_WORDS};
use qrtz_talk::DictionaryEntry;

const VECTORS: &[(&str, &str)] = &[
    ("my pin is 1 1 2 3", "My pin is 1 1 2 3."),
    ("the room code is 4 4 7 2", "The room code is 4 4 7 2."),
    ("10 10", "10 10"),
    ("she had had enough by then", "She had had enough by then."),
    ("I said that that was fine", "I said that that was fine."),
    ("what it is is a bug", "What it is is a bug."),
    ("thank you, you're the best", "Thank you, you're the best."),
    ("look at it, it's broken", "Look at it, it's broken."),
    ("I love you, you know", "I love you, you know."),
    ("it was very very good", "It was very very good."),
    ("bye bye", "Bye bye"),
    ("no no no", "No no no"),
    ("Will will be there at noon", "Will will be there at noon."),
    ("come in in the morning", "Come in in the morning."),
    ("um so I I think we should", "So I think we should."),
    ("the the plan is ready", "The plan is ready."),
    ("I, I think it works", "I think it works."),
    ("we we we need to go now", "We need to go now."),
    ("The the report", "The report"),
    ("yes", "Yes"),
    ("Yes.", "Yes"),
    ("sounds good", "Sounds good"),
    ("Sounds good.", "Sounds good"),
    ("is it ready?", "Is it ready?"),
    ("great!", "Great!"),
    ("at 3 p.m.", "At 3 p.m."),
    ("wait...", "Wait..."),
    ("um yes", "Yes"),
    ("I I I I", "I"),
    ("see you friday", "See you friday"),
    ("Yes. Sure", "Yes. Sure."),
    ("four words get a period", "Four words get a period."),
    ("first line new line second", "First line\nSecond."),
    ("price 3.50", "Price 3.50"),
    ("open config.json and fix it", "Open config.json and fix it."),
    ("the site is qrtz.ai now", "The site is qrtz.ai now."),
    (
        "email jlgrimes@users.noreply.github.com today",
        "Email jlgrimes@users.noreply.github.com today",
    ),
    (
        "go to https://qrtz.ai/docs please",
        "Go to https://qrtz.ai/docs please.",
    ),
    ("use node.js, i.e. the runtime", "Use node.js, i.e. the runtime."),
    (
        "bring snacks e.g. chips or fruit",
        "Bring snacks e.g. chips or fruit.",
    ),
    (
        "meet at 3 p.m. today with the team",
        "Meet at 3 p.m. today with the team.",
    ),
    ("i think so. i can come", "I think so. I can come."),
    ("well, i guess", "Well, I guess"),
    ("red,green and blue", "Red, green and blue."),
    // ADR 0120 (N167): a segmented recogniser's "." at a cut, with the next segment lowercase, is no sentence end.
    (
        "Upon the farthest earth. removed from thee",
        "Upon the farthest earth removed from thee.",
    ),
    (
        "The germ an insignificant seed. hardly to be seen",
        "The germ an insignificant seed hardly to be seen.",
    ),
    (
        "Send it to the team. then we can ship",
        "Send it to the team then we can ship.",
    ),
    // A capitalized next word is a sentence; an abbreviation's dot stays and does not capitalize.
    (
        "Send it to the team. Then we ship",
        "Send it to the team. Then we ship.",
    ),
    (
        "Ralph Thurston, Mr. and Mrs. Smith",
        "Ralph Thurston, Mr. and Mrs. Smith.",
    ),
    (
        "Bring apples, pears, etc. and some bread",
        "Bring apples, pears, etc. and some bread.",
    ),
    ("It is Lakers vs. them tonight", "It is Lakers vs. them tonight."),
    ("Meet Dr. Lee at 3 p.m. today", "Meet Dr. Lee at 3 p.m. today."),
    ("Is it ready? then go", "Is it ready? Then go."),
    // An all-lowercase recogniser's "." is its own sentence end (the gate on capitals).
    ("i think so. i can come", "I think so. I can come."),
    ("we shipped it. now we rest", "We shipped it. Now we rest."),
];

#[test]
fn every_accuracy_vector_types_what_was_meant() {
    let bad: Vec<String> = VECTORS
        .iter()
        .filter_map(|(raw, want)| {
            let got = rule_cleanup(raw, &[]);
            (got != *want).then(|| format!("{raw:?}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "{} of {} wrong:\n{}",
        bad.len(),
        VECTORS.len(),
        bad.join("\n")
    );
}

#[test]
fn numbers_and_intentional_doubles_are_never_collapsed() {
    for raw in ["1 1 2 3", "the code is 10 10 10 then", "call 5 5 5 1 2 1 2 now"] {
        let got = rule_cleanup(raw, &[]);
        let digits = |s: &str| s.chars().filter(char::is_ascii_digit).count();
        assert_eq!(digits(&got), digits(raw), "{raw:?} -> {got:?}");
    }
    for double in [
        "had had",
        "that that",
        "is is",
        "very very",
        "bye bye",
        "no no",
        "so so",
        "there there",
    ] {
        let raw = format!("I think {double} works here");
        assert!(
            rule_cleanup(&raw, &[]).to_lowercase().contains(double),
            "{double:?} kept in {raw:?}"
        );
    }
}

#[test]
fn a_contraction_is_not_a_repeat_of_its_first_word() {
    for (raw, want) in [
        (
            "thank you, you're welcome to join",
            "Thank you, you're welcome to join.",
        ),
        ("I can, can't I see it", "I can, can't I see it."),
        ("they they're late again", "They they're late again."),
    ] {
        assert_eq!(rule_cleanup(raw, &[]), want);
    }
}

#[test]
fn short_dictations_get_no_period_and_longer_ones_do() {
    assert_eq!(SHORT_DICTATION_WORDS, 3);
    assert_eq!(rule_cleanup("ok see you then", &[]), "Ok see you then.");
    assert_eq!(rule_cleanup("see you then", &[]), "See you then");
    assert_eq!(rule_cleanup("See you then.", &[]), "See you then");
    // The dictionary runs first; a term ending in a symbol gets nothing either way.
    let dict = vec![DictionaryEntry {
        term: "C++".into(),
        aliases: vec!["see plus plus".into()],
    }];
    assert_eq!(cleanup_dictation("see plus plus", &dict, true).text, "C++");
    assert_eq!(cleanup_dictation("qrtz", &[], true).text, "Qrtz");
    // Cleanup off: as recognised, period included.
    assert_eq!(cleanup_dictation("Yes.", &[], false).text, "Yes.");
}
