//! ADR 0196 (echo cycle 59): a dictation that ends with no mark gets "?" instead of "." when its last sentence can
//! only be a question: a yes/no opening ("Do you", "Can we", "Is it", "Is there any") or a wh-opening with an
//! auxiliary and a subject ("What time does the store close", "Where do you"), 3 to 12 words, no comma or
//! apostrophe, and nothing after the opening that starts a second clause. Commands, dropped subjects, conditional
//! and exclamatory inversions, and a mark the recogniser already wrote stay as before.
use qrtz_talk::cleanup::rule_cleanup;

fn c(raw: &str) -> String {
    rule_cleanup(raw, &[])
}

#[test]
fn an_unmarked_question_ends_with_a_question_mark() {
    for (raw, want) in [
        ("Do you want to grab lunch", "Do you want to grab lunch?"),
        (
            "what time does the store close",
            "What time does the store close?",
        ),
        (
            "Can you send me the deck before the meeting",
            "Can you send me the deck before the meeting?",
        ),
        (
            "is there anything else you need from me",
            "Is there anything else you need from me?",
        ),
        ("How long will the build take", "How long will the build take?"),
        (
            "Have you ever been to the new office",
            "Have you ever been to the new office?",
        ),
        ("Are we still on for Friday", "Are we still on for Friday?"),
        (
            "I'm heading out now. Do you want to grab lunch",
            "I'm heading out now. Do you want to grab lunch?",
        ),
        (
            "Hi Sam new line could you review my pull request today",
            "Hi Sam\nCould you review my pull request today?",
        ),
        ("Is it twenty five dollars for both", "Is it $25 for both?"),
    ] {
        assert_eq!(c(raw), want, "{raw:?}");
    }
}

#[test]
fn anything_that_might_not_be_a_question_keeps_its_period() {
    for raw in [
        // Commands and dropped subjects.
        "Do it now and send me the file",
        "Do the dishes before you go",
        "Have a good one tonight Sam",
        "Does the trick every single time",
        "Was the best day ever",
        "Did it again last night",
        "Did the dishes and took out the trash",
        "Has a nice ring to it honestly",
        "Was there for three hours",
        "Is a great idea honestly",
        // Conditional, wishing and exclamatory inversions; negative contractions.
        "Should you need anything just let me know",
        "Had I known I would have called sooner",
        "Were we to lose we would regroup",
        "May you both be very happy together",
        "Would that it were that simple",
        "Don't you dare touch my coffee",
        "Was I ever glad to see you",
        "Am I glad you called today",
        "Is it ever hot in here today",
        "Have I got news for you",
        "Can I just say thank you so much",
        "What do you know",
        // A second clause after the opening, a comma, a name, a fragment.
        "Do you want to grab lunch I'm starving",
        "Can you call Sam he is waiting",
        "Do you have the key the door is locked",
        "Can you believe it, what a game",
        "Will Smith is in the new movie",
        "Which is why I left early",
        "What I mean is we ship on Friday",
        "When you get there call me",
        "What did it was the timing",
        // ADR 0189's spoken mark names stay as that rule leaves them.
        "Is there a question mark",
        "Can you hit exclamation point",
    ] {
        assert_eq!(c(raw), format!("{raw}."), "{raw:?}");
    }
    // A mark the recogniser wrote is never changed, and a short dictation still gets none.
    assert_eq!(c("Do you want to grab lunch."), "Do you want to grab lunch.");
    assert_eq!(c("Are you free"), "Are you free");
    assert_eq!(c("Can you send it?"), "Can you send it?");
}
