//! ADR 0206 (echo cycle 66): a question to someone named in the user's dictionary gets the vocative comma and its
//! "?": "Hey Jared are you free tomorrow" → "Hey Jared, are you free tomorrow?". Only "hey", only a dictionary term
//! shaped like a name right after it, and only where the words after the name are a question ADR 0203 would take
//! after "Hey" with a nominative subject (I/you/we/they/he/she) or a wh-word other than "who". Everything else ends as
//! before. Fails without the fix (every positive came out with no comma and a ".").
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
        entry("Kubernetes", &[]),
        entry("Teams", &[]),
    ]
}

fn clean(raw: &str) -> String {
    rule_cleanup(raw, &dict())
}

#[test]
fn a_dictionary_name_after_hey_before_a_question_gets_its_comma() {
    for (raw, want) in [
        (
            "Hey Jared are you free tomorrow",
            "Hey Jared, are you free tomorrow?",
        ),
        (
            "Hey Priya can you send me the deck",
            "Hey Priya, can you send me the deck?",
        ),
        (
            "Hey Jared did you get my email",
            "Hey Jared, did you get my email?",
        ),
        (
            "Hey Priya what time does the store close",
            "Hey Priya, what time does the store close?",
        ),
        (
            "I'm outside. Hey Jared are you home",
            "I'm outside. Hey Jared, are you home?",
        ),
        (
            "hey priya are we still on for tonight",
            "Hey Priya, are we still on for tonight?",
        ),
        ("Hey Jared how was the trip", "Hey Jared, how was the trip?"),
        (
            "Um, hey Priya could you take a look at this",
            "Hey Priya, could you take a look at this?",
        ),
        (
            "Notes new line hey Jared will you call me back",
            "Notes\nHey Jared, will you call me back?",
        ),
        (
            "Hey Jared where did you put the keys",
            "Hey Jared, where did you put the keys?",
        ),
        (
            "Hey Jared is she coming to the party",
            "Hey Jared, is she coming to the party?",
        ),
        (
            "Hey pria are you around today",
            "Hey Priya, are you around today?",
        ),
        (
            "Hey  Jared   are you free tomorrow",
            "Hey Jared, are you free tomorrow?",
        ),
        (
            "Hey Jared shall we start without them",
            "Hey Jared, shall we start without them?",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn everything_else_ends_as_before() {
    for raw in [
        // not a dictionary name, or not shaped like one
        "Hey Sam are you free tomorrow",
        "Hey qrtz can you open my notes",
        "Hey Jared Smith are you free tomorrow",
        // the name can be the subject of a statement: an object pronoun, "this"/"that"/"there", "did you a ..."
        "Hey Jared did you a favor last week",
        "Hey Jared has it covered",
        "Hey Jared does it every week",
        "Hey Jared is that guy from the meetup",
        "Hey Priya is there a lot of traffic",
        "Hey Kubernetes is it worth learning",
        "Hey Priya can be late sometimes",
        // a relative clause, a statement, a second clause, an apostrophe, reported speech
        "Hey Jared who is the new designer",
        "Hey Jared you did great today",
        "Hey Jared I think we can ship it",
        "Hey Jared are you free or should we wait",
        "Hey Jared can't you just wait",
        "So I said hey Jared are you coming",
        // only "hey"; a comma already there; "did you" before a verb outside the list
        "Okay Jared can you send it over",
        "Okay Teams have you covered",
        "Hey Jared, are you free tomorrow",
        "Hey Jared did you finish the report",
        // too short, and a question that is not the last sentence
        "Hey Jared are you",
        "Hey Jared are you free. Thanks",
    ] {
        assert_eq!(clean(raw), format!("{raw}."), "{raw:?}");
    }
}

#[test]
fn no_dictionary_no_comma_and_question_words_are_never_names() {
    assert_eq!(
        rule_cleanup("Hey Jared are you free tomorrow", &[]),
        "Hey Jared are you free tomorrow."
    );
    // A term spelled like a word the question test reads is not taken for a name.
    let words = [entry("Will", &[]), entry("Who", &[]), entry("Do", &[])];
    assert_eq!(
        rule_cleanup("Hey Will are you coming tonight", &words),
        "Hey Will are you coming tonight."
    );
    // ADR 0203/0204 still decide first: with a term "Will", "Hey will you send it" is their question.
    assert_eq!(
        rule_cleanup("Hey will you send it", &words),
        "Hey, Will you send it?"
    );
}
