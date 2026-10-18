//! `cargo run -p qrtz-talk --example echo_score -- <echo-bench report.json> [--dictionary dict.json]`
//!
//! Scores a `qrtz-helper --echo-bench` report (ADR 0099) after talk's rules cleanup and dictionary, prints one line
//! per engine × variant, and writes `<report>.scored.json` next to the report. Local files only.
use qrtz_talk::bench::rescore;
use qrtz_talk::DictionaryEntry;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: echo_score <report.json> [--dictionary dict.json]");
        std::process::exit(2);
    };
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("read report")).expect("report is JSON");
    let dict: Vec<DictionaryEntry> = args
        .iter()
        .position(|a| a == "--dictionary")
        .and_then(|i| args.get(i + 1))
        .map(|p| {
            serde_json::from_slice(&std::fs::read(p).expect("read dictionary")).expect("dictionary JSON")
        })
        .unwrap_or_default();
    let scored = rescore(&report, &dict);
    println!(
        "{:<24} {:<22} {:>5} {:>8} {:>8} {:>9} {:>9} {:>6}",
        "engine", "variant", "clips", "wer", "fmt-wer", "clean-wer", "clean-fmt", "exact"
    );
    for s in scored["summary"].as_array().into_iter().flatten() {
        println!(
            "{:<24} {:<22} {:>5} {:>7.1}% {:>7.1}% {:>8.1}% {:>8.1}% {:>6}",
            s["engine"].as_str().unwrap_or("?"),
            s["variant"].as_str().unwrap_or("?"),
            s["clips"],
            100.0 * s["wer"].as_f64().unwrap_or(0.0),
            100.0 * s["formattedWer"].as_f64().unwrap_or(0.0),
            100.0 * s["cleanedWer"].as_f64().unwrap_or(0.0),
            100.0 * s["cleanedFormattedWer"].as_f64().unwrap_or(0.0),
            s["exactAfterCleanup"],
        );
    }
    let out = format!("{}.scored.json", path.trim_end_matches(".json"));
    std::fs::write(&out, serde_json::to_vec_pretty(&scored).expect("serialize"))
        .expect("write scored report");
    println!("{out}");
}
