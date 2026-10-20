#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0208 (echo cycle 68): after "Hey <dictionary name> did you", a closed list of plain base-form verbs also opens
//! the question ADR 0206 punctuates: "Hey Jared did you finish the report" → "Hey Jared, did you finish the report?".
//! "did you" + an object or complement ("did you a favor", "did you good", "did you wrong", "did you proud", "did you
//! justice", "did you like a pro") is a statement about the name and stays. Fails without the fix (every positive
//! came out with no comma and a ".").
use qrtz_talk::cleanup::rule_cleanup;
use qrtz_talk::DictionaryEntry;

fn entry(term: &str, aliases: &[&str]) -> DictionaryEntry {
    DictionaryEntry {
        term: term.into(),
        aliases: aliases.iter().map(|a| (*a).into()).collect(),
    }
}

fn dict() -> Vec<DictionaryEntry> {
    vec![
        entry("qrtz", &["cue arts"]),
        entry("Jared", &[]),
        entry("Priya", &["pria"]),
    ]
}

fn clean(raw: &str) -> String {
    rule_cleanup(raw, &dict())
}

#[test]
fn hey_name_did_you_and_a_listed_verb_is_a_question() {
    for (raw, want) in [
        (
            "Hey Jared did you finish the report",
            "Hey Jared, did you finish the report?",
        ),
        (
            "hey priya did you hear back from the landlord",
            "Hey Priya, did you hear back from the landlord?",
        ),
        (
            "I sent the draft last night. Hey Jared did you read it yet",
            "I sent the draft last night. Hey Jared, did you read it yet?",
        ),
        (
            "Hey pria did you book the flights for Friday",
            "Hey Priya, did you book the flights for Friday?",
        ),
        (
            "Hey Jared did you try the new build",
            "Hey Jared, did you try the new build?",
        ),
        (
            "Hey Jared did you sign the lease yet",
            "Hey Jared, did you sign the lease yet?",
        ),
        (
            "Hey Jared did you pick up the keys",
            "Hey Jared, did you pick up the keys?",
        ),
        (
            "Hey Jared did you merge the pull request",
            "Hey Jared, did you merge the pull request?",
        ),
        (
            "Hey Jared did you eat lunch already",
            "Hey Jared, did you eat lunch already?",
        ),
        (
            "Hey Priya did you email the client",
            "Hey Priya, did you email the client?",
        ),
        ("Hey Priya did you finish", "Hey Priya, did you finish?"),
        (
            "Notes new line hey Jared did you ship the fix",
            "Notes\nHey Jared, did you ship the fix?",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn did_you_with_an_object_or_complement_ends_as_before() {
    for raw in [
        // a statement about the name: "you" is the object of "did"
        "Hey Jared did you a solid last week",
        "Hey Jared did you a favor last week",
        "Hey Jared did you good with that advice",
        "Hey Jared did you wrong",
        "Hey Priya did you proud at the recital",
        "Hey Jared did you justice in that review",
        "Hey Jared did you dirty in that meeting",
        "Hey Jared did you like a pro on that deal",
        "Hey Priya did you a good turn yesterday",
        "Hey Priya did you right by fixing it",
        "Hey Jared did you no harm by staying",
        "Hey Priya did you well in the end",
        "Hey Priya did you credit in the writeup",
        // not a dictionary name, not "hey", a comma already there, "ever", reported speech, a second clause
        "Hey Sam did you finish the report",
        "Okay Jared did you finish the report",
        "Hey Jared, did you finish the report",
        "Hey Jared did you ever finish the book",
        "So I said hey Jared did you finish the report",
        "Hey Jared did you finish the report that I sent",
        "Hey Jared did you finish the report. Thanks",
    ] {
        assert_eq!(clean(raw), format!("{raw}."), "{raw:?}");
    }
}

#[test]
fn a_term_spelled_like_a_listed_verb_is_never_a_name() {
    let words = [entry("Book", &[]), entry("Ship", &[])];
    assert_eq!(
        rule_cleanup("Hey Book did you finish the report", &words),
        "Hey Book did you finish the report."
    );
    assert_eq!(
        rule_cleanup("Hey Ship did you read the notes", &words),
        "Hey Ship did you read the notes."
    );
}
