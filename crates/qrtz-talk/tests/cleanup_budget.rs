//! ADR 0182: the latency budget for the whole cleanup chain. Polish must add no latency that anyone can feel, so
//! `cleanup_dictation` on a long dictation (about 400 words, every rule exercised, a 20-term dictionary) must stay
//! within 2 ms. The median of many runs is checked, in the optimised build the app ships (`--release`), and with a
//! looser ceiling in a debug build (`DEBUG_FACTOR`).
use qrtz_talk::cleanup::cleanup_dictation;
use qrtz_talk::DictionaryEntry;
use std::time::Instant;

const BUDGET_MS: f64 = 2.0;
/// A debug build runs these regexes ~14x slower (measured: 1.6 ms release, 22 ms debug). Before ADR 0182 the same
/// run took 22 ms release / 200 ms debug, so the debug ceiling still fails on that.
const DEBUG_FACTOR: f64 = 30.0;

fn long_dictation() -> String {
    let parts = [
        "Um, so I I think we should, uh, ship cue arts echo on Friday.",
        "The the build is green. and the docs are almost done",
        "I spent the night in the ER, so the review moved, um, to Monday.",
        "Use a 35 mm lens new line bring the charger new paragraph",
        "We launched our new line in May, and the numbers look good, hmm, better than last year.",
        "Email jlgrimes@users.noreply.github.com or go to https://qrtz.ai/docs for details. i.e. the setup guide",
        "Mr. and Mrs. Smith will join at 3 p.m. today with the team, etc. and post gres is migrated.",
        "Book the room for Monday. Scratch that, book it for Tuesday. My back itches. Scratch that itch.",
        // ADR 0189: the dictation ends in a spoken mark, so the final-mark step does its whole check and fires.
        // ADR 0192: fillers between commas, after a word a comma can't follow (and the refusals' checks).
        "The demo is, um, basically done and, uh, we think the, um, docs are ready. If we can, um, we ship.",
        // ADR 0193: spoken amounts, converted and refused.
        "It costs twenty five dollars, up fifteen percent, at three thirty p.m. Between ten and twenty dollars.",
        "Thanks for the update. Is the demo still on question mark",
    ];
    let mut s = String::new();
    while s.split_whitespace().count() < 400 {
        for p in parts {
            s.push_str(p);
            s.push(' ');
        }
    }
    s
}

fn dictionary() -> Vec<DictionaryEntry> {
    let mut d = vec![
        DictionaryEntry {
            term: "qrtz".into(),
            aliases: vec!["cue arts".into(), "quartz".into()],
        },
        DictionaryEntry {
            term: "Postgres".into(),
            aliases: vec!["post gres".into()],
        },
    ];
    for i in 0..18 {
        d.push(DictionaryEntry {
            term: format!("Term{i}"),
            aliases: vec![format!("term number {i}")],
        });
    }
    d
}

/// The median of 41 timed runs after 5 warm-ups, checked against the budget (debug builds get [`DEBUG_FACTOR`]).
fn assert_fits_budget(text: &str, what: &str) {
    let dict = dictionary();
    for _ in 0..5 {
        cleanup_dictation(text, &dict, true);
    }
    let mut ms: Vec<f64> = (0..41)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(cleanup_dictation(std::hint::black_box(text), &dict, true));
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    let median = ms[ms.len() / 2];
    let ceiling = if cfg!(debug_assertions) {
        BUDGET_MS * DEBUG_FACTOR
    } else {
        BUDGET_MS
    };
    println!(
        "cleanup on {} words{what}: median {median:.3} ms, max {:.3} ms (ceiling {ceiling} ms)",
        text.split_whitespace().count(),
        ms[ms.len() - 1]
    );
    assert!(
        median <= ceiling,
        "cleanup took {median:.3} ms (median) on a long dictation{what}; budget {ceiling} ms"
    );
}

