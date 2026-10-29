//! ADR 0238 (echo cycle 85): a comma (or semicolon) before a curly closing quote does not get a space.
//! ASCII `"` / `'` were already skipped (`SPACE_AFTER_PUNCT`); U+201D / U+2019 were not, so
//! `"plan A," then` typed `"plan A, " then`. Fails without the fix (every positive gains the space); the
//! refusals pass on both.
use qrtz_talk::cleanup::rule_cleanup;
use qrtz_talk::DictionaryEntry;

fn priya() -> Vec<DictionaryEntry> {
    vec![DictionaryEntry {
        term: "Priya".into(),
        aliases: vec!["pria".into()],
    }]
}

#[test]
fn curly_close_after_comma_keeps_no_space() {
    let priya = priya();
    for (raw, dict, want) in [
        (
            "The plan was \u{201C}ship it Friday,\u{201D} then cut the release",
            false,
            "The plan was \u{201C}ship it Friday,\u{201D} then cut the release.",
        ),
        (
            "She said \u{201C}I\u{2019}m ready,\u{201D} and left",
            false,
            "She said \u{201C}I\u{2019}m ready,\u{201D} and left.",
        ),
        (
            "Call it \u{2018}done,\u{2019} then move on",
            false,
            "Call it \u{2018}done,\u{2019} then move on.",
        ),
        (
            "We marked it \u{201C}blocked,\u{201D} so nobody starts it",
            false,
            "We marked it \u{201C}blocked,\u{201D} so nobody starts it.",
        ),
        (
            "His note ended \u{201C}thanks,\u{201D}",
            false,
            "His note ended \u{201C}thanks,\u{201D}",
        ),
        (
            "Pick \u{201C}A,\u{201D} \u{201C}B,\u{201D} or \u{201C}C\u{201D}",
            false,
            "Pick \u{201C}A,\u{201D} \u{201C}B,\u{201D} or \u{201C}C\u{201D}",
        ),
        (
            "The label read \u{201C}fragile,\u{201D} on every box",
            false,
            "The label read \u{201C}fragile,\u{201D} on every box.",
        ),
        (
            "Write \u{201C}urgent,\u{201D} in the subject",
            false,
            "Write \u{201C}urgent,\u{201D} in the subject.",
        ),
        (
            "I told Priya \u{201C}ship tonight,\u{201D} before the freeze",
            true,
            "I told Priya \u{201C}ship tonight,\u{201D} before the freeze.",
        ),
        (
            "Use \u{201C}ok,\u{201D}; then restart",
            false,
            "Use \u{201C}ok,\u{201D}; then restart.",
        ),
        (
            "Tag it \u{201C}v1,\u{201D} and publish",
            false,
            "Tag it \u{201C}v1,\u{201D} and publish.",
        ),
    ] {
        let d: &[DictionaryEntry] = if dict { &priya } else { &[] };
        let got = rule_cleanup(raw, d);
        assert_eq!(got, want, "raw={raw:?}");
        assert!(
            !got.contains(", \u{201D}") && !got.contains(", \u{2019}"),
            "space before curly close in {got:?}"
        );
    }
}

#[test]
fn curly_close_after_comma_refusals() {
    let priya = priya();
    for (raw, dict, want) in [
        // ASCII already skipped; unchanged.
        (
            "The plan was \"ship it Friday,\" then cut the release",
            false,
            "The plan was \"ship it Friday,\" then cut the release.",
        ),
        (
            "Call it 'done,' then move on",
            false,
            "Call it 'done,' then move on.",
        ),
        // Curly OPEN after comma still gets a space.
        (
            "He said,\u{201C}hello\u{201D}",
            false,
            "He said, \u{201C}hello\u{201D}",
        ),
        (
            "She murmured,\u{2018}wait\u{2019}",
            false,
            "She murmured, \u{2018}wait\u{2019}",
        ),
        // Already has a space before the close: leave it (rare; dictionary / paste).
        (
            "Odd form \u{201C}plan A, \u{201D} then",
            false,
            "Odd form \u{201C}plan A, \u{201D} then.",
        ),
        // Normal commas and digits.
        (
            "Well, well, well, look who it is",
            false,
            "Well, well, well, look who it is.",
        ),
        (
            "Over 1,000 people signed up",
            false,
            "Over 1,000 people signed up.",
        ),
        // Curly apostrophe in a contraction after a normal comma+space.
        (
            "It\u{2019}s a nice day, isn\u{2019}t it",
            false,
            "It\u{2019}s a nice day, isn\u{2019}t it?",
        ),
        (
            "Let\u{2019}s call it a day, shall we",
            false,
            "Let\u{2019}s call it a day, shall we?",
        ),
        // Closing paren / bracket still skipped.
        ("See (a,) then (b)", false, "See (a,) then (b)"),
        ("See [a,] then [b]", false, "See [a,] then [b]"),
        // Dictionary name path unchanged.
        (
            "Hey Priya, thanks for the update",
            true,
            "Hey Priya, thanks for the update.",
        ),
        // Guillemets and other scripts: not in scope; space still goes in before ».
        ("Dit-il,\u{00BB}oui\u{00AB}", false, "Dit-il, \u{00BB}oui\u{00AB}"),
    ] {
        let d: &[DictionaryEntry] = if dict { &priya } else { &[] };
        assert_eq!(rule_cleanup(raw, d), want, "raw={raw:?}");
    }
}
