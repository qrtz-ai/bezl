//! ADR 0227 (echo cycle 76): a dictation that ends in a tag question with no mark of its own ends with "?" instead of
//! the "." the terminal step used to add: "It's a nice day, isn't it" → "It's a nice day, isn't it?". Only ", " then a
//! negative contraction and the pronoun it agrees with as a tag (or "don't you think" / "don't you agree" / "wouldn't
//! you say / agree / think"), as the last words of the last sentence. Fails without the fix (every positive came out
//! with "."); the refusals pass on both.
use qrtz_talk::cleanup::rule_cleanup;

fn clean(raw: &str) -> String {
    rule_cleanup(raw, &[])
}

#[test]
fn an_unmarked_tag_question_ends_with_a_question_mark() {
    for (raw, want) in [
        ("It's a nice day, isn't it", "It's a nice day, isn't it?"),
        (
            "You finished the report already, didn't you",
            "You finished the report already, didn't you?",
        ),
        (
            "We should leave before the traffic gets bad, shouldn't we",
            "We should leave before the traffic gets bad, shouldn't we?",
        ),
        (
            "They're still coming to the dinner tonight, aren't they",
            "They're still coming to the dinner tonight, aren't they?",
        ),
        (
            "There's still time to fix the build, isn't there",
            "There's still time to fix the build, isn't there?",
        ),
        (
            "Thanks for checking. That was the last blocker, wasn't it",
            "Thanks for checking. That was the last blocker, wasn't it?",
        ),
        (
            "It's too late to change the venue now, don't you think",
            "It's too late to change the venue now, don't you think?",
        ),
        (
            "I was right about the date, wasn't I",
            "I was right about the date, wasn't I?",
        ),
        // Curly apostrophe, a last line, spoken amounts, the three-word forms.
        (
            "It's a nice day, isn\u{2019}t it",
            "It's a nice day, isn\u{2019}t it?",
        ),
        (
            "Thanks\nIt works now, doesn't it",
            "Thanks\nIt works now, doesn't it?",
        ),
        (
            "It costs twenty five dollars, doesn't it",
            "It costs $25, doesn't it?",
        ),
        (
            "That's the whole plan, wouldn't you say",
            "That's the whole plan, wouldn't you say?",
        ),
        (
            "Sit down for a minute, won't you",
            "Sit down for a minute, won't you?",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn anything_else_keeps_the_period_or_its_own_mark() {
    for (raw, want) in [
        // Pronoun before the auxiliary: an elliptical statement.
        (
            "Maybe they will, maybe they won't",
            "Maybe they will, maybe they won't.",
        ),
        (
            "She said she would come, but she didn't",
            "She said she would come, but she didn't.",
        ),
        ("As far as I know, it isn't", "As far as I know, it isn't."),
        // The tag words start a clause, or no pronoun follows.
        (
            "Whatever happens, don't you worry",
            "Whatever happens, don't you worry.",
        ),
        (
            "If it rains, don't you dare cancel",
            "If it rains, don't you dare cancel.",
        ),
        (
            "We tried it twice, didn't work",
            "We tried it twice, didn't work.",
        ),
        // Not taken as tags: positive, "right", "okay", no comma, a capital, no agreement, "that", a name after.
        ("Go up, down, left, right", "Go up, down, left, right."),
        ("I'll do it, okay", "I'll do it, okay."),
        (
            "Close the door for me, will you",
            "Close the door for me, will you.",
        ),
        ("It's a nice day isn't it", "It's a nice day isn't it."),
        ("It's a nice day, Isn't it", "It's a nice day, Isn't it."),
        (
            "The twins are here, isn't they",
            "The twins are here, isn't they.",
        ),
        ("That one works, don't it", "That one works, don't it."),
        ("It's late already, isn't that", "It's late already, isn't that."),
        (
            "They're coming tonight, aren't they Sam",
            "They're coming tonight, aren't they Sam.",
        ),
        (
            "That's the plan, don't you say",
            "That's the plan, don't you say.",
        ),
        // A mark the recogniser wrote, a short dictation, an earlier sentence or line, a semicolon or quote.
        ("It's a nice day, isn't it.", "It's a nice day, isn't it."),
        ("It's a great song, isn't it!", "It's a great song, isn't it!"),
        ("Nice, isn't it", "Nice, isn't it"),
        (
            "We won the game, didn't we. Then we celebrated",
            "We won the game, didn't we. Then we celebrated.",
        ),
        (
            "It works now, doesn't it\nThanks",
            "It works now, doesn't it\nThanks.",
        ),
        (
            "We need to talk; it's late, isn't it",
            "We need to talk; it's late, isn't it.",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}
