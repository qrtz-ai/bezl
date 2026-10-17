//! The qrtz speech seam (`qrtz_speech::SpeechEngine`): version handshake, untrusted output, crash and update
//! mid-session, against a mock module that validates its lines like the host's on-device engine does.
mod common;
use common::*;
use qrtz_talk::qrtz_speech::engine::SpeechWordOut;
use qrtz_talk::qrtz_speech::protocol::SPEECH_PROTOCOL_VERSION;
use qrtz_talk::qrtz_speech::{Channel, SpeechEvent};
use qrtz_talk::*;
use serde_json::json;

fn with_module(version: &str) -> RigOpts {
    RigOpts {
        helper: HelperOpts {
            audio_every_ms: 50,
            module: Some((SPEECH_PROTOCOL_VERSION, version.into())),
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn start_and_hear(r: &Rig) -> String {
    let me = r.svc.clone();
    tokio::spawn(async move { me.start().await });
    until(|| r.view().partial.split(' ').count() >= 3, 1000).await;
    r.view().id.unwrap()
}

#[tokio::test(start_paused = true)]
async fn module_words_drive_on_device_dictation() {
    let r = rig(with_module("2.1.0"));
    let v = r.dictate(400).await;
    let e = v.last.unwrap();
    assert_eq!(e.engine, Engine::OnDevice);
    assert!(!e.raw.is_empty());
    assert_eq!(
        r.speech.as_ref().unwrap().started(),
        [("t1".to_string(), "2.1.0".to_string())]
    );
}

#[tokio::test(start_paused = true)]
async fn incompatible_protocol_is_refused_before_the_mic_opens() {
    let mut o = with_module("9.0.0");
    o.helper.module = Some((SPEECH_PROTOCOL_VERSION + 1, "9.0.0".into()));
    let r = rig(o);
    let v = r.svc.start().await;
    assert_eq!(v.phase, TalkPhase::Error);
    assert_eq!(v.error_kind, Some(TalkErrorKind::SpeechIncompatible));
    assert!(
        r.helper.sent_of("dictation.start").is_empty(),
        "the mic never opened"
    );
}

#[tokio::test(start_paused = true)]
async fn module_not_downloaded_is_typed() {
    let r = rig(with_module("2.1.0"));
    r.speech.as_ref().unwrap().install(None);
    let v = r.svc.start().await;
    assert_eq!(v.error_kind, Some(TalkErrorKind::SpeechUnavailable));
    assert!(r.helper.sent_of("dictation.start").is_empty());
}

#[tokio::test(start_paused = true)]
async fn incompatible_module_with_an_old_cloud_pick_is_refused_with_no_fallback() {
    let mut o = with_module("9.0.0");
    o.helper.module = Some((SPEECH_PROTOCOL_VERSION + 1, "9.0.0".into()));
    o.config = json!({"talk": {"engine": "elevenlabs"}});
    let r = rig(o);
    let v = r.svc.start().await;
    assert_eq!(v.error_kind, Some(TalkErrorKind::SpeechIncompatible));
    assert!(r.helper.sent_of("dictation.start").is_empty());
    assert!(r.svc.history().is_empty());
}

#[tokio::test(start_paused = true)]
async fn malformed_and_oversized_output_is_dropped_then_the_session_ends_cleanly() {
    let r = rig(with_module("2.1.0"));
    let id = start_and_hear(&r).await;
    let bad = [
        json!({"type": "words", "utteranceId": "u1", "final": false, "language": "en", "words": [{"text": "x".repeat(100_000), "startMs": 0, "endMs": 1}]}),
        json!({"type": "words", "utteranceId": "u1", "channel": "tape", "final": false, "language": "en", "words": [{"text": "hi", "startMs": 0, "endMs": 1}]}),
        json!({"type": "words", "utteranceId": "u1", "final": "yes", "language": "en", "words": 42}),
    ];
    for b in &bad[..2] {
        r.helper.module_line(b.clone());
    }
    sleep(1).await;
    assert!(!r.view().partial.contains("hi") && !r.view().partial.contains("xxxx"));
    assert_eq!(r.view().phase, TalkPhase::Listening, "two violations are dropped");
    // The third: the engine fails the session, the dictation ends with what it heard.
    let good = r.view().partial.clone();
    r.helper.module_line(bad[2].clone());
    until(|| r.view().phase == TalkPhase::Error, 1000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::SpeechLost));
    assert!(r
        .helper
        .sent_of("dictation.cancel")
        .iter()
        .any(|m| m["id"] == id.as_str()));
    let e = r.last_entry().expect("heard words survive");
    assert!(!e.raw.is_empty() && (good.starts_with(&e.raw) || e.raw.starts_with(&good)));
}

fn words(channel: Channel, fin: bool, ws: &[(&str, u64, u64)]) -> SpeechEvent {
    SpeechEvent::Words {
        utterance_id: "u1".into(),
        channel,
        is_final: fin,
        language: "en".into(),
        words: ws
            .iter()
            .map(|(t, s, e)| SpeechWordOut {
                text: (*t).into(),
                start_ms: *s,
                end_ms: *e,
            })
            .collect(),
    }
}

#[test]
fn talk_validator_rejects_hostile_events_even_past_the_engine() {
    let mut v = TalkValidator::new(&[Channel::Mic]);
    let h = v
        .words(&words(
            Channel::Mic,
            true,
            &[("hello", 0, 300), ("there", 310, 600)],
        ))
        .unwrap();
    assert_eq!(h.text, "hello there");
    assert!(h.is_final);
    let long = "w".repeat(65);
    let many: Vec<(&str, u64, u64)> = (0..65).map(|i| ("w", 1000 + i, 1000 + i)).collect();
    let cases = [
        words(Channel::System, false, &[("a", 1000, 1001)]), // not this session's channel
        words(Channel::Mic, true, &[("back", 0, 100)]),      // before the last final
        words(Channel::Mic, false, &[("a", 900, 800)]),      // ends before it starts
        words(Channel::Mic, false, &[("a\u{7}", 1000, 1001)]), // control character
        words(Channel::Mic, false, &[(long.as_str(), 1000, 1001)]), // word too long
        words(Channel::Mic, false, &many),                   // too many words
        words(Channel::Mic, false, &[("a", 1000, 99 * 3600 * 1000)]), // absurd time
        SpeechEvent::Words {
            utterance_id: String::new(),
            channel: Channel::Mic,
            is_final: false,
            language: "en".into(),
            words: vec![],
        },
        SpeechEvent::Words {
            utterance_id: "u".into(),
            channel: Channel::Mic,
            is_final: false,
            language: "x".repeat(40),
            words: vec![],
        },
    ];
    for (i, c) in cases.iter().enumerate() {
        assert!(v.words(c).is_err(), "case {i} must be rejected: {c:?}");
    }
    assert_eq!(v.rejected, cases.len() as u32);
    assert_eq!(
        speech::bound_final(&"y".repeat(30_000)).chars().count(),
        speech::MAX_FINAL_CHARS
    );
    assert_eq!(speech::bound_final("a\u{0000}b"), "a b");
}

#[test]
fn talk_reads_no_engine_metadata() {
    // N72: talk never reads info() JSON nor shows model or engine metadata; the handshake is start().
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/service.rs")).unwrap();
    assert!(
        !src.contains(".info()"),
        "talk must not read SpeechEngine::info()"
    );
    assert!(!src.contains("StoreStatus"));
}

#[tokio::test(start_paused = true)]
async fn module_crash_mid_dictation_ends_cleanly_and_keeps_heard_words() {
    let r = rig(with_module("2.1.0"));
    let id = start_and_hear(&r).await;
    r.helper.module_crash();
    until(|| r.view().phase == TalkPhase::Error, 1000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::SpeechLost));
    assert!(r
        .helper
        .sent_of("dictation.cancel")
        .iter()
        .any(|m| m["id"] == id.as_str()));
    assert!(
        r.helper.active().is_none(),
        "the helper dictation (and mic) is released"
    );
    assert!(!r.last_entry().unwrap().raw.is_empty());
    // The next dictation gets a fresh module process.
    let v = r.dictate(300).await;
    assert!(v.last.is_some() && v.error.is_none());
}

#[tokio::test(start_paused = true)]
async fn module_crash_with_an_old_cloud_pick_ends_with_what_was_heard() {
    let mut o = with_module("2.1.0");
    o.config = json!({"talk": {"engine": "elevenlabs"}});
    let r = rig(o);
    start_and_hear(&r).await;
    r.helper.module_crash();
    until(|| r.view().phase == TalkPhase::Error, 2000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::SpeechLost));
    let e = r.last_entry().expect("what was heard is kept");
    assert_eq!(e.engine, Engine::OnDevice);
    assert!(!e.raw.is_empty());
}

