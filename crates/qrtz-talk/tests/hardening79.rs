//! ADR 0230 (echo cycle 79): a spoken date's day is written as a figure: "on October twenty first" → "on October
//! 21st", the way the recogniser writes it when it writes digits ("Saturday, August 15th"). Only an ordinal day 1-31
//! right after a capitalised month that follows a date preposition or a weekday, with its own mark, the end, or a
//! listed word after it. Fails without the fix (every positive came out in words); the refusals pass on both.
use qrtz_talk::cleanup::rule_cleanup;
use qrtz_talk::DictionaryEntry;

/// A figure with an ordinal suffix ("5th", "21st") anywhere in the text.
fn has_day_figure(t: &str) -> bool {
    let b = t.as_bytes();
    (1..b.len())
        .any(|i| b[i - 1].is_ascii_digit() && ["st", "nd", "rd", "th"].iter().any(|s| t[i..].starts_with(s)))
}

fn clean(raw: &str) -> String {
    rule_cleanup(raw, &[])
}

#[test]
fn a_spoken_date_s_day_is_written_as_a_figure() {
    for (raw, want) in [
        (
            "Let's meet on October twenty first",
            "Let's meet on October 21st.",
        ),
        (
            "The launch is on October fifth, so plan for it.",
            "The launch is on October 5th, so plan for it.",
        ),
        (
            "Can we push the review to November third",
            "Can we push the review to November 3rd?",
        ),
        (
            "Please send the contract by May second.",
            "Please send the contract by May 2nd.",
        ),
        (
            "The deadline is March thirty first",
            "The deadline is March 31st.",
        ),
        (
            "Starting January tenth we will meet weekly",
            "Starting January 10th we will meet weekly.",
        ),
        (
            "Let's meet on October fifth at three p.m.",
            "Let's meet on October 5th at 3 p.m.",
        ),
        (
            "Saturday, August fifteenth. The sea was calm.",
            "Saturday, August 15th. The sea was calm.",
        ),
        (
            "Our offsite is the week of September twenty-second",
            "Our offsite is the week of September 22nd.",
        ),
        (
            "I have been out since February twelfth.",
            "I have been out since February 12th.",
        ),
        (
            "Hi Priya, the invoice is due by June thirtieth.",
            "Hi Priya, the invoice is due by June 30th.",
        ),
        ("Tuesday, August eighteenth", "Tuesday, August 18th"),
        ("Tuesday August eighteenth", "Tuesday August 18th"),
        ("By May first we need the draft", "By May 1st we need the draft."),
        // The day ends its line: what the next line says is not part of it.
        (
            "The deadline is October fifth\nTwenty people are coming.",
            "The deadline is October 5th\nTwenty people are coming.",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn every_day_gets_its_suffix() {
    let days = [
        "first",
        "second",
        "third",
        "fourth",
        "fifth",
        "sixth",
        "seventh",
        "eighth",
        "ninth",
        "tenth",
        "eleventh",
        "twelfth",
        "thirteenth",
        "fourteenth",
        "fifteenth",
        "sixteenth",
        "seventeenth",
        "eighteenth",
        "nineteenth",
        "twentieth",
    ];
    let mut spoken: Vec<String> = days.iter().map(|d| (*d).to_owned()).collect();
    for d in &days[..9] {
        spoken.push(format!("twenty {d}"));
    }
    spoken.push("thirtieth".into());
    spoken.push("thirty first".into());
    let want = [
        "1st", "2nd", "3rd", "4th", "5th", "6th", "7th", "8th", "9th", "10th", "11th", "12th", "13th",
        "14th", "15th", "16th", "17th", "18th", "19th", "20th", "21st", "22nd", "23rd", "24th", "25th",
        "26th", "27th", "28th", "29th", "30th", "31st",
    ];
    for (s, w) in spoken.iter().zip(want) {
        assert_eq!(
            clean(&format!("It was due by December {s}.")),
            format!("It was due by December {w}."),
            "{s}"
        );
    }
}

#[test]
fn a_date_shape_that_can_read_another_way_stays_as_spoken() {
    for raw in [
        "Give it to April first.",
        "You can go after June first.",
        "Let's focus on October first, then November.",
        "My priority is March first.",
        "In March first we went home.",
        "On April fourth graders take the state test.",
        "We are out on October fifth and sixth.",
        "We are out on October fifth, sixth, and seventh.",
        "We launched on October fifth, 2026, in Austin.",
        "We met on October twenty one at the cafe.",
        "He arrived on October fifth the team cheered.",
        "The Fourth of July parade starts at noon.",
        "We moved to October first, then to November.",
        "I talked to May first and then to Sam.",
        "In June, first we test the app.",
        "I'll check on June first, then on Sam.",
        "Book it for Monday, October fifth, twenty twenty six.",
        "Is it on October fifth or sixth?",
        "It's due on October 5th.",
        "We finished second in March.",
        "Starting March second half sales picked up.",
        "The final is on October thirty second.",
        "On October Fifth we met.",
        "On October, fifth we met.",
        "We met on October twenty, first in line.",
        "We met on October fifth, or the sixth at the latest.",
    ] {
        let out = clean(raw);
        assert!(!has_day_figure(&out) || has_day_figure(raw), "{raw:?} -> {out:?}");
        // Only the older rules touch it: the words of the day are still there.
        assert_eq!(
            out.split_whitespace().count(),
            raw.split_whitespace().count(),
            "{raw:?} -> {out:?}"
        );
    }
    // A lowercase month is never one (the recogniser capitalises months).
    assert_eq!(clean("on october fifth we met"), "On october fifth we met.");
}

#[test]
fn a_month_the_dictionary_spells_is_left_alone() {
    let dict = [DictionaryEntry {
        term: "June".into(),
        aliases: vec![],
    }];
    assert_eq!(
        rule_cleanup("Ask for help since June ninth.", &dict),
        "Ask for help since June ninth."
    );
    assert_eq!(
        rule_cleanup("Ask for help since June ninth.", &[]),
        "Ask for help since June 9th."
    );
}
