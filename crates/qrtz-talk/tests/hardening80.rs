#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0232 (echo cycle 80): an unmarked question that opens with a contracted wh-word ("What's", "Where's",
//! "How'd", "What're", "When'll", "What've") ends with "?" like its spelled-out form ("What is the plan for
//! tonight?"). The contraction is the only apostrophe allowed, it is read as the wh-word and its auxiliary, and its
//! subject must agree with it. Fails without the fix (every positive got "."); the refusals pass on both.
use qrtz_talk::cleanup::{rule_cleanup, WH_CONTRACTIONS};
use qrtz_talk::DictionaryEntry;

fn clean(raw: &str) -> String {
    rule_cleanup(raw, &[])
}

#[test]
fn a_question_opening_with_a_wh_contraction_ends_with_a_question_mark() {
    for (raw, want) in [
        ("What's the plan for tonight", "What's the plan for tonight?"),
        (
            "Where's the meeting this afternoon",
            "Where's the meeting this afternoon?",
        ),
        ("How's the new job going", "How's the new job going?"),
        (
            "When's the next train to the city",
            "When's the next train to the city?",
        ),
        (
            "Who's the lead on this project",
            "Who's the lead on this project?",
        ),
        ("Why's the build so slow today", "Why's the build so slow today?"),
        ("How'd it go with the client", "How'd it go with the client?"),
        ("Where'd you put the charger", "Where'd you put the charger?"),
        (
            "What're your plans for the weekend",
            "What're your plans for the weekend?",
        ),
        (
            "Who're they meeting with today",
            "Who're they meeting with today?",
        ),
        ("What've you been up to lately", "What've you been up to lately?"),
        (
            "When'll it be ready for review",
            "When'll it be ready for review?",
        ),
        (
            "Where's the guy who called you",
            "Where's the guy who called you?",
        ),
        // A curly apostrophe, as some recogniser builds write it.
        (
            "What\u{2019}s the best way to get there",
            "What\u{2019}s the best way to get there?",
        ),
        // After an opener (ADR 0203), with ADR 0204's comma after "Hey".
        (
            "So what's the plan for tomorrow",
            "So what's the plan for tomorrow?",
        ),
        (
            "So, what's the plan for tonight",
            "So, what's the plan for tonight?",
        ),
        ("And how'd the meeting go", "And how'd the meeting go?"),
        (
            "Hey what's the status on the release",
            "Hey, what's the status on the release?",
        ),
        (
            "Okay, how's the weather out there",
            "Okay, how's the weather out there?",
        ),
        // Only the last sentence.
        (
            "I just got back. What's the plan now",
            "I just got back. What's the plan now?",
        ),
        ("what's the plan for tonight", "What's the plan for tonight?"),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn a_statement_or_an_unsure_shape_keeps_its_period() {
    for raw in [
        // Free relatives: the wh-clause is the subject of a later verb.
        "What's done is done and we move on",
        "What's left needs to go in the fridge",
        "What's more we saved a lot of money",
        "What's important is that we ship on time",
        "What's the right answer depends on the context",
        "When's the launch matters less than the quality",
        "What's the best plan really varies by team",
        "What's the plan depends on the weather",
        "What's your loss is my gain",
        "What's on the agenda is the budget",
        "How's it going is what I always ask",
        "What's it like to be you is the question",
        "Where's the line between work and life is a good question",
        "Who's coming is up to you",
        "Who's on call has the pager this week",
        "What's she said is true",
        // The placeholder name ("his" / "her" are not subjects here).
        "What's his name called me this morning",
        "What's her name from accounting stopped by",
        "What's-his-name called me this morning",
        // Not a sentence-opening wh-contraction.
        "Here's what's happening with the launch",
        "That's what's up with the build",
        "She's what's holding the team together",
        "Whatever's left goes to the food bank",
        "Let's see what's left in the budget",
        "He's the lead on this project",
        "It's the plan for tonight",
        "Well what's the plan for tonight",
        "Where's Waldo is my favorite book",
        "What's mine stays mine",
        // Shapes left out on purpose (ADR 0232): no subject after it, a second apostrophe,
        // a comma, a second clause, "ever", subject agreement, two contractions in one word.
        "Who's coming to the party on Saturday",
        "What\u{2019}s up with that",
        "What's the deal with Sam's car",
        "What's the plan, Sam",
        "What's the plan we talked about yesterday",
        "What're you doing tonight I'm bored",
        "Why's he ever late to the standup",
        "What's they doing over there",
        "Where've he gone this time",
        "What's'll the plan for tonight",
        "I asked. What'd you know",
    ] {
        let got = clean(raw);
        assert!(!got.ends_with('?'), "{raw:?} -> {got:?}");
    }
    // The recogniser's own mark and a short dictation stay as they were.
    assert_eq!(
        clean("What's the plan for tonight."),
        "What's the plan for tonight."
    );
    assert_eq!(
        clean("Where's the meeting this afternoon!"),
        "Where's the meeting this afternoon!"
    );
    assert_eq!(clean("What's the plan"), "What's the plan");
    assert_eq!(clean("What'd you know"), "What'd you know");
}

#[test]
fn each_contraction_takes_only_the_subjects_that_agree_with_it() {
    let subjects = [
        "i", "you", "we", "they", "he", "she", "it", "the", "this", "that", "my", "your", "our", "their",
    ];
    for (tail, aux, agree) in WH_CONTRACTIONS {
        for s in subjects {
            let raw = format!("Where'{tail} {s} going to stay tonight");
            let want = agree.contains(&s);
            assert_eq!(clean(&raw).ends_with('?'), want, "{raw:?} ({aux})");
        }
    }
}

#[test]
fn a_name_from_the_dictionary_before_it_stays_as_adr_0206_left_it() {
    let dict = [DictionaryEntry {
        term: "Priya".into(),
        aliases: vec![],
    }];
    for raw in [
        "Hey Priya what's the plan for today",
        "Hey Priya where's the meeting today",
    ] {
        assert_eq!(rule_cleanup(raw, &dict), format!("{raw}."), "{raw:?}");
    }
    // The spelled-out form still takes ADR 0206's comma and "?".
    assert_eq!(
        rule_cleanup("Hey Priya where is the meeting today", &dict),
        "Hey Priya, where is the meeting today?"
    );
}
