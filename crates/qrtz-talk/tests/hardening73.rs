#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Echo cycle 73 (ADR 0222, N194, backlog P4 mic edge cases): an `error{dictation.mic}` carries no dictation id.
//! The helper sends it when a mic open fails: a refused `dictation.start`, `startMic` failing at a start, or the
//! arm on a key press (`EchoController.pushKey(.down)` → `configure(warmMic: true)` → `attachMic`) failing because
//! the input device just went away. That last one can come while the previous dictation is finalizing or
//! inserting (a quick re-press, N110), whose mic is already closed. Talk used to fail whatever was active: it
//! cancelled the finalizing dictation (the helper drops its final), released its stop wait with nothing, so the
//! words were lost with no history entry, and the pill said "microphone access denied". Now only a listening
//! dictation (and only one the error names, when it names one) is failed; a finishing one keeps its words, and the
//! press's own start reports the mic when it runs. Fails without the fix (the finalizing and inserting cases lose
//! the dictation or end it in an error; the other-id case fails the dictation).
mod common;
use common::*;
use qrtz_talk::*;
use serde_json::json;

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

async fn hold(r: &Rig, ms: u64) {
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(ms).await;
    r.helper.key("up");
}

/// What the helper sends when a mic open fails (`Dictation.startMic`'s catch): no id.
fn mic_open_failed() -> serde_json::Value {
    json!({"type": "error", "code": "dictation.mic", "message": "no microphone input device", "fatal": false})
}

fn cancels(r: &Rig, id: &str) -> usize {
    r.helper
        .sent_of("dictation.cancel")
        .iter()
        .filter(|m| m["id"] == id)
        .count()
}

/// The device is pulled right at the release and the user presses again while the recogniser finalizes: the arm's
/// mic open fails. The finalizing dictation still ends with the helper's full final, inserted, never cancelled.
#[tokio::test(start_paused = true)]
async fn a_failed_mic_open_for_a_press_while_finalizing_keeps_the_last_dictation() {
    let r = rig(opts(HelperOpts {
        stopped_delay_ms: 1_500,
        final_tail_words: 2,
        ..Default::default()
    }));
    hold(&r, 800).await;
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    let id = r.view().id.expect("an id");
    r.helper.key("down");
    r.helper.deliver(mic_open_failed());
    sleep(50).await;
    assert_eq!(
        r.view().phase,
        TalkPhase::Finalizing,
        "still waiting for its final"
    );
    r.helper.key("up");
    until(|| !r.view().active(), 8_000).await;
    assert_eq!(cancels(&r, &id), 0, "the finalizing dictation is never cancelled");
    let e = r.last_entry().expect("the dictation is kept");
    assert_eq!(
        e.raw,
        r.helper.finals()[0],
        "the helper's full final, tail words included"
    );
    assert!(e.inserted, "and it went in");
    assert!(e.error.is_none(), "{:?}", e.error);
    assert!(!r.phases().contains(&"error".to_owned()), "{:?}", r.phases());
    assert_eq!(r.view().error_kind, None);
    // The released press started nothing (N110), and nothing is left running in the helper.
    assert_eq!(r.helper.sent_of("dictation.start").len(), 1);
    assert_eq!(r.helper.active(), None);
}

/// The same failure while the last dictation's text is being pasted (a slow app): the paste finishes, no error.
#[tokio::test(start_paused = true)]
async fn a_failed_mic_open_while_inserting_leaves_the_paste_alone() {
    let r = rig(opts(HelperOpts {
        insert_delay_ms: 1_000,
        ..Default::default()
    }));
    hold(&r, 800).await;
    until(|| r.view().phase == TalkPhase::Inserting, 4000).await;
    r.helper.deliver(mic_open_failed());
    until(|| !r.view().active(), 8_000).await;
    let e = r.last_entry().expect("the dictation is kept");
    assert!(e.inserted);
    assert!(e.error.is_none(), "{:?}", e.error);
    assert!(!r.phases().contains(&"error".to_owned()), "{:?}", r.phases());
    assert_eq!(r.view().error_kind, None);
}

/// A listening dictation whose mic can't open still fails at once, typed, and is cancelled in the helper (as before).
/// One the error doesn't name is left alone.
#[tokio::test(start_paused = true)]
async fn a_listening_dictation_fails_on_its_own_mic_error_only() {
    let r = rig(opts(HelperOpts::default()));
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    let id = r.view().id.expect("an id");
    let mut other = mic_open_failed();
    other["id"] = json!("someone-else");
    r.helper.deliver(other);
    sleep(300).await;
    assert_eq!(r.view().phase, TalkPhase::Listening, "another start's error");
    r.helper.deliver(mic_open_failed());
    until(|| r.view().phase == TalkPhase::Error, 1000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::MicDenied));
    assert_eq!(r.view().error.as_deref(), Some("no microphone input device"));
    assert_eq!(cancels(&r, &id), 1);
    r.helper.key("up");
    sleep(500).await;
    assert_eq!(r.view().phase, TalkPhase::Error, "the release changes nothing");
}
