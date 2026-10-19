//! ADRs 0192 and 0193 (echo cycle 58): two polish rules, each only in its unambiguous form.
//! - ADR 0192: a filler the recogniser set off with commas takes the comma before it along, but only after a word a
//!   comma can't follow ("the, um, design team", "and, uh, we", "The demo is, um, basically done", "Can you, um,
//!   send"). A comma that belongs to the sentence ("So, uh, the build", "If we can, um, we ship") stays.
//! - ADR 0193: spoken amounts become figures only where the reading is certain: "twenty five dollars" → "$25",
//!   "fifteen percent" → "15%", "three thirty p.m." → "3:30 p.m.". Ranges, decimals, cents, "a hundred", sentence
//!   starts and the singular "dollar" stay as spoken.
use qrtz_talk::cleanup::rule_cleanup;

fn c(raw: &str) -> String {
    rule_cleanup(raw, &[])
}

#[test]
fn a_filler_takes_its_comma_after_a_word_no_comma_follows() {
    // Determiners and "and" / "but" / "or" / "because".
    assert_eq!(
        c("We need to talk to the, um, design team."),
        "We need to talk to the design team."
    );
    assert_eq!(
        c("I fixed the bug and, uh, pushed the change."),
        "I fixed the bug and pushed the change."
    );
    assert_eq!(
        c("I left because, um, the commute was too long."),
        "I left because the commute was too long."
    );
    // A copula or modal in a plain main clause, before a plain word.
    assert_eq!(
        c("The demo is, um, basically done."),
        "The demo is basically done."
    );
    assert_eq!(
        c("Um, so I think we should, uh, ship it on Friday."),
        "So I think we should ship it on Friday."
    );
    assert_eq!(
        c("The launch will, um, probably slip a week."),
        "The launch will probably slip a week."
    );
    // A question's subject pronoun.
    assert_eq!(c("Can you, um, send me the deck?"), "Can you send me the deck?");
    assert_eq!(c("Is it, uh, ready?"), "Is it ready?");
    // Only a comma before the filler, or more fillers in a row.
    assert_eq!(c("I think we should, uh ship it."), "I think we should ship it.");
    assert_eq!(
        c("The demo is, um, uh, basically done."),
        "The demo is basically done."
    );
    // The long case: "and, uh, we we think".
    assert_eq!(
        c("So, um, I talked to the team and, uh, we we think the launch should move to next week because the the docs aren't ready."),
        "So, I talked to the team and we think the launch should move to next week because the docs aren't ready."
    );
}

#[test]
fn a_comma_the_sentence_needs_stays() {
    for (raw, want) in [
        ("So, uh, the build is green now.", "So, the build is green now."),
        ("Well, um, I guess so.", "Well, I guess so."),
        // The verb ends a clause: a clause opener before it, a reply word first, or the next word opens a clause.
        (
            "When we can, um, the team will ship it.",
            "When we can, the team will ship it.",
        ),
        (
            "If we can, um, we should ship it today.",
            "If we can, we should ship it today.",
        ),
        (
            "Whatever it is, um, the answer is no.",
            "Whatever it is, the answer is no.",
        ),
        ("That's how it is, um, honestly.", "That's how it is, honestly."),
        ("Here it is, um, finally.", "Here it is, finally."),
        ("We should, um, but we won't.", "We should, but we won't."),
        ("The thing is, um, we're late.", "The thing is, we're late."),
        ("The question is, um, do we ship?", "The question is, do we ship?"),
        ("We were, um, like, so close.", "We were, like, so close."),
        // A vocative after it.
        ("Yes we can, um, Sam.", "Yes we can, Sam."),
        ("I know you can, um, honey.", "I know you can, honey."),
        ("Do you, um, Jared, want coffee?", "Do you, Jared, want coffee?"),
        ("Is it, um, Sam?", "Is it, Sam?"),
        // A pronoun that isn't a question's subject; the letter A.
        ("I love you, um, so much.", "I love you, so much."),
        ("Plan A, um, then plan B.", "Plan A, then plan B."),
    ] {
        assert_eq!(c(raw), want, "{raw:?}");
    }
}

#[test]
fn spoken_amounts_become_figures() {
    assert_eq!(c("It costs twenty five dollars."), "It costs $25.");
    assert_eq!(c("He owes me forty-two dollars."), "He owes me $42.");
    assert_eq!(
        c("The budget is two thousand five hundred dollars."),
        "The budget is $2,500."
    );
    assert_eq!(
        c("I paid one hundred and fifty dollars for it."),
        "I paid $150 for it."
    );
    assert_eq!(
        c("We grew by fifteen percent this quarter."),
        "We grew by 15% this quarter."
    );
    assert_eq!(c("Sales are up 15 percent."), "Sales are up 15%.");
    assert_eq!(c("The rate is 3.5 percent now."), "The rate is 3.5% now.");
    assert_eq!(c("I'm one hundred percent sure."), "I'm 100% sure.");
    assert_eq!(
        c("The meeting is at three thirty p.m."),
        "The meeting is at 3:30 p.m."
    );
    assert_eq!(c("Call me at nine a.m. tomorrow."), "Call me at 9 a.m. tomorrow.");
    assert_eq!(
        c("The flight leaves at ten oh five a.m."),
        "The flight leaves at 10:05 a.m."
    );
    assert_eq!(c("At eleven fifty nine p.m. we ship."), "At 11:59 p.m. we ship.");
    // After the spoken mark and the terminal "." (their word counts are the spoken words').
    assert_eq!(c("Is it fifteen percent question mark"), "Is it 15%?");
    assert_eq!(c("It costs zero dollars"), "It costs $0.");
}

#[test]
fn amounts_that_could_read_another_way_stay_as_spoken() {
    for raw in [
        "Give it a hundred percent.",
        "It costs a couple of dollars.",
        "It was a few hundred dollars.",
        "It costs one or two dollars.",
        "It costs between ten and twenty dollars.",
        "Rates rose two point five percent.",
        "It was five dollars and fifty cents.",
        "Ten Percent Happier is a good book.",
        "I have twenty five dollar bills.",
        "Prices are in 2025 dollars.",
        "It cost two million dollars.",
        "It cost fifteen hundred dollars.",
        "It cost nineteen ninety nine dollars.",
        "Growth was ten to fifteen percent.",
        "The tip was fifteen percent to twenty percent.",
        "I need ten dollars for two tickets.",
        "Meet at three p.m. to five p.m.",
        "It went from two to three p.m.",
        "Twenty five dollars is too much.",
        "Ninety nine percent of the time it works.",
        "I'd bet one dollar on it.",
        "Let's meet at three. I am free all day.",
    ] {
        assert_eq!(c(raw), raw, "{raw:?}");
    }
}
