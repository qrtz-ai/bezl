//! ADR 0216: ctrl+fn records through echo's dictation pipeline (same press, release, pill and local recogniser),
//! but its transcript goes to ctrl do (`ctrl:do {text}`) and is never inserted. Plain fn still dictates.
mod common;
use common::*;
use qrtz_talk::*;
use serde_json::json;

async fn press(r: &Rig, ctrl: bool) {
    r.helper
        .deliver(json!({"type": "hotkey", "action": "down", "ctrl": ctrl}));
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(1500).await;
    r.helper
        .deliver(json!({"type": "hotkey", "action": "up", "ctrl": ctrl}));
    until(|| r.view().phase == TalkPhase::Idle, 10_000).await;
    sleep(50).await;
}

fn flashes(r: &Rig) -> Vec<String> {
    r.helper
        .sent_of("pill.flash")
        .iter()
        .map(|m| {
            format!(
                "{}:{}",
                m["kind"].as_str().unwrap(),
                m["message"].as_str().unwrap()
            )
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn ctrl_fn_goes_to_ctrl_do_and_is_never_inserted() {
    let r = rig(RigOpts::default());
    press(&r, true).await;
    let cmds = r.commands.lock().unwrap().clone();
    assert_eq!(cmds.len(), 1, "one command");
    assert!(!cmds[0].trim().is_empty());
    assert!(r.helper.sent_of("insert.text").is_empty(), "never inserted");
    assert!(r.clip.0.lock().unwrap().is_empty(), "never on the clipboard");
    assert!(r.svc.history().is_empty(), "not a dictation");
    assert!(
        flashes(&r).iter().any(|f| f.starts_with("info:ctrl: ")),
        "{:?}",
        flashes(&r)
    );
}

#[tokio::test(start_paused = true)]
async fn plain_fn_still_dictates_and_a_ctrl_press_does_not_stick() {
    let r = rig(RigOpts::default());
    press(&r, true).await;
    press(&r, false).await;
    assert_eq!(r.commands.lock().unwrap().len(), 1, "only the ctrl press");
    assert_eq!(
        r.helper.sent_of("insert.text").len(),
        1,
        "the plain press inserted"
    );
    assert_eq!(r.svc.history().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn without_ctrl_the_words_are_still_not_typed() {
    let r = rig(RigOpts {
        no_ctrl: true,
        ..Default::default()
    });
    press(&r, true).await;
    assert!(r.helper.sent_of("insert.text").is_empty());
    assert!(flashes(&r)
        .iter()
        .any(|f| f.starts_with("error:ctrl isn't available")));
}

/// Review H-C: a ctrl+fn press whose final times out runs nothing (it may be half the command), says so, and the
/// final that comes later is not delivered anywhere: no command, no paste, no clipboard, no history.
#[tokio::test(start_paused = true)]
async fn a_ctrl_press_whose_final_times_out_runs_nothing() {
    let r = rig(RigOpts::default());
    r.helper.set_opts(|o| {
        o.stopped_delay_ms = 20_000;
        o.final_tail_words = 3;
    });
    press(&r, true).await;
    sleep(25_000).await;
    assert!(r.commands.lock().unwrap().is_empty(), "nothing ran");
    assert!(r.helper.sent_of("insert.text").is_empty());
    assert!(
        r.clip.0.lock().unwrap().is_empty(),
        "the late final is not on the clipboard"
    );
    assert!(r.svc.history().is_empty());
    assert!(
        flashes(&r)
            .iter()
            .any(|f| f.starts_with("error:didn't catch it all")),
        "{:?}",
        flashes(&r)
    );
}

/// Review H-C2: the helper dies after the key-up, before its `dictation.stopped`. The stop waiter is released with
/// nothing (not a timeout), and that must not fall back to the words shown so far or to the on-device engine:
/// no command, and the pill says it didn't catch it all.
#[tokio::test(start_paused = true)]
async fn a_ctrl_press_whose_helper_dies_before_the_final_runs_nothing() {
    let r = rig(RigOpts::default());
    r.helper.set_opts(|o| o.stopped_delay_ms = 20_000);
    r.helper
        .deliver(json!({"type": "hotkey", "action": "down", "ctrl": true}));
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(1500).await;
    r.helper
        .deliver(json!({"type": "hotkey", "action": "up", "ctrl": true}));
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    sleep(100).await;
    r.helper.die();
    until(
        || r.view().phase == TalkPhase::Idle || r.view().phase == TalkPhase::Error,
        10_000,
    )
    .await;
    sleep(25_000).await;
    assert!(r.commands.lock().unwrap().is_empty(), "nothing ran");
    assert!(r.helper.sent_of("insert.text").is_empty());
    assert!(r.clip.0.lock().unwrap().is_empty());
    assert!(r.svc.history().is_empty());
    // The dead helper can't draw a flash, so the view says it.
    let v = r.view();
    assert_eq!(v.phase, TalkPhase::Error);
    assert_eq!(v.error.as_deref(), Some(qrtz_talk::service::MSG_CTRL_CUT_SHORT));
    assert!(v.error.unwrap().starts_with("didn't catch it all"));
}
