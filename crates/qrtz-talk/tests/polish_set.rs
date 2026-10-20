#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0182: the Echo polish test set (spec/vectors/talk/polish.json), scored on every run.
//! - `keep`: false-positive vectors. Text where a rule's pattern appears but the words must stay; the cleanup must
//!   produce `expect` exactly (zero hits on the false-positive set). A vector with `"dict": true` runs with the file's
//!   dictionary (ADR 0206's names), the others with none.
//! - `polish`: recogniser-style raw text and the hand-written ideal. Scored as exact matches and character edit
//!   distance; a case whose distance to its ideal grows against spec/vectors/talk/polish-baseline.json is a
//!   regression, and the regression count must stay 0. `QRTZ_POLISH_BASELINE=1` rewrites the baseline after an
//!   accepted change. The score is printed (`--nocapture`) and recorded in docs/echo/dictation-tuning.md.
use qrtz_talk::cleanup::rule_cleanup;
use qrtz_talk::DictionaryEntry;
use serde_json::Value;

const SET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/vectors/talk/polish.json");
const BASELINE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../spec/vectors/talk/polish-baseline.json"
);

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

fn load(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn no_false_positive_rewrites() {
    let v = load(SET);
    let dict: Vec<DictionaryEntry> = serde_json::from_value(v["dictionary"].clone()).unwrap();
    let keep = v["keep"].as_array().unwrap();
    let bad: Vec<String> = keep
        .iter()
        .filter_map(|k| {
            let (raw, want) = (k["raw"].as_str().unwrap(), k["expect"].as_str().unwrap());
            // ADR 0206: a vector with `"dict": true` runs with the file's dictionary (its names), others with none.
            let d: &[DictionaryEntry] = if k["dict"] == true { &dict } else { &[] };
            let got = rule_cleanup(raw, d);
            (got != want).then(|| format!("{} {raw:?}: got {got:?}, want {want:?}", k["id"]))
        })
        .collect();
    assert!(keep.len() >= 40, "the false-positive set shrank");
    assert!(
        bad.is_empty(),
        "{} false-positive hits:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn polish_score_never_regresses() {
    let v = load(SET);
    let dict: Vec<DictionaryEntry> = serde_json::from_value(v["dictionary"].clone()).unwrap();
    let mut base = load(BASELINE);
    let update = std::env::var("QRTZ_POLISH_BASELINE").as_deref() == Ok("1");
    let (mut exact, mut dist, mut base_dist, mut chars) = (0, 0, 0, 0);
    let mut regressions = Vec::new();
    let cases = v["polish"].as_array().unwrap();
    for p in cases {
        let (id, raw, ideal) = (
            p["id"].as_str().unwrap(),
            p["raw"].as_str().unwrap(),
            p["ideal"].as_str().unwrap(),
        );
        let got = rule_cleanup(raw, &dict);
        let was = base["baseline"][id]
            .as_str()
            .expect("every case has a baseline")
            .to_owned();
        let (d, bd) = (edit_distance(&got, ideal), edit_distance(&was, ideal));
        exact += usize::from(d == 0);
        dist += d;
        base_dist += bd;
        chars += ideal.chars().count();
        if d > bd {
            regressions.push(format!("{id} {raw:?}: {was:?} -> {got:?} (ideal {ideal:?})"));
        }
        if update {
            base["baseline"][id] = Value::String(got);
        }
    }
    println!(
        "polish set: {exact}/{} exact, edit distance {dist} chars ({:.2} % of {chars}), baseline {base_dist}, regressions {}",
        cases.len(),
        100.0 * dist as f64 / chars as f64,
        regressions.len()
    );
    assert!(regressions.is_empty(), "regressions:\n{}", regressions.join("\n"));
    if update {
        std::fs::write(
            BASELINE,
            format!("{}\n", serde_json::to_string_pretty(&base).unwrap()),
        )
        .unwrap();
    }
}
