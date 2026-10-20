#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0235 (echo cycle 83): cleanup never leaves a doubled or stray mark where it took a filler out. A filler said
//! as its own sentence ("Thanks. Um. Can you send it", "Um. Hello") goes with its mark, so the text reads as if the
//! filler hadn't been said. Fails without the fix (every positive kept "..", "?.", "!." or a leading "."); the
//! refusals pass on both.
use qrtz_talk::cleanup::rule_cleanup;
use qrtz_talk::DictionaryEntry;

fn clean(raw: &str) -> String {
    rule_cleanup(raw, &[])
}

#[test]
fn a_filler_sentence_goes_with_its_mark() {
    for (raw, want) in [
        (
            "Thanks for the update. Um. Can you send me the deck tonight",
            "Thanks for the update. Can you send me the deck tonight?",
        ),
        (
            "Um. I think we should push the launch to next week",
            "I think we should push the launch to next week.",
        ),
        (
            "The build is green now. Uh. Let's ship it after lunch",
            "The build is green now. Let's ship it after lunch.",
        ),
        (
            "Are you coming to the offsite? Hmm. Let me know by Friday",
            "Are you coming to the offsite? Let me know by Friday.",
        ),
        (
            "That demo went really well! Um. Thanks everyone for staying late",
            "That demo went really well! Thanks everyone for staying late.",
        ),
        (
            "I'll review the pull request tonight. Uh.",
            "I'll review the pull request tonight.",
        ),
        (
            "Okay, so the plan is set. Um... Let's meet again on Monday",
            "Okay, so the plan is set. Let's meet again on Monday.",
        ),
        (
            "We need to fix the login bug first. Um. Uh. Then we can look at the dashboard",
            "We need to fix the login bug first. Then we can look at the dashboard.",
        ),
        ("Really? Uh. I doubt it", "Really? I doubt it."),
        ("Wow! Um. That was great", "Wow! That was great."),
        ("at 3 p.m. Um. Then we leave", "At 3 p.m. Then we leave."),
        (
            "We shipped it. Erm?! Anyway, next topic",
            "We shipped it. Anyway, next topic.",
        ),
        ("Thanks. Hmmm. Okay", "Thanks. Okay."),
        ("Done.\nUm.\nThen we leave.", "Done.\n\nThen we leave."),
        ("Um. Hello there my good friend.", "Hello there my good friend."),
        ("Um... so what do you think", "So what do you think?"),
        ("Okay. Um.", "Okay"),
        ("Um.", ""),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

/// The text a filler sentence leaves is the text without it, with or without a dictionary.
#[test]
fn a_filler_sentence_reads_as_if_it_was_never_said() {
    let dict = [DictionaryEntry {
        term: "Priya".into(),
        aliases: vec!["pria".into()],
    }];
    for (with, without) in [
        ("Thanks pria. Um. Can you send it", "Thanks pria. Can you send it"),
        (
            "It's done. Ah. The deck is in the folder.",
            "It's done. The deck is in the folder.",
        ),
        ("Is it ready? Mhm. Ship it then", "Is it ready? Ship it then"),
        (
            "We start at nine. Uhh! Wait, at ten",
            "We start at nine. Wait, at ten",
        ),
        ("Done... Um. Next", "Done... Next"),
    ] {
        assert_eq!(rule_cleanup(with, &[]), rule_cleanup(without, &[]), "{with:?}");
        assert_eq!(
            rule_cleanup(with, &dict),
            rule_cleanup(without, &dict),
            "{with:?}"
        );
    }
}

/// Left exactly as before: no filler, a kept capital filler, a filler-shaped start of a word, a lowercase word after
/// the filler sentence, a filler after a comma or with a comma after it (ADR 0235 leaves those three), and commas.
#[test]
fn the_refusals_are_unchanged() {
    for (raw, want) in [
        (
            "Which school did you pick? UM. Go Blue!",
            "Which school did you pick? UM. Go Blue!",
        ),
        ("It works. UM. Okay", "It works. UM. Okay."),
        (
            "Where does she work now? ER. She took the night shift.",
            "Where does she work now? ER. She took the night shift.",
        ),
        ("a 35 mm. Lens", "A 35 mm. Lens."),
        (
            "Use the range 0..10 in the loop.",
            "Use the range 0..10 in the loop.",
        ),
        (
            "Grab the umbrella. Umbrella stands are by the door.",
            "Grab the umbrella. Umbrella stands are by the door.",
        ),
        (
            "Turn left. Ahead is the parking garage.",
            "Turn left. Ahead is the parking garage.",
        ),
        (
            "Done. Umami is the fifth taste.",
            "Done. Umami is the fifth taste.",
        ),
        (
            "Okay. Uhura is my favorite character.",
            "Okay. Uhura is my favorite character.",
        ),
        ("Is it you? Erma! Come in.", "Is it you? Erma! Come in."),
        ("Wait... What did you say?", "Wait... What did you say?"),
        (
            "Visit qrtz.ai. The docs are there.",
            "Visit qrtz.ai. The docs are there.",
        ),
        // A lowercase word after the filler sentence, a filler after a comma or with a comma after it: as before this
        // change (ADR 0235, rejected).
        ("so... um. next we go", "So.... Next we go."),
        ("Thanks. Um. can you send it", "Thanks.. Can you send it?"),
        ("So, um. Next we go", "So, . Next we go."),
        ("Thanks. Um, can you send it", "Thanks can you send it."),
        (
            "Over 1,000 people signed up last week.",
            "Over 1,000 people signed up last week.",
        ),
        (
            "The tuple is (1,2,3) in the test.",
            "The tuple is (1,2,3) in the test.",
        ),
        ("Thanks, Sam, see you tomorrow.", "Thanks, Sam, see you tomorrow."),
        ("Call me at 5, 6, or 7.", "Call me at 5, 6, or 7."),
        ("Well, well, look who's here.", "Well, well, look who's here."),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}
