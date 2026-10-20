#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! N182 (ADR 0177): a fresh qrtz has echo on before its speech model is downloaded (ADR 0175). A press then gets the
//! helper's `dictation.asr` refusal ("no speech recogniser is ready"); the pill used to say "try again in a moment",
//! which never helps. With the app's probe saying echo's model is missing, it points at the download instead.
mod common;
use common::*;
use qrtz_talk::{TalkErrorKind, TalkPhase, Timeouts, MSG_SPEECH_MISSING};

fn opts(h: HelperOpts) -> RigOpts {
    RigOpts {
        helper: HelperOpts {
            audio_every_ms: 100,
            ..h
        },
        timeouts: Timeouts::default(),
        ..Default::default()
    }
}

async fn refused_press(speech_missing: bool) -> (TalkErrorKind, String) {
    let r = rig(RigOpts {
        speech_missing,
        ..opts(HelperOpts {
            no_recogniser: true,
            ..Default::default()
        })
    });
    r.helper.key("down");
    r.helper.key("up");
    until(|| r.view().phase == TalkPhase::Error, 1000).await;
    let v = r.view();
    assert!(
        r.helper.sent_of("dictation.cancel").is_empty(),
        "a refused start is never cancelled"
    );
    assert!(r.svc.history().is_empty());
    (v.error_kind.unwrap(), v.error.unwrap())
}

#[tokio::test(start_paused = true)]
async fn a_press_before_echos_model_is_downloaded_points_at_the_download() {
    let (kind, msg) = refused_press(true).await;
    assert_eq!(kind, TalkErrorKind::SpeechUnavailable);
    assert_eq!(msg, MSG_SPEECH_MISSING);
    assert_eq!(
        msg,
        qrtz_talk::speech::MSG_NOT_INSTALLED,
        "one text on every route (ADR 0175 decision 5)"
    );
    assert!(msg.contains("download"), "{msg}");
    assert!(!msg.contains("try again in a moment"), "{msg}");
}

#[tokio::test(start_paused = true)]
async fn a_busy_or_loading_recogniser_with_the_model_on_disk_keeps_try_again() {
    let (kind, msg) = refused_press(false).await;
    assert_eq!(kind, TalkErrorKind::SpeechUnavailable);
    assert!(msg.contains("try again in a moment"), "{msg}");
}
