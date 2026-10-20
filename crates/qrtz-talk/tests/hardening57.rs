#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADRs 0188 and 0189 (echo cycle 57): two polish rules, each only in its unambiguous form.
//! - ADR 0188: a list of short lines (3+ lines, no blank line, each 1-3 words, no sentence mark but on the last)
//!   gets no ".".
//! - ADR 0189: a dictation-final spoken "question mark" / "exclamation point" / "exclamation mark" becomes "?" / "!"
//!   only where it can't be the noun.
use qrtz_talk::cleanup::rule_cleanup;

fn c(raw: &str) -> String {
    rule_cleanup(raw, &[])
}

#[test]
fn a_list_of_short_lines_gets_no_period() {
    assert_eq!(
        c("Groceries new line eggs new line milk"),
        "Groceries\nEggs\nMilk"
    );
    assert_eq!(
        c("73 Naples St new line San Francisco new line CA 94112"),
        "73 Naples St\nSan Francisco\nCA 94112"
    );
    // The recogniser's own final "." goes too, at a clean sentence end.
    assert_eq!(
        c("Groceries new line eggs new line milk."),
        "Groceries\nEggs\nMilk"
    );
}

#[test]
fn not_a_list_keeps_its_period() {
    // Two lines are not a list.
    assert_eq!(c("Call her new line then email Sam"), "Call her\nThen email Sam.");
    assert_eq!(
        c("Dear team, new line, quick update"),
        "Dear team,\nQuick update."
    );
    // A long line, a paragraph break, sentence ends on the lines.
    assert_eq!(
        c("Dear team new line quick update new line the launch moved to Monday"),
        "Dear team\nQuick update\nThe launch moved to Monday."
    );
    assert_eq!(
        c("Hi Sam new paragraph see you soon new paragraph Jared"),
        "Hi Sam\n\nSee you soon\n\nJared."
    );
    assert_eq!(
        c("Done. New line. Next. New line. Ship it."),
        "Done.\nNext.\nShip it."
    );
    // A dotted token or an abbreviation keeps its dot; "?" stays.
    assert_eq!(
        c("Eggs new line milk new line at 3 p.m."),
        "Eggs\nMilk\nAt 3 p.m."
    );
    assert_eq!(
        c("Pens new line paper new line staples etc."),
        "Pens\nPaper\nStaples etc."
    );
    assert_eq!(
        c("Need anything new line eggs new line milk?"),
        "Need anything\nEggs\nMilk?"
    );
}

#[test]
fn a_final_spoken_mark_becomes_the_mark() {
    assert_eq!(
        c("Is the meeting still on question mark"),
        "Is the meeting still on?"
    );
    assert_eq!(c("Is it ready question mark."), "Is it ready?");
    assert_eq!(c("Are you there question mark"), "Are you there?");
    assert_eq!(c("Great work everyone exclamation point"), "Great work everyone!");
    assert_eq!(c("Happy birthday Sam exclamation mark"), "Happy birthday Sam!");
    assert_eq!(c("Congratulations exclamation point"), "Congratulations!");
    assert_eq!(
        c("Thanks for the update. Is the demo still on question mark"),
        "Thanks for the update. Is the demo still on?"
    );
}

#[test]
fn a_spoken_mark_that_can_be_the_noun_stays() {
    for raw in [
        "Is that a question mark or a period",
        "Is there a question mark",
        "Is there question mark",
        "Does the title need question mark",
        "Which key is question mark",
        "How do I type question mark",
        "Is it a big question mark",
        "Did you see that question mark",
        "That's still a big question mark",
        "Add an exclamation point",
        "My favorite key is exclamation point",
        "Shift plus one types exclamation point",
        "Shift 1 exclamation point",
        "Can you hit exclamation point",
        "Use fewer exclamation points",
        "Open qrtz.ai question mark",
        "What do I add at the end, question mark",
    ] {
        let out = c(raw);
        assert!(!out.ends_with('?') && !out.ends_with('!'), "{raw:?} -> {out:?}");
        assert!(
            out.contains("question mark") || out.contains("exclamation"),
            "{raw:?} -> {out:?}"
        );
    }
    // Capitalized: a name or a new sentence, not the command.
    assert_eq!(
        c("The band was called Question Mark"),
        "The band was called Question Mark."
    );
    assert_eq!(
        c("What's this symbol? Question mark."),
        "What's this symbol? Question mark."
    );
}
