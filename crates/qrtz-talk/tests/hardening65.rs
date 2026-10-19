//! ADR 0205 (echo cycle 65): a subject stutter across a comma, "we, we" / "they, they", collapses only when the
//! first pronoun opens its sentence (alone or after one of so/and/but/then/because) and a plain modal follows the
//! second ("So we, we can ship it tomorrow" → "So we can ship it tomorrow."). Everything else keeps both words.
//! Fails without the fix (every positive kept "we, we" / "they, they").
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
fn a_subject_stutter_before_a_modal_collapses() {
    for (raw, want) in [
        ("So we, we can ship it tomorrow.", "So we can ship it tomorrow."),
        ("We, we can try again tomorrow.", "We can try again tomorrow."),
        ("They, they should be here by now.", "They should be here by now."),
        ("But we, we could ask Sam first.", "But we could ask Sam first."),
        (
            "And they, they will send the invoice.",
            "And they will send the invoice.",
        ),
        (
            "Because they, they might not come.",
            "Because they might not come.",
        ),
        (
            "Then we, we would need a new room.",
            "Then we would need a new room.",
        ),
        ("So, we, we can ship it tomorrow.", "So, we can ship it tomorrow."),
        (
            "I'm tired. So we, we should go home.",
            "I'm tired. So we should go home.",
        ),
        ("Um, so we, we can ship it", "So we can ship it."),
        ("so we, we can ship cue arts", "So we can ship qrtz."),
        (
            "Notes new line we, we must finish today",
            "Notes\nWe must finish today.",
        ),
        (
            "Is it done? They, they may need more time.",
            "Is it done? They may need more time.",
        ),
        ("So we, we shall see.", "So we shall see."),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn every_other_subject_repeat_is_kept() {
    for (raw, want) in [
        // not at a sentence start: an inversion, an ellipsis, a comparison, a compound subject, an object
        ("Neither have we, we were busy.", "Neither have we, we were busy."),
        (
            "Neither can we, we can only wait.",
            "Neither can we, we can only wait.",
        ),
        (
            "So do we, we just never say it.",
            "So do we, we just never say it.",
        ),
        (
            "So can they, they can come too.",
            "So can they, they can come too.",
        ),
        (
            "They're faster than we, we can keep up.",
            "They're faster than we, we can keep up.",
        ),
        (
            "You and we, we can share the cost.",
            "You and we, we can share the cost.",
        ),
        ("Tell them we, we can do it.", "Tell them we, we can do it."),
        ("And then we, we will see.", "And then we, we will see."),
        ("Okay we, we can do that.", "Okay we, we can do that."),
        (
            "Where are they, they can't be far.",
            "Where are they, they can't be far.",
        ),
        // no plain modal after the second pronoun
        ("So we, we went home early.", "So we, we went home early."),
        ("So we, we can't make it.", "So we, we can't make it."),
        ("So we, we cannot make it.", "So we, we cannot make it."),
        ("So we, we candidly told them.", "So we, we candidly told them."),
        ("So they, they May be back.", "So they, they May be back."),
        ("So we, we're going home.", "So we, we're going home."),
        ("We, we.", "We, we"),
        // not exactly one ", " between the two
        ("So we, we, we can ship it.", "So we, we, we can ship it."),
        ("So we,  we can ship it.", "So we, we can ship it."),
        // other pronouns across a comma stay a clause boundary
        ("Thank you, you can go now.", "Thank you, you can go now."),
        ("I love you, you know.", "I love you, you know."),
        ("Ask him, he can help.", "Ask him, he can help."),
        ("Take it, it will help.", "Take it, it will help."),
        ("So he, he can drive.", "So he, he can drive."),
        // names and code that look like the pattern
        ("We, WE can do it.", "We, WE can do it."),
        ("Set they, they_can to true.", "Set they, they_can to true."),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}