#[tokio::test(start_paused = true)]
async fn module_updated_mid_session_keeps_the_session_pinned() {
    let r = rig(with_module("2.1.0"));
    let id = start_and_hear(&r).await;
    r.helper.module_update("2.2.0");
    sleep(200).await;
    let sp = r.speech.clone().unwrap();
    assert_eq!(
        sp.version_of(&id).as_deref(),
        Some("2.1.0"),
        "pinned to the version it started on"
    );
    assert_eq!(r.view().phase, TalkPhase::Listening);
    r.svc.stop().await;
    until(|| !r.view().active(), 2000).await;
    assert!(r.view().error.is_none() && r.view().last.is_some());
    let v = r.dictate(300).await;
    assert!(v.last.is_some() && v.error.is_none());
    assert_eq!(
        sp.started().iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>(),
        ["2.1.0", "2.2.0"]
    );
}

#[tokio::test(start_paused = true)]
async fn an_update_that_ends_the_session_ends_the_dictation_cleanly() {
    // An engine that cannot keep the old version (e.g. its files were removed) fails the session: talk keeps what
    // was heard and says so, never "listening" forever.
    let r = rig(with_module("2.1.0"));
    start_and_hear(&r).await;
    r.helper.module_update("2.2.0");
    r.helper.module_crash();
    until(|| r.view().phase == TalkPhase::Error, 1000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::SpeechLost));
    assert!(!r.last_entry().unwrap().raw.is_empty());
}

