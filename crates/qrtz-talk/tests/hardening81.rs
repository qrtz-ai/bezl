#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0233 (echo cycle 81): an unmarked question whose auxiliary is contracted onto the wh-word's modifier ("What
//! time's the meeting tomorrow", "How long'll it take", "How long've you been waiting") ends with "?" like its
//! spelled-out form ("What time is the meeting tomorrow?"). Only "what" + time / day / size and "how" + long / much /
//! often / far / old / soon, with ADR 0232's tails and subject agreement and its embedded-predicate refusal. Fails
//! without the fix (every positive got "."); the refusals pass on both.
use qrtz_talk::cleanup::{rule_cleanup, WH_CONTRACTIONS, WH_MODIFIER_CONTRACTIONS};
use qrtz_talk::DictionaryEntry;

fn clean(raw: &str) -> String {
    rule_cleanup(raw, &[])
}

#[test]
fn a_question_with_its_auxiliary_contracted_onto_the_modifier_ends_with_a_question_mark() {
    for (raw, want) in [
        (
            "What time's the meeting tomorrow",
            "What time's the meeting tomorrow?",
        ),
        (
            "How long's the flight to Denver",
            "How long's the flight to Denver?",
        ),
        (
            "How long'll it take to get there",
            "How long'll it take to get there?",
        ),
        (
            "How much'll it cost to fix the car",
            "How much'll it cost to fix the car?",
        ),
        (
            "How long've you been waiting here",
            "How long've you been waiting here?",
        ),
        (
            "How far's the drive from the airport",
            "How far's the drive from the airport?",
        ),
        ("How old's your daughter now", "How old's your daughter now?"),
        (
            "What day's the party this week",
            "What day's the party this week?",
        ),
        (
            "How much'd you pay for the tickets",
            "How much'd you pay for the tickets?",
        ),
        (
            "What time'd you get home last night",
            "What time'd you get home last night?",
        ),
        (
            "Hey what time's the dinner tonight",
            "Hey, what time's the dinner tonight?",
        ),
        (
            "So how long's the drive from here",
            "So how long's the drive from here?",
        ),
        (
            "I just parked. How long'll you be",
            "I just parked. How long'll you be?",
        ),
        (
            "What time\u{2019}s the game on Sunday",
            "What time\u{2019}s the game on Sunday?",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn a_statement_or_an_unsure_shape_keeps_its_period() {
    for raw in [
        // Statements: the wh-clause is the subject of a later verb, a second clause, or no wh-word first.
        "What time's best depends on your schedule",
        "What time's the meeting depends on the client",
        "How long's the flight matters less than the price",
        "How old's the house doesn't matter to me",
        "What day's the deadline is up to the manager",
        "How far's the drive will decide where we stay",
        "How much's the ticket varies by the day",
        "How long's the line is the real question",
        "How much's the rent determines where we live",
        "How far's the hotel changes the whole plan",
        "What time's the show remains a mystery",
        "How soon's the launch seems up in the air",
        "How long's the meeting has nothing to do with it",
        "What time's the train makes no difference",
        "How old's your car tells you a lot about it",
        "That time's the best one for me",
        "This time's going to be different",
        "Next time's on me for sure",
        "Last time's meeting ran over by an hour",
        "Any time's fine with me this week",
        "When time's up we stop writing",
        "Half time's the best part of the game",
        "Dinner time's at seven for the kids",
        "Show time's at eight tonight",
        "Some time's better than no time at all",
        "Every day's a new chance to get it right",
        "She asked how long's the drive",
        // Left out on purpose (ADR 0233): no subject after it, a comma, a second apostrophe, a modifier not
        // listed for that wh-word, "ever", "Well", subject agreement, 'm, two contractions in one word.
        "How much's left in the budget",
        "How long's left on the timer",
        "What time's good for you",
        "How long's it been since we talked",
        "What time's the meeting, Sam",
        "What size's the problem we're facing",
        "What else's on the list today",
        "What kind's the best for us",
        "How long's he ever been late",
        "Who time's the meeting tomorrow",
        "Why long's the meeting tomorrow",
        "What time's'll the meeting be",
        "How long're he staying with us",
        "How long've she been waiting here",
        "What time's they arriving tonight",
        "Well what time's the meeting tomorrow",
        "How time's the meeting tomorrow",
        "What long's the flight to Denver",
        "How long's the flight to Denver is that right",
        "How long'm I supposed to wait here",
    ] {
        let got = clean(raw);
        assert!(!got.ends_with('?'), "{raw:?} -> {got:?}");
    }
    // The recogniser's own mark and a short dictation stay as they were.
    assert_eq!(
        clean("What time's the meeting tomorrow."),
        "What time's the meeting tomorrow."
    );
    assert_eq!(
        clean("How long's the flight to Denver!"),
        "How long's the flight to Denver!"
    );
    assert_eq!(clean("How long's it"), "How long's it");
}

#[test]
fn each_modifier_and_tail_takes_only_the_subjects_that_agree_with_it() {
    let subjects = [
        "i", "you", "we", "they", "he", "she", "it", "the", "this", "that", "my", "your", "our", "their",
    ];
    for (wh, modifiers) in WH_MODIFIER_CONTRACTIONS {
        for m in *modifiers {
            for (tail, aux, agree) in WH_CONTRACTIONS {
                for s in subjects {
                    let raw = format!("{wh} {m}'{tail} {s} going to stay tonight");
                    assert_eq!(clean(&raw).ends_with('?'), agree.contains(&s), "{raw:?} ({aux})");
                }
            }
        }
    }
    // A modifier listed for the other wh-word, or any other wh-word, is refused.
    for raw in [
        "How time's the meeting tomorrow",
        "What long's the flight to Denver",
        "Where time's the meeting tomorrow",
        "When long's the flight to Denver",
        "Who much's the ticket for the show",
    ] {
        assert!(!clean(raw).ends_with('?'), "{raw:?}");
    }
}

#[test]
fn a_name_from_the_dictionary_before_it_stays_as_adr_0206_left_it() {
    let dict = [DictionaryEntry {
        term: "Priya".into(),
        aliases: vec![],
    }];
    assert_eq!(
        rule_cleanup("Hey Priya what time's the dinner tonight", &dict),
        "Hey Priya what time's the dinner tonight."
    );
}
