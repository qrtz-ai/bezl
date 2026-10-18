//! Echo tuning (ADR 0099, docs/echo/dictation-tuning.md): scoring for the helper's `--echo-bench` report. The helper
//! scores the recogniser's raw text; this adds what talk does to it before the insert (the rules cleanup and the
//! dictionary, [`cleanup_dictation`]), so the cleanup's effect on content and on formatting is measured on the same
//! clips. Pure functions over local JSON; `examples/echo_score.rs` is the command line. The word normalisation is
//! the helper's `EchoWER` (QrtzEcho/EchoBench.swift), kept equal by the unit tests on both sides (the
//! typographic-quote folding of N162 is Rust-only until the helper's side takes it, review §12.31).
use crate::cleanup::cleanup_dictation;
use crate::contract::DictionaryEntry;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const UNITS: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];

fn tens(w: &str) -> Option<u32> {
    Some(match w {
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "fifty" => 50,
        "sixty" => 60,
        "seventy" => 70,
        "eighty" => 80,
        "ninety" => 90,
        _ => return None,
    })
}

/// N162: a reference typed on a Mac (TextEdit, Notes) gets smart quotes, the recogniser and talk write ASCII. Both
/// sides are scored with ASCII apostrophes and quotes, so "Don’t" and "don't" are the same word.
fn ascii_quotes(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{02BC}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            c => c,
        })
        .collect()
}

/// Content words: lowercased, punctuation dropped (apostrophes kept inside words, typographic ones as ASCII), `%` as
/// "percent", number words 0–99 as digits.
pub fn normalize(s: &str) -> Vec<String> {
    let mut cleaned = String::new();
    for ch in ascii_quotes(s).to_lowercase().chars() {
        if ch.is_alphanumeric() || ch == '\'' {
            cleaned.push(ch);
        } else if ch == '%' {
            cleaned.push_str(" percent ");
        } else {
            cleaned.push(' ');
        }
    }
    let raw: Vec<&str> = cleaned
        .split(' ')
        .map(|w| w.trim_matches('\''))
        .filter(|w| !w.is_empty())
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let w = raw[i];
        if let Some(t) = tens(w) {
            if let Some(u) = raw.get(i + 1).and_then(|n| UNITS.iter().position(|x| x == n)) {
                if (1..10).contains(&u) {
                    out.push((t + u as u32).to_string());
                    i += 2;
                    continue;
                }
            }
            out.push(t.to_string());
        } else if let Some(u) = UNITS.iter().position(|x| *x == w) {
            out.push(u.to_string());
        } else {
            out.push(w.to_owned());
        }
        i += 1;
    }
    out
}