#[test]
fn no_model_names_in_the_crate() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let banned = ["fluidaudio", "nemotron", "parakeet", "whisper"];
    let mut hits = vec![];
    for sub in ["src", "tests", "tests/common"] {
        for f in std::fs::read_dir(dir.join(sub)).unwrap().flatten() {
            let p = f.path();
            if p.is_dir() || p.ends_with("speech_module.rs") {
                continue;
            }
            let t = std::fs::read_to_string(&p).unwrap().to_lowercase();
            for b in banned {
                if t.contains(b) {
                    hits.push(format!("{}: {b}", p.display()));
                }
            }
        }
    }
    assert!(hits.is_empty(), "{hits:?}");
}

// ---------------------------------------------------------------------------------------- revocation (ADR 0046 §4b)

fn revoked_on_an_expired_list(o: RigOpts) -> Rig {
    let r = rig(o);
    let sp = r.speech.clone().unwrap();
    sp.install(Some((SPEECH_PROTOCOL_VERSION, "2.2.0"))); // 2.1.0 is the previous good version
    sp.revoke_current(true);
    r
}

#[tokio::test(start_paused = true)]
async fn revoked_on_an_expired_list_never_rolls_back_and_says_unavailable() {
    let r = revoked_on_an_expired_list(with_module("2.1.0"));
    let sp = r.speech.clone().unwrap();
    assert_eq!(
        sp.current().as_deref(),
        Some("2.2.0"),
        "no rollback to 2.1.0 while the list is expired"
    );
    let v = r.svc.start().await;
    assert_eq!(v.phase, TalkPhase::Error);
    assert_eq!(v.error_kind, Some(TalkErrorKind::SpeechUnavailable));
    let msg = v.error.unwrap();
    assert!(
        !msg.contains("2.2.0") && !msg.contains("2.1.0"),
        "no version in the client text: {msg}"
    );
    assert!(
        r.helper.sent_of("dictation.start").is_empty(),
        "the mic never opened"
    );
    assert!(sp.started().is_empty(), "no session on a revoked version");
    // A fresh list would roll back (the store's rule), and dictation works again on 2.1.0.
    sp.revoke_current(false);
    assert_eq!(sp.current().as_deref(), Some("2.1.0"));
    let v = r.dictate(300).await;
    assert!(v.last.is_some() && v.error.is_none());
}

