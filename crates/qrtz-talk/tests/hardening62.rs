//! ADR 0201 (echo cycle 62): the comma the recogniser wrote before each spoken "new line" of a dictated list goes
//! ("Groceries, new line, eggs, new line, milk." → "Groceries⏎Eggs⏎Milk"), only when the whole text is a list of
//! 1-2 word lines of plain lowercase common words with a comma on every line but the last. Greetings, sign-offs,
//! names, addresses, verse, digits, apostrophes, inner commas and partial lists keep every comma. Fails without the
//! fix (the positives kept their commas: polish case p49 was 2 chars off).
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
fn a_dictated_list_loses_its_line_commas() {
    for (raw, want) in [
        (
            "Groceries, new line, eggs, new line, milk.",
            "Groceries\nEggs\nMilk",
        ),
        (
            "To do, new line, call mom, new line, pay rent.",
            "To do\nCall mom\nPay rent",
        ),
        (
            "Shopping, new line, olive oil, new line, rice, new line, paper towels",
            "Shopping\nOlive oil\nRice\nPaper towels",
        ),
        (
            "Options, new line, red, new line, blue, new line, green.",
            "Options\nRed\nBlue\nGreen",
        ),
        (
            "Groceries, new line, eggs, new line, milk",
            "Groceries\nEggs\nMilk",
        ),
        ("Groceries,\neggs,\nmilk.", "Groceries\nEggs\nMilk"),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}

#[test]
fn meant_commas_and_anything_unclear_stay() {
    for (raw, want) in [
        (
            "Hi team, new line, thanks, new line, Jared",
            "Hi team,\nThanks,\nJared",
        ),
        (
            "Dear team, new line, quick update, new line, lunch moved.",
            "Dear team,\nQuick update,\nLunch moved",
        ),
        ("Thanks, new line, love, new line, mom", "Thanks,\nLove,\nMom"),
        (
            "Cheers, new line, see you, new line, soon.",
            "Cheers,\nSee you,\nSoon",
        ),
        (
            "Talk soon, new line, take care, new line, bye",
            "Talk soon,\nTake care,\nBye",
        ),
        (
            "Hey Sam, new line, quick one, new line, call me.",
            "Hey Sam,\nQuick one,\nCall me",
        ),
        (
            "Morning all, new line, standup moved, new line, sorry.",
            "Morning all,\nStandup moved,\nSorry",
        ),
        (
            "Kind regards, new line, the cue arts team, new line, San Francisco",
            "Kind regards,\nThe qrtz team,\nSan Francisco",
        ),
        (
            "Jared Grimes, new line, Naples Street, new line, San Francisco",
            "Jared Grimes,\nNaples Street,\nSan Francisco",
        ),
        (
            "Groceries, new line, eggs, new line, Sam",
            "Groceries,\nEggs,\nSam",
        ),
        (
            "Roses are red, new line, violets are blue, new line, sugar is sweet.",
            "Roses are red,\nViolets are blue,\nSugar is sweet",
        ),
        (
            "Twinkle twinkle, new line, little star, new line, how I wonder.",
            "Twinkle twinkle,\nLittle star,\nHow I wonder",
        ),
        (
            "Hello, new line, it's me, new line, Jared.",
            "Hello,\nIt's me,\nJared",
        ),
        (
            "Parts, new line, two bolts, new line, 4 nuts.",
            "Parts,\nTwo bolts,\n4 nuts",
        ),
        (
            "Agenda, new line, Q3 numbers, new line, hiring.",
            "Agenda,\nQ3 numbers,\nHiring",
        ),
        (
            "Groceries, new line, eggs, milk, new line, bread.",
            "Groceries,\nEggs, milk,\nBread",
        ),
        (
            "Groceries, new line, eggs new line milk",
            "Groceries,\nEggs\nMilk",
        ),
        (
            "Groceries, new line, eggs, new line, milk!",
            "Groceries,\nEggs,\nMilk!",
        ),
        (
            "Groceries, new paragraph, eggs, new line, milk, new line, bread.",
            "Groceries,\n\nEggs,\nMilk,\nBread.",
        ),
        (
            "Agenda, new line, budget, new line, hiring, new line, what do we do next?",
            "Agenda,\nBudget,\nHiring,\nWhat do we do next?",
        ),
        ("Eggs, new line, milk.", "Eggs,\nMilk."),
        // An all-lowercase recogniser gives no casing signal, so nothing changes.
        (
            "groceries, new line, eggs, new line, milk",
            "Groceries,\nEggs,\nMilk",
        ),
    ] {
        assert_eq!(clean(raw), want, "{raw:?}");
    }
}
