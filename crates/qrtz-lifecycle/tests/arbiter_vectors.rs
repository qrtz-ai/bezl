//! Runs the shared helper-arbiter vectors (spec/vectors/helper/arbiter.json + arbiter-edges.json) against the Rust
//! port. The same files are run by vitest (packages/core/src/helper/arbiter*.test.ts) and the Swift helper tests,
//! so passing here means the Tauri host and the helper agree on every accept/ignore/refuse, effect and degrade.

use std::path::PathBuf;

use qrtz_lifecycle::model::{Degradation, OsPermission};
use qrtz_lifecycle::{arbiter_step, degradations_for_revocation, ArbiterEvent, ArbiterState};
use serde_json::Value;

fn load(file: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/vectors/helper")
        .join(file);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .unwrap()
}

fn run(file: &str) -> usize {
    let v = load(file);
    let mut steps = 0;
    for c in v["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let denied: Vec<OsPermission> = serde_json::from_value(c["initialDenied"].clone()).unwrap();
        let mut s = ArbiterState::initial(denied);
        for (i, st) in c["steps"].as_array().unwrap().iter().enumerate() {
            let e: ArbiterEvent = serde_json::from_value(st["event"].clone())
                .unwrap_or_else(|err| panic!("{name} step {i}: {err}"));
            assert_eq!(
                serde_json::to_value(e).unwrap(),
                st["event"],
                "{name} step {i}: event round trip"
            );
            let r = arbiter_step(&s, &e);
            assert_eq!(
                serde_json::to_value(&r).unwrap(),
                st["expect"],
                "{file}: {name} step {i} {}",
                st["event"]
            );
            s = r.state;
            steps += 1;
        }
    }
    steps
}

#[test]
fn arbiter_vectors_match_ts_and_swift() {
    assert_eq!(
        load("arbiter.json")["cases"].as_array().unwrap().len(),
        13,
        "coverage changed: update this count"
    );
    assert!(run("arbiter.json") > 20);
}

#[test]
fn arbiter_edge_vectors_match_ts_and_swift() {
    assert!(run("arbiter-edges.json") > 5);
}

#[test]
fn revocation_degradations_cover_every_pair() {
    use OsPermission::*;
    let d = |p, prior: &[OsPermission]| degradations_for_revocation(p, prior);
    assert_eq!(d(Microphone, &[]), vec![Degradation::NoMic]);
    assert_eq!(
        d(Microphone, &[ScreenRecording]),
        vec![Degradation::AudioUnavailable]
    );
    assert_eq!(d(ScreenRecording, &[]), vec![Degradation::MicOnly]);
    assert_eq!(
        d(ScreenRecording, &[Microphone]),
        vec![Degradation::AudioUnavailable]
    );
    assert_eq!(
        d(Accessibility, &[Microphone, ScreenRecording]),
        vec![Degradation::AxUnavailable]
    );
}