/// Formatted words: whitespace tokens as written (case and punctuation count; typographic quotes as ASCII, N162).
pub fn formatted(s: &str) -> Vec<String> {
    ascii_quotes(s).split_whitespace().map(str::to_owned).collect()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Score {
    pub ref_words: usize,
    pub sub: usize,
    pub del: usize,
    pub ins: usize,
}

impl Score {
    pub fn errors(&self) -> usize {
        self.sub + self.del + self.ins
    }
    pub fn wer(&self) -> f64 {
        if self.ref_words > 0 {
            self.errors() as f64 / self.ref_words as f64
        } else if self.errors() > 0 {
            1.0
        } else {
            0.0
        }
    }
    pub fn add(&mut self, o: Score) {
        self.ref_words += o.ref_words;
        self.sub += o.sub;
        self.del += o.del;
        self.ins += o.ins;
    }
    pub fn json(&self) -> Value {
        json!({"refWords": self.ref_words, "sub": self.sub, "del": self.del, "ins": self.ins, "wer": self.wer()})
    }
}

/// Word edit distance split into substitutions, deletions and insertions (same tie-breaking as the helper).
pub fn score(r: &[String], h: &[String]) -> Score {
    let (n, m) = (r.len(), h.len());
    if n == 0 || m == 0 {
        return Score {
            ref_words: n,
            sub: 0,
            del: n,
            ins: m,
        };
    }
    // (cost, sub, del, ins)
    let mut prev: Vec<(usize, usize, usize, usize)> = (0..=m).map(|j| (j, 0, 0, j)).collect();
    for a in 1..=n {
        let mut cur = vec![(a, 0, a, 0)];
        for b in 1..=m {
            let (diag, up, left) = (prev[b - 1], prev[b], cur[b - 1]);
            let mut best = if r[a - 1] == h[b - 1] {
                diag
            } else {
                (diag.0 + 1, diag.1 + 1, diag.2, diag.3)
            };
            if up.0 + 1 < best.0 {
                best = (up.0 + 1, up.1, up.2 + 1, up.3);
            }
            if left.0 + 1 < best.0 {
                best = (left.0 + 1, left.1, left.2, left.3 + 1);
            }
            cur.push(best);
        }
        prev = cur;
    }
    let x = prev[m];
    Score {
        ref_words: n,
        sub: x.1,
        del: x.2,
        ins: x.3,
    }
}

#[derive(Default)]
struct Agg {
    clips: usize,
    raw: Score,
    raw_fmt: Score,
    clean: Score,
    clean_fmt: Score,
    /// Clips whose cleaned text equals the reference exactly (what was meant to be typed).
    exact: usize,
}

/// Scores every run of a `--echo-bench` report raw and after talk's cleanup (`auto_cleanup` on) with `dict`.
/// Returns `{summary: [...], runs: [{clip, engine, variant, cleaned, raw, cleanedScore, ...}]}`.
pub fn rescore(report: &Value, dict: &[DictionaryEntry]) -> Value {
    let mut groups: BTreeMap<(String, String), Agg> = BTreeMap::new();
    let mut runs = Vec::new();
    for r in report["runs"].as_array().into_iter().flatten() {
        let reference = r["reference"].as_str().unwrap_or_default();
        let hyp = r["hypothesis"].as_str().unwrap_or_default();
        let cleaned = cleanup_dictation(hyp, dict, true).text;
        let key = (
            r["engine"].as_str().unwrap_or("?").to_owned(),
            r["variant"].as_str().unwrap_or("?").to_owned(),
        );
        let s = [
            score(&normalize(reference), &normalize(hyp)),
            score(&formatted(reference), &formatted(hyp)),
            score(&normalize(reference), &normalize(&cleaned)),
            score(&formatted(reference), &formatted(&cleaned)),
        ];
        let g = groups.entry(key).or_default();
        g.clips += 1;
        g.raw.add(s[0]);
        g.raw_fmt.add(s[1]);
        g.clean.add(s[2]);
        g.clean_fmt.add(s[3]);
        if cleaned.trim() == reference.trim() {
            g.exact += 1;
        }
        runs.push(json!({
            "clip": r["clip"], "engine": r["engine"], "variant": r["variant"], "cleaned": cleaned,
            "raw": s[0].json(), "rawFormatted": s[1].json(), "cleanedScore": s[2].json(), "cleanedFormatted": s[3].json(),
        }));
    }
    let summary: Vec<Value> = groups
        .into_iter()
        .map(|((engine, variant), g)| {
            json!({
                "engine": engine, "variant": variant, "clips": g.clips,
                "wer": g.raw.wer(), "formattedWer": g.raw_fmt.wer(),
                "cleanedWer": g.clean.wer(), "cleanedFormattedWer": g.clean_fmt.wer(),
                "exactAfterCleanup": g.exact,
            })
        })
        .collect();
    json!({"summary": summary, "runs": runs})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn normalize_matches_the_helper() {
        // Same cases as EchoBenchTests.testNormalizeFoldsCasePunctuationAndNumbers.
        assert_eq!(
            normalize("Move it to Thursday, at twenty five past 3!"),
            w("move it to thursday at 25 past 3")
        );
        assert_eq!(normalize("Don't stop—it's 50%."), w("don't stop it's 50 percent"));
    }

    #[test]
    fn typographic_quotes_score_as_ascii() {
        // N162: a reference with smart quotes (macOS TextEdit) against the recogniser's ASCII.
        assert_eq!(
            normalize("Don\u{2019}t stop, it\u{2019}s fine"),
            w("don't stop it's fine")
        );
        assert_eq!(
            score(&normalize("I\u{2019}ll be late"), &normalize("I'll be late")).errors(),
            0
        );
        assert_eq!(
            score(
                &formatted("\u{201C}Ship it,\u{201D} he said."),
                &formatted("\"Ship it,\" he said.")
            )
            .errors(),
            0
        );
    }

    #[test]
    fn edits_split_by_kind() {
        let s = score(&w("a b c d"), &w("a x c d e"));
        assert_eq!((s.sub, s.del, s.ins), (1, 0, 1));
        assert_eq!(score(&w("a b c"), &w("a")).del, 2);
        assert_eq!(score(&[], &[]).wer(), 0.0);
    }

    #[test]
    fn rescore_applies_the_rules_and_the_dictionary() {
        let report = json!({"runs": [
            {"clip": "c", "engine": "e", "variant": "shipped", "reference": "Ship qrtz to everyone today.", "hypothesis": "um ship cue arts to everyone today"},
        ]});
        let dict = vec![DictionaryEntry {
            term: "qrtz".into(),
            aliases: vec!["cue arts".into()],
        }];
        let r = rescore(&report, &dict);
        assert_eq!(r["runs"][0]["cleaned"], "Ship qrtz to everyone today.");
        let s = &r["summary"][0];
        assert_eq!(s["cleanedWer"], 0.0);
        assert_eq!(s["exactAfterCleanup"], 1);
        assert!(s["wer"].as_f64().unwrap() > 0.0);
    }
}
