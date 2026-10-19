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

#[test]
fn the_cleanup_chain_fits_its_budget_on_a_long_dictation() {
    let text = long_dictation();
    let dict = dictionary();
    for _ in 0..5 {
        cleanup_dictation(&text, &dict, true);
    }
    let mut ms: Vec<f64> = (0..41)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(cleanup_dictation(std::hint::black_box(&text), &dict, true));
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
        "cleanup on {} words: median {median:.3} ms, max {:.3} ms (ceiling {ceiling} ms)",
        text.split_whitespace().count(),
        ms[ms.len() - 1]
    );
    assert!(
        median <= ceiling,
        "cleanup took {median:.3} ms (median) on a long dictation; budget {ceiling} ms"
    );
}
