//! Echo cycle 75, backlog P8 (crash safety, talk side; ADR 0225). The capture helper (the echo process) dies and
//! the app restarts it. A new helper process starts with its push key "up" (`HotkeyFilter.down == false`), so the
//! release of a press the old process heard never arrives. These pin that no press of a dead helper can leave the
//! pill listening, and that a helper death before a start is answered says what happened.
mod common;
use common::*;
use qrtz_talk::*;

fn opts(h: HelperOpts, local_start_delay_ms: u64) -> RigOpts {
    RigOpts {
        helper: HelperOpts {
            audio_every_ms: 100,
            ..h
        },
        timeouts: Timeouts::default(),
        local_start_delay_ms,
        ..Default::default()
    }
}

fn starts(r: &Rig) -> usize {
    r.helper.sent_of("dictation.start").len()
}

/// N200: fn goes down under helper 1, the engine handshake is still running, helper 1 dies and helper 2 says
/// hello before the handshake ends. The release happens while no helper that saw the press is alive, so no `up`
/// ever comes. Before: talk started the dictation on helper 2 anyway and the pill listened until the next press.
/// After: the press is dropped (nothing started, nothing inserted), and the next press dictates normally.
#[tokio::test(start_paused = true)]
async fn a_press_whose_helper_died_during_the_engine_start_never_listens() {
    let r = rig(opts(HelperOpts::default(), 400));
    r.helper.key("down");
    sleep(100).await;
    r.helper.die();
    sleep(50).await;
    r.helper.restart();
    // The handshake ends at ~400 ms; give it plenty of room.
    sleep(1_000).await;
    assert_ne!(r.view().phase, TalkPhase::Listening, "no pill left listening");
    assert!(!r.view().active());
    assert_eq!(
        starts(&r),
        0,
        "the dead helper's press never starts a dictation on the new one"
    );
    assert!(r.helper.active().is_none());
    // A fresh press under the new helper works end to end.
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2_000).await;
    sleep(400).await;
    r.helper.key("up");
    until(|| !r.view().active(), 10_000).await;
    assert!(r.view().last.is_some(), "the next press dictates");
    assert_eq!(starts(&r), 1);
}

/// Same race, but helper 2 isn't up yet when the handshake ends: the start can't be sent and talk says the helper
/// isn't running (unchanged, pinned here); nothing is left listening, and a press after the restart dictates.
#[tokio::test(start_paused = true)]
async fn a_press_while_the_helper_is_down_fails_at_once_and_the_next_one_works() {
    let r = rig(opts(HelperOpts::default(), 200));
    r.helper.key("down");
    sleep(50).await;
    r.helper.die();
    until(|| !r.view().active() && r.view().phase == TalkPhase::Error, 2_000).await;
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::HelperUnavailable));
    r.helper.key("up");
    r.helper.restart();
    sleep(20).await;
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2_000).await;
    sleep(400).await;
    r.helper.key("up");
    until(|| !r.view().active(), 10_000).await;
    assert!(r.view().last.is_some());
}

/// N201: the helper dies after `dictation.start` was sent and before it answered `dictation.started`. Talk
/// releases the start wait and ends the dictation as helper-lost; it must never call that "Microphone did not
/// start" (the mic was fine; the process went away), whichever task takes the lock first.
#[tokio::test(start_paused = true)]
async fn a_helper_death_before_the_start_is_answered_says_helper_lost() {
    let r = rig(opts(
        HelperOpts {
            mute: true,
            ..Default::default()
        },
        0,
    ));
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2_000).await;
    sleep(50).await;
    r.helper.die();
    until(|| !r.view().active(), 2_000).await;
    let v = r.view();
    assert_eq!(v.phase, TalkPhase::Error);
    assert_eq!(v.error_kind, Some(TalkErrorKind::HelperLost), "{:?}", v.error);
    // And a new helper takes the next press.
    r.helper.set_opts(|o| o.mute = false);
    r.helper.restart();
    sleep(20).await;
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2_000).await;
    sleep(400).await;
    r.helper.key("up");
    until(|| !r.view().active(), 10_000).await;
    assert!(r.view().last.is_some());
}

/// N200 variant: a restart is a fresh `hello` with no supervisor "down" first (a quick respawn). The press was
/// heard by the old process, so it is dropped the same way.
#[tokio::test(start_paused = true)]
async fn a_press_from_before_a_hello_never_starts_on_the_new_helper() {
    let r = rig(opts(HelperOpts::default(), 400));
    r.helper.restart(); // helper 1's hello
    sleep(20).await;
    r.helper.key("down");
    sleep(100).await;
    r.helper.restart(); // helper 2's hello: helper 1 is gone
    sleep(1_000).await;
    assert!(!r.view().active(), "no pill left listening");
    assert_eq!(starts(&r), 0);
}

/// Not dropped: a press under the current helper whose handshake is slow still starts and stops as usual (the
/// guard only looks at which helper heard the press).
#[tokio::test(start_paused = true)]
async fn a_slow_handshake_under_the_same_helper_still_dictates() {
    let r = rig(opts(HelperOpts::default(), 400));
    r.helper.restart();
    sleep(20).await;
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2_000).await;
    sleep(400).await;
    r.helper.key("up");
    until(|| !r.view().active(), 10_000).await;
    assert!(r.view().last.is_some());
    assert_eq!(starts(&r), 1);
}

/// Not dropped: a press made before any hello (the helper's first lines can race the app's start) is helper 1's,
/// and helper 1's hello arriving during the handshake is not a restart.
#[tokio::test(start_paused = true)]
async fn the_first_hello_during_a_handshake_is_not_a_restart() {
    let r = rig(opts(HelperOpts::default(), 400));
    r.helper.key("down");
    sleep(100).await;
    r.helper.deliver(serde_json::json!({"type": "hello", "protocol": 1, "helperVersion": "test", "os": "test", "features": []}));
    until(|| r.view().phase == TalkPhase::Listening, 2_000).await;
    sleep(400).await;
    r.helper.key("up");
    until(|| !r.view().active(), 10_000).await;
    assert!(r.view().last.is_some());
}