#[tokio::test(start_paused = true)]
async fn revoked_on_an_expired_list_is_unavailable_with_no_cloud_fallback() {
    // ADR 0076: even with an old config that picked ElevenLabs, a revoked module means talk is unavailable.
    let mut o = with_module("2.1.0");
    o.config = json!({"talk": {"engine": "elevenlabs"}});
    let r = revoked_on_an_expired_list(o);
    let v = r.svc.start().await;
    assert_eq!(v.phase, TalkPhase::Error);
    assert_eq!(v.error_kind, Some(TalkErrorKind::SpeechUnavailable));
    let msg = v.error.unwrap();
    assert!(msg.contains("unavailable") && !msg.contains("cloud"), "{msg}");
    assert!(r.helper.sent_of("dictation.start").is_empty());
    assert!(r.speech.as_ref().unwrap().started().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_running_session_on_a_revoked_version_ends_at_its_next_safe_point() {
    let r = rig(with_module("2.1.0"));
    let id = start_and_hear(&r).await;
    let sp = r.speech.clone().unwrap();
    sp.revoke_current(true);
    // The utterance in flight goes on (partials keep coming) ...
    sleep(150).await;
    assert_eq!(r.view().phase, TalkPhase::Listening);
    assert!(sp.live(&id));
    // ... until it is final: the safe point. The session ends there, the dictation with it, keeping what was heard.
    r.helper.module_line(json!({"type": "words", "utteranceId": "u1", "channel": "mic", "final": true,
        "language": "en", "words": [{"text": "hello", "startMs": 0, "endMs": 40}, {"text": "there", "startMs": 50, "endMs": 90}]}));
    until(|| r.view().phase == TalkPhase::Error, 1000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::SpeechUnavailable));
    assert!(!sp.live(&id));
    assert!(r
        .helper
        .sent_of("dictation.cancel")
        .iter()
        .any(|m| m["id"] == id.as_str()));
    assert!(r.last_entry().unwrap().raw.contains("hello there"));
    // The next dictation is refused before the mic (no rollback on an expired list).
    let n = r.helper.sent_of("dictation.start").len();
    let v = r.svc.start().await;
    assert_eq!(v.error_kind, Some(TalkErrorKind::SpeechUnavailable));
    assert_eq!(r.helper.sent_of("dictation.start").len(), n);
}

#[test]
fn failure_reasons_tell_not_installed_and_revoked_from_a_crash() {
    use qrtz_talk::qrtz_speech::FailReason as F;
    for r in [F::NotInstalled, F::Revoked, F::Stale] {
        assert_eq!(speech::fail_error(r).0, TalkErrorKind::SpeechUnavailable, "{r:?}");
    }
    for r in [F::Crashed, F::ProtocolViolations, F::StartFailed, F::Timeout] {
        assert_eq!(speech::fail_error(r).0, TalkErrorKind::SpeechLost, "{r:?}");
    }
}
