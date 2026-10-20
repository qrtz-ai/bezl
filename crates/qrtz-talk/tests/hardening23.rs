#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! Cycle 23: the push key while the previous dictation is still finishing (N110), and a tap shorter than the
//! on-device engine's start (N111). Since `886e436` the pill hides the moment the key is released, so nothing on
//! screen says a dictation is still finishing: a press then must not be lost, and a key that is up must never leave
//! the mic listening.
mod common;
use common::*;
use qrtz_talk::*;

fn opts(h: HelperOpts) -> RigOpts {
    RigOpts {
        helper: HelperOpts {
            audio_every_ms: 50,
            ..h
        },
        ..Default::default()
    }
}

async fn hold(r: &Rig, ms: u64) {
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(ms).await;
    r.helper.key("up");
}

fn inserted_ids(r: &Rig) -> Vec<String> {
    r.helper
        .sent_of("insert.text")
        .iter()
        .map(|m| m["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// N110: a press while the last dictation's text is being inserted starts the next one at once; both are inserted,
/// in order, and the first one finishing never takes the view away from the second.
#[tokio::test(start_paused = true)]
async fn a_press_while_the_last_text_is_inserted_starts_the_next_dictation() {
    let r = rig(opts(HelperOpts {
        insert_delay_ms: 250,
        ..Default::default()
    }));
    hold(&r, 300).await;
    until(|| r.view().phase == TalkPhase::Inserting, 2000).await;
    let first = r.view().id.expect("first id");
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    let second = r.view().id.expect("second id");
    assert_ne!(first, second);
    // The first insert lands while the second is listening: the view stays the second's, with the first as `last`.
    until(|| r.svc.history().len() == 1, 2000).await;
    let v = r.view();
    assert_eq!(v.phase, TalkPhase::Listening);
    assert_eq!(v.id.as_deref(), Some(second.as_str()));
    assert_eq!(v.last.as_ref().map(|e| e.id.as_str()), Some(first.as_str()));
    sleep(200).await;
    r.helper.key("up");
    until(|| !r.view().active(), 5000).await;
    let h = r.svc.history();
    assert_eq!(h.len(), 2, "both dictations kept");
    assert_eq!(h[0].id, second);
    assert_eq!(h[1].id, first);
    assert!(h.iter().all(|e| e.inserted && !e.text.is_empty()));
    assert_eq!(inserted_ids(&r), [first, second]);
    assert!(r.helper.active().is_none(), "the mic is closed");
}

/// N110: a press while the last dictation is still finalizing (the helper hasn't handed back its text) isn't
/// dropped: the next dictation starts as soon as the last one is past finalizing, while the key is still held.
#[tokio::test(start_paused = true)]
async fn a_press_while_finalizing_starts_once_the_last_one_is_past_it() {
    let r = rig(opts(HelperOpts {
        stopped_delay_ms: 200,
        ..Default::default()
    }));
    hold(&r, 300).await;
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    let first = r.view().id.expect("first id");
    r.helper.key("down");
    sleep(50).await;
    assert_eq!(r.view().phase, TalkPhase::Finalizing, "still the first one");
    until(
        || r.view().phase == TalkPhase::Listening && r.view().id.as_deref() != Some(first.as_str()),
        2000,
    )
    .await;
    sleep(300).await;
    r.helper.key("up");
    until(|| !r.view().active(), 5000).await;
    let h = r.svc.history();
    assert_eq!(h.len(), 2, "the second press was not lost");
    assert_eq!(h[1].id, first);
    assert!(h.iter().all(|e| e.inserted));
    assert!(r.helper.active().is_none());
}

/// N110: released again before the last dictation got past finalizing: nothing is started, nothing sticks, and the
/// key works normally afterwards.
#[tokio::test(start_paused = true)]
async fn a_press_released_while_finalizing_starts_nothing() {
    let r = rig(opts(HelperOpts {
        stopped_delay_ms: 300,
        ..Default::default()
    }));
    hold(&r, 300).await;
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    r.helper.key("down");
    sleep(50).await;
    r.helper.key("up");
    until(|| !r.view().active(), 5000).await;
    sleep(500).await;
    assert!(!r.view().active());
    assert_eq!(r.helper.sent_of("dictation.start").len(), 1);
    assert!(r.helper.active().is_none());
    hold(&r, 300).await;
    until(|| !r.view().active(), 5000).await;
    assert_eq!(r.svc.history().len(), 2);
}

/// N111: a tap shorter than the on-device engine's start (the module spawn + handshake). Before the fix, the
/// release found nothing listening yet, the start then opened the mic, and with the key already up the dictation
/// stayed listening with a hot mic while every later press was ignored.
#[tokio::test(start_paused = true)]
async fn a_tap_shorter_than_the_engine_start_never_leaves_the_mic_listening() {
    let r = rig(RigOpts {
        local_start_delay_ms: 200,
        ..opts(HelperOpts::default())
    });
    r.helper.key("down");
    sleep(20).await;
    r.helper.key("up");
    sleep(1500).await;
    assert!(
        !r.view().active(),
        "stuck in {:?} with the key up",
        r.view().phase
    );
    assert!(r.helper.active().is_none(), "the mic is closed");
    // The key still works.
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(400).await;
    r.helper.key("up");
    until(|| !r.view().active(), 5000).await;
    assert!(r.helper.active().is_none());
    assert!(r.svc.history().iter().any(|e| e.inserted));
}

/// N111 with a slow start and a normal hold: the hold is unaffected (the release stops the dictation it started).
#[tokio::test(start_paused = true)]
async fn a_normal_hold_with_a_slow_engine_start_is_unchanged() {
    let r = rig(RigOpts {
        local_start_delay_ms: 200,
        ..opts(HelperOpts::default())
    });
    hold(&r, 400).await;
    until(|| !r.view().active(), 5000).await;
    let h = r.svc.history();
    assert_eq!(h.len(), 1);
    assert!(h[0].inserted);
    assert_eq!(r.phases(), ["listening", "finalizing", "inserting", "idle"]);
}

/// The helper dies while an older dictation's insert is outstanding and a newer one is listening: the old
/// insert's wait is released at once (its words go to the clipboard), and the newer dictation ends as helper-lost.
#[tokio::test(start_paused = true)]
async fn helper_death_during_an_overlap_releases_the_old_insert() {
    let r = rig(opts(HelperOpts {
        insert_delay_ms: 5_000,
        ..Default::default()
    }));
    hold(&r, 300).await;
    until(|| r.view().phase == TalkPhase::Inserting, 2000).await;
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(100).await;
    r.helper.die();
    until(|| r.svc.history().len() == 2, 2000).await;
    let h = r.svc.history();
    assert!(h.iter().all(|e| !e.inserted));
    assert!(
        !r.clip.0.lock().unwrap().is_empty(),
        "the words are on the clipboard"
    );
    assert!(!r.view().active());
}
