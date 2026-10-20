#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0207 (echo cycle 67): ADR 0205's subject stutter also collapses before a negated modal or "cannot"
//! ("So we, we can't make it" → "So we can't make it."). ASCII and curly apostrophes; "candidly" and other lookalikes
//! stay. Fails without the fix (every positive kept "we, we" / "they, they").
use qrtz_talk::cleanup::rule_cleanup;
use qrtz_talk::DictionaryEntry;

fn clean(raw: &str) -> String {
    let dict = [DictionaryEntry {
        term: "qrtz".into(),
        aliases: vec!["cue arts".into()],
    }];
    rule_cleanup(raw, &dict)
}

#[test]
fn a_subject_stutter_before_a_negated_modal_collapses() {
    for (raw, want) in [
        ("So we, we can't make it.", "So we can't make it."),
        ("So we, we cannot make it.", "So we cannot make it."),
        ("We, we won't be there tonight.", "We won't be there tonight."),
        (
            "They, they shouldn't wait any longer.",
            "They shouldn't wait any longer.",
        ),
        (
            "But we, we couldn't finish on time.",
            "But we couldn't finish on time.",
        ),
        (
            "And they, they wouldn't agree to that.",
            "And they wouldn't agree to that.",
        ),
        (
            "Because we, we mustn't tell anyone yet.",
            "Because we mustn't tell anyone yet.",
        ),
        (
            "Then they, they mightn't come at all.",
            "Then they mightn't come at all.",
        ),
        ("So we, we mayn't need it.", "So we mayn't need it."),
        ("So, we, we can't ship it today.", "So, we can't ship it today."),
        (
            "I'm sorry. So we, we can't make the dinner.",
            "I'm sorry. So we can't make the dinner.",
        ),
        (
            "So they, they won't call you back.",
            "So they won't call you back.",
        ),
        (
            "Notes new line we, we can't finish today",
            "Notes\nWe can't finish today.",
        ),
        ("Um, so we, we cannot join", "So we cannot join."),
        // curly apostrophe (U+2019)
        ("So we, we can\u{2019}t make it.", "So we can\u{2019}t make it."),
        (
            "But they, they won\u{2019}t be ready.",
            "But they won\u{2019}t be ready.",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn negated_modal_lookalikes_and_non_starts_are_kept() {
    for (raw, want) in [
        // "cannot" must not fire on "candidly"; missing/typo apostrophe forms stay
        ("So we, we candidly told them.", "So we, we candidly told them."),
        (
            "So we, we canned the whole project.",
            "So we, we canned the whole project.",
        ),
        ("So we, we cant make it.", "So we, we cant make it."),
        ("So we, we cann't make it.", "So we, we cann't make it."),
        ("So we, we Cannot make it.", "So we, we Cannot make it."),
        ("So we, we CAN'T make it.", "So we, we CAN'T make it."),
        ("So we, we can'tt make it.", "So we, we can'tt make it."),
        ("So we, we shan't make it.", "So we, we shan't make it."),
        ("So we, we're not going.", "So we, we're not going."),
        ("So we, we went home early.", "So we, we went home early."),
        // not at a sentence start
        (
            "Neither have we, we can't wait forever.",
            "Neither have we, we can't wait forever.",
        ),
        (
            "So do we, we can't ever say it.",
            "So do we, we can't ever say it.",
        ),
        (
            "You and we, we can't share the cost.",
            "You and we, we can't share the cost.",
        ),
        ("Tell them we, we can't do it.", "Tell them we, we can't do it."),
        ("And then we, we can't see.", "And then we, we can't see."),
        ("Okay we, we can't do that.", "Okay we, we can't do that."),
        ("Well we, we can't stay.", "Well we, we can't stay."),
        ("Hey we, we can't leave yet.", "Hey we, we can't leave yet."),
        (
            "Where are they, they can't be far.",
            "Where are they, they can't be far.",
        ),
        (
            "They're faster than we, we can't keep up.",
            "They're faster than we, we can't keep up.",
        ),
        // other pronouns / separators
        ("Thank you, you can't go yet.", "Thank you, you can't go yet."),
        ("So he, he can't drive.", "So he, he can't drive."),
        ("So we, we, we can't ship it.", "So we, we, we can't ship it."),
        ("So we,  we can't ship it.", "So we, we can't ship it."),
        ("We, WE can't do it.", "We, WE can't do it."),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}
