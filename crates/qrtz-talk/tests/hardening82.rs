//! ADR 0234 (echo cycle 82): a spoken title before a name is written as its abbreviation, the way the recogniser
//! writes it in other decodes of the same clip: "Mister Hopkins" → "Mr. Hopkins", "missus Chen" → "Mrs. Chen",
//! "Mister and Missus Lee" → "Mr. and Mrs. Lee". Only "Mister" / "mister" / "Missus" / "missus" as a whole word, one
//! space, then a capitalised name (or "and", the partner title and a name); a vocative "mister", a sentence that
//! starts after it, the "Mister Rogers" names and a dictionary that spells the title keep the words. Fails without
//! the fix (every positive kept "Mister" / "Missus"); the refusals pass on both.
use qrtz_talk::cleanup::{rule_cleanup, SPOKEN_TITLES, TITLE_KEPT, TITLE_NOT_NAMES};
use qrtz_talk::DictionaryEntry;

fn clean(raw: &str) -> String {
    rule_cleanup(raw, &[])
}

#[test]
fn a_spoken_title_before_a_name_is_written_as_its_abbreviation() {
    for (raw, want) in [
        (
            "I had a call with Mister Patel about the contract",
            "I had a call with Mr. Patel about the contract.",
        ),
        (
            "Please send the invoice to Missus Chen by Friday",
            "Please send the invoice to Mrs. Chen by Friday.",
        ),
        (
            "Mister Thompson will join the meeting at noon",
            "Mr. Thompson will join the meeting at noon.",
        ),
        (
            "Can you ask mister Garcia to call me back",
            "Can you ask Mr. Garcia to call me back?",
        ),
        (
            "Mister and Missus Lee are coming to dinner on Saturday",
            "Mr. and Mrs. Lee are coming to dinner on Saturday.",
        ),
        (
            "We got a lovely card from Mister and Mrs. Walker.",
            "We got a lovely card from Mr. and Mrs. Walker.",
        ),
        ("Missus and Mister Smith are here", "Mrs. and Mr. Smith are here."),
        ("Thank you, Mister President.", "Thank you, Mr. President."),
        (
            "I left a message for Mister O'Brien at the front desk",
            "I left a message for Mr. O'Brien at the front desk.",
        ),
        (
            "Mister Kim's flight lands at six tonight",
            "Mr. Kim's flight lands at six tonight.",
        ),
        (
            "Missus Chen\u{2019}s office is upstairs",
            "Mrs. Chen\u{2019}s office is upstairs.",
        ),
        (
            "Mister Smiths' house is the blue one",
            "Mr. Smiths' house is the blue one.",
        ),
        (
            "The package is for Missus Rodriguez-Lopez in apartment four",
            "The package is for Mrs. Rodriguez-Lopez in apartment four.",
        ),
        (
            "Mister Smith, Missus Jones and Mister Lee signed it",
            "Mr. Smith, Mrs. Jones and Mr. Lee signed it.",
        ),
        (
            "Attendees new line Mister Hall new line Missus Grant",
            "Attendees\nMr. Hall\nMrs. Grant",
        ),
        ("Mister Smith?", "Mr. Smith?"),
        // Real recogniser hypotheses cached on the box (LibriSpeech clips whose other decodes wrote "Mr.").
        (
            "So, to the surprise of the Democratic Committee and all his friends, mister Hopkins announced",
            "So, to the surprise of the Democratic Committee and all his friends, Mr. Hopkins announced.",
        ),
        (
            "Lady Crichton, Mister and Mrs. Vanderpool, Mister Harry Cresswell and his sister",
            "Lady Crichton, Mr. and Mrs. Vanderpool, Mr. Harry Cresswell and his sister.",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn a_title_that_is_not_before_a_name_keeps_its_words() {
    for raw in [
        // A vocative, then a mark or a sentence with no mark between.
        "Hey mister, you dropped your wallet.",
        "Thanks mister. That was really kind of you.",
        "Excuse me mister I think you dropped this.",
        "Excuse me mister i think you dropped this.",
        "Listen mister I'm not going to ask you again.",
        "Sorry mister It's not for sale.",
        "Thank you mister The car is ready out front.",
        "Bye mister See you tomorrow.",
        "Good morning mister What can I get you?",
        "Thanks mister May I ask your name?",
        "Thanks mister Nobody else stopped to help.",
        "Hey Mister, Sam says hi.",
        "Mister, Smith is here to see you.",
        // Names written with the spelled-out title.
        "We watched Mister Rogers with the kids last night.",
        "The Mister Softee truck came by again today.",
        "Grandpa loved watching Mister Ed reruns.",
        "There is a Mister Donut right by the station.",
        "Mister Mister was a band in the eighties.",
        // No name after it.
        "She called him a little mister all day long.",
        "The missus wants us home by six.",
        "Say hi to the mister and the missus for me.",
        "We met Mister on the train, or so he called himself.",
        "Mister and Missus",
        "Mister T pities the fool.",
        "Hey Mister DJ, play our song.",
        "Ask Mister O'brien about the lease.",
        "Mister Smith- is here.",
        "I met Mister Jos\u{e9} Ramos at the conference.",
        "The meeting with Mister Monday is canceled.",
        // Other titles, other spellings, the partner without its dot, a verb that sounds like it.
        "Doctor Who is my favorite show.",
        "Saint Paul is a lovely city in the summer.",
        "Miss Daisy will see you now.",
        "Mr. Smith is already in the lobby.",
        "Mrs Smith and Mr Jones are both here.",
        "Mister and Mrs Smith arrived early.",
        "Mister and misses Smith arrived early.",
        "MISTER SMITH IS HERE.",
        "MISTER Smith is here.",
        "Misters Smith and Jones are here.",
        "Mister's here already",
        "(Mister Smith) is here.",
        // A line break between them.
        "Talk to mister\nSmith tomorrow.",
    ] {
        let out = clean(raw);
        assert!(!out.contains("Mr.") || raw.contains("Mr."), "{raw:?} -> {out:?}");
        assert!(
            !out.contains("Mrs.") || raw.contains("Mrs."),
            "{raw:?} -> {out:?}"
        );
    }
    // Byte for byte where the text needs no other step.
    for raw in [
        "Excuse me mister I think you dropped this.",
        "We watched Mister Rogers with the kids last night.",
        "Mister and misses Smith arrived early.",
        "Talk to mister\nSmith tomorrow.",
    ] {
        assert_eq!(clean(raw), raw);
    }
}

#[test]
fn a_dictionary_that_spells_the_title_turns_the_rule_off() {
    let entry = |term: &str, aliases: &[&str]| DictionaryEntry {
        term: term.into(),
        aliases: aliases.iter().map(|a| (*a).into()).collect(),
    };
    let raw = "I had a call with Mister Patel about the contract";
    for dict in [
        vec![entry("Mister Softee", &[])],
        vec![entry("Mrs. Chen", &["missus chen"])],
    ] {
        assert_eq!(
            rule_cleanup(raw, &dict),
            "I had a call with Mister Patel about the contract.",
            "{dict:?}"
        );
    }
    // The user's own spelling of the word stays.
    assert_eq!(
        rule_cleanup(raw, &[entry("MISTER", &[])]),
        "I had a call with MISTER Patel about the contract."
    );
    // Any other dictionary leaves the rule on.
    assert_eq!(
        rule_cleanup(raw, &[entry("Priya", &["pria"]), entry("Patel", &[])]),
        "I had a call with Mr. Patel about the contract."
    );
}

#[test]
fn every_title_reads_the_same_names() {
    let names = [
        ("Smith", true),
        ("Smith's", true),
        ("Smith\u{2019}s", true),
        ("Jones'", true),
        ("O'Brien", true),
        ("O\u{2019}Brien", true),
        ("McDonald", true),
        ("Rodriguez-Lopez", true),
        ("Smith,", true),
        ("Smith;", true),
        ("President", true),
        ("smith", false),
        ("SMITH", false),
        ("S", false),
        ("I", false),
        ("I'm", false),
        ("It's", false),
        ("Don't", false),
        ("O'brien", false),
        ("Smith-", false),
        ("Smith-jones", false),
        ("Jos\u{e9}", false),
        ("Smith2", false),
        ("May", false),
        ("Friday", false),
        ("Rogers", false),
        ("Ed", false),
    ];
    for (spoken, abbr) in SPOKEN_TITLES {
        for (name, converts) in names {
            let raw = format!("We spoke with {spoken} {name} about the lease");
            let out = clean(&raw);
            let want = format!("We spoke with {abbr} {name} about the lease.");
            if converts {
                assert_eq!(out, want, "{raw:?}");
            } else {
                assert!(out.contains(spoken), "{raw:?} -> {out:?}");
            }
        }
    }
    for w in TITLE_KEPT.iter().copied().chain(TITLE_NOT_NAMES.split(' ')) {
        let out = clean(&format!("We spoke with Mister {w} about the lease"));
        assert!(out.contains("Mister"), "{w}: {out:?}");
    }
}