/// One test, so the timed runs never share the CPU with each other.
#[test]
fn the_cleanup_chain_fits_its_budget_on_a_long_dictation() {
    assert_fits_budget(&long_dictation(), "");
    // ADR 0196: the same dictation ending in an unmarked question, so the terminal step runs the whole question
    // check on its last sentence and fires.
    let text = format!(
        "{}Thanks. Can you review the release notes before Friday",
        long_dictation()
    );
    assert!(cleanup_dictation(&text, &dictionary(), true)
        .text
        .ends_with("before Friday?"));
    assert_fits_budget(&text, " ending in a question");
    // ADR 0206: ending in "Hey <dictionary name> <question>", so the vocative check runs in full (after the whole
    // ADR 0196 check said no) and fires.
    let text = format!(
        "{}Thanks. Hey Postgres can you take the backup tonight",
        long_dictation()
    );
    assert!(cleanup_dictation(&text, &dictionary(), true)
        .text
        .ends_with("Hey Postgres, can you take the backup tonight?"));
    assert_fits_budget(&text, " ending in a vocative question");
    // ADR 0227: ending in a tag question, so the ADR 0196 and ADR 0206 checks run and say no, then the tag check
    // runs and fires.
    let text = format!("{}Thanks. The backup ran last night, didn't it", long_dictation());
    assert!(cleanup_dictation(&text, &dictionary(), true)
        .text
        .ends_with("The backup ran last night, didn't it?"));
    assert_fits_budget(&text, " ending in a tag question");
    // ADR 0229: ending in "Please … , will you", so the tag check runs its opener test and fires.
    let text = format!(
        "{}Thanks. Please take the backup tonight, will you",
        long_dictation()
    );
    assert!(cleanup_dictation(&text, &dictionary(), true)
        .text
        .ends_with("Please take the backup tonight, will you?"));
    assert_fits_budget(&text, " ending in an imperative tag");
    // ADR 0230: ending in a spoken date, so the date scan (which already reads every text with "May" in it) fires.
    let text = format!(
        "{}Thanks. Let's meet on October twenty first at noon",
        long_dictation()
    );
    assert!(cleanup_dictation(&text, &dictionary(), true)
        .text
        .ends_with("Let's meet on October 21st at noon."));
    assert_fits_budget(&text, " ending in a spoken date");
    // ADR 0232: ending in a question that opens with a wh-contraction, so the contraction path runs.
    let text = format!("{}Thanks. What's the plan for tonight", long_dictation());
    assert!(cleanup_dictation(&text, &dictionary(), true)
        .text
        .ends_with("What's the plan for tonight?"));
    assert_fits_budget(&text, " ending in a wh-contraction question");
    // ADR 0233: ending in a question whose auxiliary is contracted onto the modifier ("What time's").
    let text = format!("{}Thanks. What time's the meeting tomorrow", long_dictation());
    assert!(cleanup_dictation(&text, &dictionary(), true)
        .text
        .ends_with("What time's the meeting tomorrow?"));
    assert_fits_budget(&text, " ending in a modifier-contraction question");
    // ADR 0234: spoken titles throughout ("Mister and Missus Smith" in every repeat of the dictation) and at its end,
    // so the title scan reads every word of the text and fires on each.
    let text = format!(
        "{}Thanks. I spoke with Mister Patel about the lease",
        long_dictation().replace("Mr. and Mrs. Smith", "Mister and Missus Smith")
    );
    let out = cleanup_dictation(&text, &dictionary(), true).text;
    assert!(out.ends_with("I spoke with Mr. Patel about the lease.") && !out.contains("Mister"));
    assert_fits_budget(&text, " with spoken titles");
    // ADR 0235: a filler sentence ("Um.") before every sentence of the dictation that starts with a capital, so the
    // opening-filler scan fires all through the text.
    let mut body = String::new();
    for (i, part) in long_dictation().split(". ").enumerate() {
        if i > 0 {
            body.push_str(if part.starts_with(|c: char| c.is_ascii_uppercase()) {
                ". Um. "
            } else {
                ". "
            });
        }
        body.push_str(part);
    }
    let text = format!("Um. {body} Thanks. Um. Can you send it");
    let out = cleanup_dictation(&text, &dictionary(), true).text;
    let plain = cleanup_dictation(
        &format!("{} Thanks. Can you send it", long_dictation()),
        &dictionary(),
        true,
    )
    .text;
    assert!(text.matches("Um. ").count() > 20);
    assert_eq!(out, plain);
    assert_fits_budget(&text, " with filler sentences");
    // ADR 0236: every comma of the dictation doubled (",, "), so the comma-run scan fires all through the text; no
    // ", ," may be left where a word follows.
    let text = format!(
        "{} Thanks. We could ship it on Friday,, but the tests are still red",
        long_dictation().replace(", ", ",, ")
    );
    let out = cleanup_dictation(&text, &dictionary(), true).text;
    assert!(text.matches(",, ").count() > 20);
    assert!(out.ends_with("We could ship it on Friday, but the tests are still red."));
    assert!(
        !out.as_bytes()
            .windows(4)
            .any(|w| w[..3] == *b", ," && w[3] == b' '),
        "{out}"
    );
    assert_fits_budget(&text, " with comma runs");
    // ADR 0238: many ",\u{201D}" pairs so SPACE_AFTER_PUNCT sees a curly close after each comma; no ", ”" left.
    let mut body = String::new();
    for (i, part) in long_dictation().split(". ").enumerate() {
        if i > 0 {
            body.push_str(". ");
        }
        body.push_str(part);
        if part.starts_with(|c: char| c.is_ascii_uppercase()) {
            body.push_str(" \u{201C}ok,\u{201D}");
        }
    }
    let text = format!("{body} Thanks. The plan was \u{201C}ship it Friday,\u{201D} then cut the release");
    let out = cleanup_dictation(&text, &dictionary(), true).text;
    assert!(text.matches(",\u{201D}").count() > 10);
    assert!(
        !out.contains(", \u{201D}") && !out.contains(", \u{2019}"),
        "{out}"
    );
    assert!(out.ends_with("The plan was \u{201C}ship it Friday,\u{201D} then cut the release."));
    assert_fits_budget(&text, " with curly closing quotes after commas");
}
