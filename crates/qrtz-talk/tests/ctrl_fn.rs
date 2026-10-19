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

fn nothing_ran(r: &Rig) {
    assert!(r.commands.lock().unwrap().is_empty(), "nothing ran");
    assert!(r.helper.sent_of("insert.text").is_empty(), "nothing typed");
    assert!(r.clip.0.lock().unwrap().is_empty(), "nothing on the clipboard");
    assert!(r.svc.history().is_empty(), "no history");
}

async fn hold_ctrl(r: &Rig) -> String {
    r.helper
        .deliver(json!({"type": "hotkey", "action": "down", "ctrl": true}));
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(1500).await;
    r.helper.active().expect("listening")
}

/// Review H-C3 (a): the helper ends a ctrl+fn hold itself (mic lost) with the words so far: half a command runs
/// nothing, and the mic message still shows.
#[tokio::test(start_paused = true)]
async fn a_ctrl_hold_the_helper_ends_runs_nothing() {
    let r = rig(RigOpts::default());
    let id = hold_ctrl(&r).await;
    r.helper.deliver(json!({"type": "dictation.stopped", "id": id, "text": "delete the",
        "durationMs": 1500, "reason": "mic-lost"}));
    until(|| r.view().phase == TalkPhase::Error, 10_000).await;
    sleep(10_000).await;
    nothing_ran(&r);
    assert_eq!(r.view().error_kind, Some(TalkErrorKind::MicLost));
}

/// Review H-C4 (b): a real final with no text is no command, never the live partials or the on-device engine's.
#[tokio::test(start_paused = true)]
async fn a_ctrl_press_with_an_empty_final_runs_nothing() {
    let r = rig(RigOpts::default());
    r.helper.set_opts(|o| o.stopped_delay_ms = 60_000);
    let id = hold_ctrl(&r).await;
    assert!(!r.view().text.is_empty() || !r.view().partial.is_empty(), "live words were shown");
    r.helper
        .deliver(json!({"type": "hotkey", "action": "up", "ctrl": true}));
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    sleep(50).await;
    r.helper.deliver(
        json!({"type": "dictation.stopped", "id": id, "text": "  ", "durationMs": 1500}),
    );
    until(|| r.view().phase == TalkPhase::Idle, 10_000).await;
    sleep(10_000).await;
    nothing_ran(&r);
}

/// Review R2 (c): the release the helper makes up at sleep (lid closed mid-hold) cuts a ctrl+fn command short.
#[tokio::test(start_paused = true)]
async fn a_sleep_release_during_a_ctrl_hold_runs_nothing() {
    let r = rig(RigOpts::default());
    hold_ctrl(&r).await;
    r.helper.deliver(
        json!({"type": "hotkey", "action": "up", "ctrl": true, "sleep": true}),
    );
    until(|| r.view().phase == TalkPhase::Idle, 10_000).await;
    sleep(10_000).await;
    nothing_ran(&r);
    // The next ordinary ctrl+fn press runs again.
    press(&r, true).await;
    assert_eq!(r.commands.lock().unwrap().len(), 1);
}

/// R2: plain fn keeps N196: a sleep release inserts what was heard.
#[tokio::test(start_paused = true)]
async fn a_sleep_release_of_plain_fn_still_inserts() {
    let r = rig(RigOpts::default());
    r.helper
        .deliver(json!({"type": "hotkey", "action": "down"}));
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(1500).await;
    r.helper
        .deliver(json!({"type": "hotkey", "action": "up", "sleep": true}));
    until(|| r.view().phase == TalkPhase::Idle, 10_000).await;
    sleep(100).await;
    assert_eq!(r.helper.sent_of("insert.text").len(), 1);
}

/// Review M1 (d): a mic or recogniser error while a ctrl command is finalizing runs nothing, even when the final
/// still comes afterwards.
#[tokio::test(start_paused = true)]
async fn a_mic_or_asr_error_while_a_ctrl_command_finalizes_runs_nothing() {
    for code in ["dictation.mic", "dictation.asr"] {
        let r = rig(RigOpts::default());
        r.helper.set_opts(|o| o.stopped_delay_ms = 300);
        let id = hold_ctrl(&r).await;
        r.helper
            .deliver(json!({"type": "hotkey", "action": "up", "ctrl": true}));
        until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
        r.helper.deliver(json!({"type": "error", "code": code, "id": id,
            "message": "x", "fatal": false}));
        sleep(10_000).await;
        assert!(r.commands.lock().unwrap().is_empty(), "{code}: nothing ran");
        assert!(r.helper.sent_of("insert.text").is_empty(), "{code}");
        assert!(r.clip.0.lock().unwrap().is_empty(), "{code}");
        assert!(r.svc.history().is_empty(), "{code}");
    }
}
