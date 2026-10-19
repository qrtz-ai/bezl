//! Hardening cycle 34, N166 (ADR 0107): since ADR 0104 the press opens the mic, and the helper's `dictation.start`
//! takes everything heard since the press as its pre-roll (up to `armMaxMs`, 10 s). The helper decodes that head
//! and counts it in its release budget (`samples`), but talk's stop wait counted only key-down-at-start to
//! release. A press held through the last dictation's finalizing (N110) starts seconds after the press, so talk
//! gave up seconds before the helper's final and pasted the last partial: the cut-off tail of N130 again. Now the
//! wait counts the head: `dictation.started`'s `preRollMs`, or the press-to-start time, whichever is larger.
mod common;
use common::*;
use qrtz_talk::*;

fn swift_static(src: &str, name: &str) -> u64 {
    let at = src
        .find(&format!("static let {name} = "))
        .unwrap_or_else(|| panic!("EchoController has no {name}"));
    let rest = &src[at + "static let ".len() + name.len() + " = ".len()..];
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '_')).unwrap();
    rest[..end].replace('_', "").parse().unwrap()
}

/// The host's copy of the arm's cap is the helper's (echo's controller since ADR 0146 M2.2).
#[test]
fn the_host_copy_of_the_arm_cap_matches_echo_controller_swift() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../native/qrtz-helper/Sources/QrtzEcho/EchoController.swift"),
    )
    .unwrap();
    assert_eq!(swift_static(&src, "armMaxMs"), HelperTail::default().arm_max_ms);
}

fn backlogged(pre_roll_ms: u64) -> Rig {
    rig(RigOpts {
        helper: HelperOpts {
            audio_every_ms: 100,
            // The recogniser is behind: each final comes 6.5 s after its release (inside the helper's budget for
            // the audio it holds, past talk's old bound for a 0.5 s hold, 5.54 s).
            stopped_delay_ms: 6500,
            final_tail_words: 3,
            pre_roll_ms,
            ..Default::default()
        },
        timeouts: Timeouts::default(),
        ..Default::default()
    })
}

fn assert_pasted_in_full(r: &Rig, n: usize) {
    let finals = r.helper.finals();
    assert_eq!(finals.len(), n);
    let h = r.svc.history();
    assert_eq!(h.len(), n);
    let e = &h[0];
    assert_eq!(e.raw, finals[n - 1], "the real final, not the last partial");
    assert!(e.error.is_none(), "{:?}", e.error);
    let ins = r.helper.sent_of("insert.text");
    assert_eq!(ins.len(), n, "each pasted once");
    assert_eq!(ins[n - 1]["text"].as_str().unwrap(), format!("{} ", e.text));
}

/// A press during the last dictation's finalizing, then a 0.5 s hold: the helper heard ~7 s (the head since the
/// press plus the hold) and answers 6.5 s after the release. Pasted in full; before N166 talk went ahead at 5.5 s.
#[tokio::test(start_paused = true)]
async fn a_press_queued_behind_a_finishing_dictation_waits_for_its_whole_head() {
    let r = backlogged(0);
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(1000).await;
    r.helper.key("up");
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    let first = r.view().id.expect("first");
    r.helper.key("down");
    until(
        || r.view().phase == TalkPhase::Listening && r.view().id.as_deref() != Some(first.as_str()),
        10_000,
    )
    .await;
    sleep(500).await;
    r.helper.key("up");
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    sleep(6000).await;
    assert_eq!(
        r.view().phase,
        TalkPhase::Finalizing,
        "went ahead before the helper's final"
    );
    until(|| !r.view().active(), 10_000).await;
    assert_pasted_in_full(&r, 2);
}

/// The helper says how much head it fed (`dictation.started{preRollMs}`): a 6 s armed head and a 0.5 s hold get
/// the helper's wait for 6.5 s of audio.
#[tokio::test(start_paused = true)]
async fn the_started_pre_roll_counts_in_the_stop_wait() {
    let r = backlogged(6000);
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(500).await;
    r.helper.key("up");
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    sleep(6000).await;
    assert_eq!(r.view().phase, TalkPhase::Finalizing, "went ahead early");
    until(|| !r.view().active(), 10_000).await;
    assert_pasted_in_full(&r, 1);
}

/// The head is bounded like the helper's ring: a helper reporting more (or garbage) doesn't stretch the wait past
/// the helper's own cap, and a plain dictation with no head waits as before.
#[test]
fn the_head_is_bounded_by_the_arm_cap() {
    let t = Timeouts::default();
    let h = HelperTail::default();
    assert!(t.stopped_for(500.0 + h.arm_max_ms as f64) <= 25_000);
    assert!(t.stopped_for(500.0) < t.stopped_for(500.0 + 6000.0));
}
