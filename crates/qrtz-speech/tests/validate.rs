//! Untrusted module output (ADR 0046 §3.5) and the shared protocol fixtures (spec/vectors/speech).
use qrtz_speech::protocol::{encode_host_to_speech, parse_speech_line, protocol};
use qrtz_speech::validate::{check_hello, OutputValidator, Verdict};
use qrtz_speech::SpeechEvent;
use std::path::PathBuf;

fn lines(f: &str) -> Vec<String> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/vectors/speech")
        .join(f);
    std::fs::read_to_string(p)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

#[test]
fn shared_fixtures_match_the_zod_schema() {
    let valid = lines("speech-to-host.jsonl");
    let mut types = std::collections::BTreeSet::new();
    for l in &valid {
        let v = parse_speech_line(l).unwrap_or_else(|e| panic!("{l}: {e}"));
        types.insert(v["type"].as_str().unwrap().to_owned());
    }
    assert_eq!(types, protocol().speech_to_host_types.iter().cloned().collect());
    for l in lines("invalid-speech-to-host.jsonl") {
        assert!(parse_speech_line(&l).is_err(), "{l}");
    }
    for l in lines("host-to-speech.jsonl") {
        let v: serde_json::Value = serde_json::from_str(&l).unwrap();
        assert_eq!(
            encode_host_to_speech(&v).unwrap(),
            format!("{l}\n"),
            "sorted keys, byte-identical"
        );
    }
}

fn words(utt: &str, ch: &str, fin: bool, lang: &str, w: &[(&str, u64, u64)]) -> String {
    serde_json::json!({
        "type": "words", "utteranceId": utt, "channel": ch, "final": fin, "language": lang,
        "words": w.iter().map(|(t, s, e)| serde_json::json!({"text": t, "startMs": s, "endMs": e})).collect::<Vec<_>>(),
    })
    .to_string()
}

#[test]
fn accepts_good_words_and_speakers() {
    let mut v = OutputValidator::new();
    let Verdict::Event(ev) = v.check(
        &words("u1", "mic", true, "en", &[("hi", 0, 300), ("there", 300, 700)]),
        0,
        1000,
    ) else {
        panic!()
    };
    assert_eq!(ev.text().as_deref(), Some("hi there"));
    let sp = r#"{"type":"speaker","channel":"system","speaker":"system:2","startMs":0,"endMs":900}"#;
    assert!(matches!(
        v.check(sp, 1, 1000),
        Verdict::Event(SpeechEvent::Speaker { .. })
    ));
}

#[test]
fn drops_violations_and_fails_after_three() {
    let mut v = OutputValidator::new();
    // A language outside the allowlist (valid BCP-47 shape).
    assert!(matches!(
        v.check(&words("u1", "mic", true, "de", &[("hallo", 0, 10)]), 0, 1000),
        Verdict::Dropped(_)
    ));
    // Words in the future (beyond the audio written + 1 s).
    assert!(matches!(
        v.check(&words("u2", "mic", true, "en", &[("x", 0, 5000)]), 0, 1000),
        Verdict::Dropped(_)
    ));
    // A speaker id from the other channel.
    let sp = r#"{"type":"speaker","channel":"mic","speaker":"system:0","startMs":0,"endMs":10}"#;
    assert!(matches!(v.check(sp, 0, 1000), Verdict::Fail(_)));
}

#[test]
fn timestamps_are_monotonic_per_channel() {
    let mut v = OutputValidator::new();
    assert!(matches!(
        v.check(&words("u1", "mic", true, "en", &[("a", 5000, 6000)]), 0, 7000),
        Verdict::Event(_)
    ));
    // The other channel has its own clock.
    assert!(matches!(
        v.check(&words("u2", "system", true, "en", &[("b", 100, 200)]), 0, 7000),
        Verdict::Event(_)
    ));
    // Back in time on mic: dropped.
    assert!(matches!(
        v.check(&words("u3", "mic", true, "en", &[("c", 1000, 1200)]), 0, 7000),
        Verdict::Dropped(_)
    ));
    // Words out of order inside one event: dropped.
    assert!(matches!(
        v.check(
            &words("u4", "mic", false, "en", &[("d", 6500, 6600), ("e", 6000, 6100)]),
            0,
            7000
        ),
        Verdict::Dropped(_)
    ));
}

#[test]
fn bounds_long_lines_rate_and_speaker_count() {
    let mut v = OutputValidator::new();
    let long = "x".repeat(70_000);
    assert!(matches!(v.check(&long, 0, 0), Verdict::Dropped(_)));
    let mut v = OutputValidator::new();
    let log = r#"{"type":"log","level":"info","msg":"m"}"#;
    let mut dropped = 0;
    for _ in 0..250 {
        if matches!(v.check(log, 10, 0), Verdict::Dropped(_)) {
            dropped += 1;
        }
    }
    assert_eq!(dropped, 50, "200 events per second");
    assert_eq!(v.violations(), 0, "rate limiting is not a violation");
    assert!(matches!(v.check(log, 1010, 0), Verdict::Ignore), "next second");
    let mut v = OutputValidator::new();
    for i in 0..8 {
        let sp =
            format!(r#"{{"type":"speaker","channel":"mic","speaker":"mic:{i}","startMs":0,"endMs":10}}"#);
        assert!(matches!(v.check(&sp, 0, 1000), Verdict::Event(_)));
    }
}

#[test]
fn hello_must_name_the_bundle_version_and_protocol() {
    let h = r#"{"type":"hello","protocol":1,"moduleVersion":"1.2.0","languages":["en"],"maxSpeakersPerChannel":8}"#;
    assert!(check_hello(h, "1.2.0").is_ok());
    assert!(check_hello(h, "1.3.0").is_err());
    assert!(check_hello(&h.replace("\"protocol\":1", "\"protocol\":2"), "1.2.0").is_err());
    assert!(check_hello(r#"{"type":"status","phase":"ready"}"#, "1.2.0").is_err());
}
