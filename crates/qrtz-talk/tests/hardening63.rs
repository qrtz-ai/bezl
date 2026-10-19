//! ADR 0203 (echo cycle 63): a question after a discourse opener ends with "?" instead of the cleanup's "."
//! ("So do you want to grab lunch" → "So do you want to grab lunch?"). Openers: so, okay, ok, hey, and, but, also,
//! alright, with or without their own comma. A bare "So" needs do/does/did/can/could/will/would and a plain verb after
//! the subject (the agreeing ellipsis "So did we last year" stays); "and"/"but" need "you" as a yes/no subject (a
//! story's "And was he mad" stays); "well", "oh", "now", "boy", "man" are not openers. Fails without the fix (every
//! positive got ".").
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
fn a_question_after_an_opener_gets_a_question_mark() {
    for raw in [
        "So do you want to grab lunch",
        "So, do you want to grab lunch",
        "Okay, can you send me the file",
        "Hey, can you take a look at this",
        "And did you hear back from them",
        "But do you have time today",
        "And are you coming with us",
        "Also can you send the slides",
        "Alright, are we ready to start",
        "So what time does the store close",
        "And how was the trip",
        "I like it. So do you want to grab lunch",
    ] {
        assert_eq!(clean(raw), format!("{raw}?"), "{raw:?}");
    }
    // ADR 0204 (cycle 64): "hey", "okay", "ok" take a comma before the question.
    for (raw, want) in [
        ("Okay can you send me the file", "Okay, can you send me the file?"),
        ("OK can we move the meeting", "OK, can we move the meeting?"),
        (
            "Hey are you free tomorrow afternoon",
            "Hey, are you free tomorrow afternoon?",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
    assert_eq!(clean("Hey can you check cue arts"), "Hey, can you check qrtz?");
    assert_eq!(
        clean("Um so do you want to grab lunch"),
        "So do you want to grab lunch?"
    );
}

#[test]
fn ellipsis_exclamations_commands_and_anything_unclear_keep_their_period() {
    for raw in [
        // the agreeing ellipsis after a bare "So"
        "So did we last year",
        "So do I at work",
        "So can we in the summer",
        "So would I if I could",
        "So are we at the moment",
        "So is it on weekends",
        "So have I many times",
        "So could you at that age",
        "So will they at the next one",
        "So did he after the game",
        "And so do I every day",
        "But so did we last year",
        // exclamatory inversion
        "Well do I remember that day",
        "Well, was he surprised",
        "Oh do I love that song",
        "Oh, was it hot outside",
        "Boy do I need a vacation",
        "Man was I tired last night",
        "Now would you look at that",
        "And was he mad about it",
        "And do I miss it every day",
        "But were we surprised by that",
        "But can we still make it",
        // commands, dropped subjects, determiners
        "Okay do the dishes",
        "So do it now",
        "Okay is the store open",
        "Also did it again last night",
        // ADR 0196's refusals still hold after an opener
        "Okay can you send it we need it today",
        "Hey, should I bring anything",
        "Okay, have I got news for you",
        "But what do you know",
        "Alright, can I just say something",
        "And were we to lose the deal",
        // no question opening, more than one comma, two openers
        "So, I was thinking we could go",
        "Hey, Sam, can you send it",
        "Okay so do you want to go",
        "So, so do you want to go",
    ] {
        assert_eq!(clean(raw), format!("{raw}."), "{raw:?}");
    }
    assert_eq!(clean("So do you"), "So do you");
}
