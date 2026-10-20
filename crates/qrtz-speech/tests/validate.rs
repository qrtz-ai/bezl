#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Untrusted module output (ADR 0046 §3.5) and the shared protocol fixtures (spec/vectors/speech).
use qrtz_speech::protocol::{encode_host_to_speech, parse_speech_line, protocol};
use qrtz_speech::validate::{check_hello, OutputValidator, Verdict};
use qrtz_speech::{Channel, SessionKind, SessionSpec, SpeakerMode, SpeechEvent, StatusError};
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

#[test]
fn n68_dictation_sessions_reject_system_channel_events() {
    let mut v = OutputValidator::for_session("d1", &[Channel::Mic]);
    assert!(matches!(
        v.check(&words("u1", "system", true, "en", &[("x", 0, 10)]), 0, 1000),
        Verdict::Dropped(_)
    ));
    let sp = r#"{"type":"speaker","channel":"system","speaker":"system:0","startMs":0,"endMs":10}"#;
    assert!(matches!(v.check(sp, 0, 1000), Verdict::Dropped(_)));
    assert!(matches!(
        v.check(&words("u2", "mic", true, "en", &[("x", 0, 10)]), 0, 1000),
        Verdict::Event(_)
    ));
}

#[test]
fn n78_status_text_never_reaches_clients_and_finished_is_checked() {
    let mut v = OutputValidator::for_session("s1", &[Channel::Mic, Channel::System]);
    let st = r#"{"type":"status","phase":"failed","code":"load-failed","detail":"<b>click here</b>"}"#;
    let Verdict::Event(ev) = v.check(st, 0, 0) else {
        panic!()
    };
    assert_eq!(
        ev,
        SpeechEvent::Status {
            phase: qrtz_speech::engine::StatusPhase::Failed,
            error: Some(StatusError::LoadFailed)
        }
    );
    assert!(!format!("{ev:?}").contains("click"));
    assert!(matches!(
        v.check(r#"{"type":"finished","sessionId":"other","durationMs":0}"#, 0, 0),
        Verdict::Dropped(_)
    ));
    assert!(matches!(
        v.check(
            r#"{"type":"finished","sessionId":"s1","durationMs":99999}"#,
            0,
            1000
        ),
        Verdict::Dropped(_)
    ));
    let spec = |hint: Option<&str>| SessionSpec {
        session_id: "s1".into(),
        kind: SessionKind::Recording,
        channels: vec![Channel::Mic, Channel::System],
        language_hint: hint.map(String::from),
        speaker_mode: SpeakerMode::Fast,
    };
    assert!(qrtz_speech::protocol::start_message(&spec(Some("en-US")), &[("m/a.bin".into(), 7, 3)]).is_ok());
    assert!(qrtz_speech::protocol::start_message(&spec(Some("de")), &[]).is_err());
    assert!(qrtz_speech::protocol::start_message(&spec(None), &[("m/a.bin".into(), 9, 3)]).is_err());
}

#[tokio::test]
async fn n77_line_reads_are_bounded_before_allocation() {
    use qrtz_speech::protocol::{read_bounded_line, LineTooLong};
    let mut data = "x".repeat(200_000).into_bytes();
    data.extend_from_slice(b"\n{\"ok\":1}\n");
    let mut r = tokio::io::BufReader::with_capacity(4096, &data[..]);
    assert_eq!(
        read_bounded_line(&mut r, 65_536).await.unwrap(),
        Some(Err(LineTooLong))
    );
    assert_eq!(
        read_bounded_line(&mut r, 65_536).await.unwrap(),
        Some(Ok("{\"ok\":1}".to_string()))
    );
    assert_eq!(read_bounded_line(&mut r, 65_536).await.unwrap(), None);
}
