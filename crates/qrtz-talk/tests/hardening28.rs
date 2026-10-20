#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Hardening cycle 28: dictation under the failures Jared named (helper crash after the release, a refused start
//! racing the key, a meeting sharing the helper, a full disk while the text goes in), as lasting tests on the
//! scripted helper and a paused clock. These pin behaviour that holds today (T1, N110, N134, T4) on the paths the
//! cycle-27 helper changes (`b0eee50`, N131/N134) touch; the mic-switch finding (N138) is helper-only and has no
//! host test yet (see the review §12.27).
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

/// The helper crashes after the release, while the recogniser finalizes (its `dictation.stopped` would come 4 s
/// later): talk ends at once (T1 releases the stop wait; N142: no insert wait on a helper that is gone), and what
/// was heard is kept and goes to the clipboard, saying why.
#[tokio::test(start_paused = true)]
async fn helper_crash_while_finalizing_ends_at_once_and_keeps_the_words() {
    let r = rig(opts(HelperOpts {
        stopped_delay_ms: 4_000,
        ..Default::default()
    }));
    hold(&r, 800).await;
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    let heard = r.view().partial.clone();
    assert!(!heard.is_empty());
    r.helper.die();
    // N142: before, ~2.5 s more in `inserting`, waiting for a paste from a helper that was gone.
    until(|| !r.view().active(), 200).await;
    let e = r.last_entry().expect("the dictation is kept");
    assert_eq!(e.raw, heard, "the words heard before the crash");
    assert!(!e.inserted);
    assert_eq!(
        r.clip.0.lock().unwrap().last(),
        Some(&format!("{} ", e.text)),
        "the words are on the clipboard"
    );
    assert_eq!(
        e.error.as_deref(),
        Some("the capture helper could not take the text, so it was not pasted; copied to clipboard")
    );
    // The restarted helper serves the next press.
    r.helper.restart();
    sleep(20).await;
    hold(&r, 300).await;
    until(|| !r.view().active(), 8_000).await;
    assert_eq!(r.svc.history().len(), 2);
}

/// N134 racing the key: the refusal for a start reaches talk after a quick tap's release. Talk fails it once,
/// typed, never cancels a dictation that never ran (a stop it sent on the release names only that id, which the
/// helper ignores), keeps nothing, and holds nothing in the helper's arbiter.
#[tokio::test(start_paused = true)]
async fn a_refused_start_after_a_quick_tap_leaves_nothing_running() {
    let r = rig(opts(HelperOpts {
        no_recogniser: true,
        ..Default::default()
    }));
    r.helper.key("down");
    r.helper.key("up");
    until(|| r.view().phase == TalkPhase::Error, 1000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::SpeechUnavailable));
    sleep(5_000).await;
    let id = r.helper.sent_of("dictation.start")[0]["id"].clone();
    assert!(
        r.helper.sent_of("dictation.cancel").is_empty(),
        "cancel sent for a refused start"
    );
    assert!(r.helper.sent_of("dictation.stop").iter().all(|m| m["id"] == id));
    assert!(
        !r.helper.arbiter().dictating,
        "the helper's arbiter holds no dictation"
    );
    assert!(r.svc.history().is_empty(), "nothing to keep");
}

/// A late `dictation.asr` refusal naming an older start (a helper restart's leftovers, a reordered line) never fails
/// the dictation that is running now; one naming it does.
#[tokio::test(start_paused = true)]
async fn a_refusal_for_another_start_never_fails_the_running_dictation() {
    let r = rig(opts(HelperOpts::default()));
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    let id = r.view().id.unwrap();
    r.helper.deliver(
        json!({"type": "error", "code": "dictation.asr", "message": "no speech recogniser is ready",
        "fatal": false, "id": "t-older"}),
    );
    sleep(300).await;
    assert_eq!(
        r.view().phase,
        TalkPhase::Listening,
        "an older start's refusal failed this one"
    );
    r.helper.deliver(
        json!({"type": "error", "code": "dictation.asr", "message": "no speech recogniser is ready",
        "fatal": false, "id": id}),
    );
    until(|| r.view().phase == TalkPhase::Error, 500).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::SpeechUnavailable));
}

/// A meeting records on the same helper; a dictation runs inside it and the meeting stops mid-sentence. The
/// dictation keeps its mic and finishes in full; the recording is gone from the helper's arbiter.
#[tokio::test(start_paused = true)]
async fn the_meeting_stopping_mid_sentence_never_cuts_the_dictation() {
    let r = rig(opts(HelperOpts {
        final_tail_words: 2,
        ..Default::default()
    }));
    assert!(matches!(
        r.helper.recording_start("/tmp/qrtz-c28-meeting.wav").decision,
        qrtz_lifecycle::Decision::Accept
    ));
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(400).await;
    assert!(matches!(
        r.helper.recording_stop().decision,
        qrtz_lifecycle::Decision::Accept
    ));
    assert!(r.helper.arbiter().dictating, "the dictation still holds the mic");
    sleep(400).await;
    r.helper.key("up");
    until(|| !r.view().active(), 8_000).await;
    let finals = r.helper.finals();
    let e = r.last_entry().unwrap();
    assert_eq!(e.raw, finals[0], "the full final, tail included");
    assert!(e.error.is_none(), "{:?}", e.error);
}

/// The disk fills while a long dictation is inserted (history can't be saved): the text still goes in, the error
/// is said, and once space is back the next dictation saves both.
#[tokio::test(start_paused = true)]
async fn a_full_disk_never_costs_the_dictated_text() {
    let r = rig(opts(HelperOpts::default()));
    r.io.disk_full.store(true, std::sync::atomic::Ordering::SeqCst);
    hold(&r, 3_000).await;
    until(|| !r.view().active(), 8_000).await;
    let ins = r.helper.sent_of("insert.text");
    assert_eq!(ins.len(), 1, "pasted despite the full disk");
    assert!(!ins[0]["text"].as_str().unwrap().is_empty());
    r.io.disk_full.store(false, std::sync::atomic::Ordering::SeqCst);
    hold(&r, 300).await;
    until(|| !r.view().active(), 8_000).await;
    let saved = std::fs::read_to_string(&r.store).unwrap_or_default();
    let n = r.svc.history().len();
    assert_eq!(n, 2);
    assert_eq!(
        saved.matches("\"raw\"").count(),
        2,
        "both dictations saved once the disk had room"
    );
}
