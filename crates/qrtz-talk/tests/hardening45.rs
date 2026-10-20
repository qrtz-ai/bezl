#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Hardening cycle 45, ADR 0147 (N172): echo switched off mid-dictation. ADR 0146 made `modules.echo.enabled` and
//! `modules.enabled` turn echo off at runtime (as `talk.enabled` always did). Switching off unsets the push key on
//! the helper, which stops its key tap, so the release of a key already held never arrives; and talk used to drop a
//! release while echo was off anyway. The dictation then kept the mic open until a timeout. Now: a configure with
//! echo off cancels a listening dictation (nothing inserted), a release always ends what a press started, and a
//! push-key start whose engine handshake outlives the switch never opens the mic.
mod common;
use common::*;
use qrtz_talk::*;
use serde_json::json;

fn off() -> serde_json::Value {
    json!({"modules": {"echo": {"enabled": false}}})
}

#[tokio::test(start_paused = true)]
async fn switching_echo_off_mid_dictation_cancels_it() {
    let r = rig(RigOpts::default());
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(400).await;
    r.set_config(off());
    r.svc.configure();
    until(|| r.view().phase == TalkPhase::Idle, 2000).await;
    assert_eq!(
        r.helper.sent_of("dictation.cancel").len(),
        1,
        "{}",
        r.everything_said()
    );
    let conf = r.helper.sent_of("dictation.configure");
    assert_eq!(
        conf.last().unwrap()["pushKey"],
        "",
        "the helper's key tap is unset"
    );
    assert!(r.helper.active().is_none(), "the mic is closed");
    // The release the helper would no longer send, if it came anyway, starts nothing.
    r.helper.key("up");
    sleep(1000).await;
    assert_eq!(r.view().phase, TalkPhase::Idle);
    assert!(r.helper.sent_of("dictation.stop").is_empty());
    assert!(
        r.helper.finals().is_empty(),
        "nothing inserted: {}",
        r.everything_said()
    );
    // Back on: the next press dictates as usual.
    r.set_config(json!({}));
    r.svc.configure();
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(400).await;
    r.helper.key("up");
    until(
        || r.view().phase == TalkPhase::Idle && r.view().last.is_some(),
        10_000,
    )
    .await;
    assert_eq!(r.helper.sent_of("dictation.stop").len(), 1);
}

/// The config changed without a configure (it is read lazily): a release still ends the dictation it started.
#[tokio::test(start_paused = true)]
async fn a_release_ends_the_dictation_even_with_echo_off() {
    let r = rig(RigOpts::default());
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(400).await;
    r.set_config(off());
    r.helper.key("up");
    until(
        || r.view().phase == TalkPhase::Idle && r.view().last.is_some(),
        10_000,
    )
    .await;
    assert_eq!(
        r.helper.sent_of("dictation.stop").len(),
        1,
        "{}",
        r.everything_said()
    );
    // A press with echo off still starts nothing.
    r.helper.key("down");
    sleep(1000).await;
    assert_eq!(r.view().phase, TalkPhase::Idle);
    assert_eq!(r.helper.sent_of("dictation.start").len(), 1);
}

/// Switched off while the engine handshake of a push-key start was still running: the mic never opens.
#[tokio::test(start_paused = true)]
async fn switching_off_during_the_handshake_never_opens_the_mic() {
    let r = rig(RigOpts {
        local_start_delay_ms: 300,
        ..Default::default()
    });
    r.helper.key("down");
    sleep(100).await;
    r.set_config(off());
    r.svc.configure();
    sleep(1000).await;
    assert!(
        r.helper.sent_of("dictation.start").is_empty(),
        "{}",
        r.everything_said()
    );
    assert_eq!(r.view().phase, TalkPhase::Idle);
    assert!(r.helper.active().is_none());
}
