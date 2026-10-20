#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Cycle 30 (N150, ADR 0096): a press refused before `dictation.start` is its own attempt. Each refusal view carries
//! the id that press drew, so two refused presses in a row are two failures to the shell's pill gate (it keys a
//! flash by the view's id), and a re-emitted view keeps its id. Before: the refusal kept no id, so the second tap's
//! view was identical to the first and the pill said nothing.
mod common;
use common::*;
use qrtz_talk::qrtz_speech::protocol::SPEECH_PROTOCOL_VERSION;
use qrtz_talk::*;
use serde_json::Value;

fn missing_module() -> Rig {
    let r = rig(RigOpts {
        helper: HelperOpts {
            audio_every_ms: 50,
            module: Some((SPEECH_PROTOCOL_VERSION, "2.1.0".into())),
            ..Default::default()
        },
        ..Default::default()
    });
    r.speech.as_ref().unwrap().install(None);
    r
}

fn error_views(r: &Rig) -> Vec<Value> {
    r.events
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|(e, p)| e == "talk:state" && p["phase"] == "error")
        .map(|(_, p)| p.clone())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn two_refused_taps_in_a_row_are_two_attempts() {
    let r = missing_module();
    for n in 1..=3 {
        r.helper.key("down");
        until(|| error_views(&r).len() >= n, 1000).await;
        r.helper.key("up");
        sleep(50).await;
    }
    let views = error_views(&r);
    assert!(views.len() == 3, "{views:?}");
    let ids: Vec<&str> = views
        .iter()
        .map(|v| v["id"].as_str().expect("a refusal names its attempt"))
        .collect();
    for w in ids.windows(2) {
        assert_ne!(
            w[0], w[1],
            "back-to-back refusals are different attempts: {ids:?}"
        );
    }
    for v in &views {
        assert_eq!(v["errorKind"], "speech-unavailable", "{v}");
    }
    assert!(
        r.helper.sent_of("dictation.start").is_empty(),
        "the mic never opened"
    );
    // The view at rest is the last refusal, id and all (a re-emit keeps it).
    assert_eq!(r.view().id.as_deref(), Some(*ids.last().unwrap()));
}

#[tokio::test(start_paused = true)]
async fn after_a_refusal_the_next_dictation_still_works() {
    let r = missing_module();
    let v = r.svc.start().await;
    assert_eq!(v.phase, TalkPhase::Error);
    let refused = v.id.clone().expect("refusal id");
    r.speech
        .as_ref()
        .unwrap()
        .install(Some((SPEECH_PROTOCOL_VERSION, "2.1.0")));
    let v = r.dictate(400).await;
    let e = v.last.expect("a history entry");
    assert_ne!(e.id, refused, "the refused attempt's id is never reused");
    assert!(!e.raw.is_empty());
}
