#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0229 (echo cycle 78): an imperative that opens with "Let's" or "Please" and ends in its positive tag, with no
//! mark of its own, ends with "?" instead of the "." the terminal step used to add: "Let's grab lunch, shall we" →
//! "Let's grab lunch, shall we?", "Please close the door, will you" → "…, will you?". Only "Let's … , shall we" and
//! "Please … , will / would / can / could you", as the last words of the last sentence. Fails without the fix (every
//! positive came out with "."); the refusals pass on both.
use qrtz_talk::cleanup::rule_cleanup;

fn clean(raw: &str) -> String {
    rule_cleanup(raw, &[])
}

#[test]
fn an_imperative_with_its_tag_ends_with_a_question_mark() {
    for (raw, want) in [
        (
            "Let's grab lunch after the standup, shall we",
            "Let's grab lunch after the standup, shall we?",
        ),
        (
            "Let's push the budget sync to Friday, shall we",
            "Let's push the budget sync to Friday, shall we?",
        ),
        (
            "Please close the door on your way out, will you",
            "Please close the door on your way out, will you?",
        ),
        (
            "Please send me the slides before the call, would you",
            "Please send me the slides before the call, would you?",
        ),
        (
            "Please keep an eye on the build tonight, can you",
            "Please keep an eye on the build tonight, can you?",
        ),
        (
            "Please forward this to Priya, could you",
            "Please forward this to Priya, could you?",
        ),
        (
            "Thanks for the notes. Let's pick this up tomorrow, shall we",
            "Thanks for the notes. Let's pick this up tomorrow, shall we?",
        ),
        (
            "Let\u{2019}s call it a day, shall we",
            "Let\u{2019}s call it a day, shall we?",
        ),
        (
            "Please, double check the numbers before Friday, will you",
            "Please, double check the numbers before Friday, will you?",
        ),
        (
            "Please pay the twenty dollars at the front desk, would you",
            "Please pay the $20 at the front desk, would you?",
        ),
        ("Let's go, shall we", "Let's go, shall we?"),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn other_openers_orders_and_marks_keep_their_period() {
    for (raw, want) in [
        (
            "Close the door for me, will you",
            "Close the door for me, will you.",
        ),
        (
            "Let's see how it goes, we shall",
            "Let's see how it goes, we shall.",
        ),
        (
            "Let's meet at noon if we can, we will",
            "Let's meet at noon if we can, we will.",
        ),
        (
            "Please stop, you will hurt yourself",
            "Please stop, you will hurt yourself.",
        ),
        (
            "Please be patient, you will get it",
            "Please be patient, you will get it.",
        ),
        (
            "Please send the report by Friday, you will",
            "Please send the report by Friday, you will.",
        ),
        (
            "Please, Will, you have to see this",
            "Please, Will, you have to see this.",
        ),
        (
            "Please call me when you can, you know",
            "Please call me when you can, you know.",
        ),
        ("Please do what you will", "Please do what you will."),
        (
            "Please share the doc, will you. Thanks",
            "Please share the doc, will you. Thanks.",
        ),
        ("Please send it over, will you!", "Please send it over, will you!"),
        ("Let's ship it, shall we.", "Let's ship it, shall we."),
        (
            "Please don't, you will regret it",
            "Please don't, you will regret it.",
        ),
        ("Let's be honest, we can't", "Let's be honest, we can't."),
        (
            "Please, if you can, you should come",
            "Please, if you can, you should come.",
        ),
        (
            "Please let me know whether you will",
            "Please let me know whether you will.",
        ),
        (
            "Let's wait and see if they will",
            "Let's wait and see if they will.",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}
