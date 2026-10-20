#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! ADR 0118: echo dictation runs on a segmented batch recogniser. The helper cuts the hold at pauses (or at the
//! 15 s cap), decodes finished segments during the hold and only the last segment after the release, and says so
//! on `dictation.stopped` (`asr`, `segments`, `tailDecodeMs`). Once a dictation has said so, talk's stop wait
//! follows the last segment plus at most one segment still decoding, not the whole dictation's length (the
//! streaming bound, `audio/10`, kept for the first dictation after a helper start and for streaming recognisers).
mod common;
use common::*;
use qrtz_talk::*;
use serde_json::json;

/// For every length, talk waits at least the helper's own segmented budget plus the pad (the helper owns the
/// deadline, N130), and never less than the plain stop floor.
#[test]
fn talk_never_gives_up_before_the_segmented_helper() {
    let t = Timeouts::default();
    let h = HelperTail::default();
    for a in (0..=600_000u64).step_by(250) {
        let wait = t.stopped_for_segmented(a as f64);
        let need = h.segmented_budget_ms(a) + h.pad_ms;
        assert!(wait >= need, "audio {a}: talk {wait} < helper {need}");
        assert!(wait >= t.stopped);
    }
}

/// The wait stops growing past two segments (the last one, and one still decoding in the background): the same
/// after 30 s, 60 s or 5 min of dictation, and far less than the streaming bound for a long one.
#[test]
fn the_wait_follows_the_last_segment_not_the_dictation() {
    let t = Timeouts::default();
    let h = HelperTail::default();
    let span = (h.segment_cap_ms - h.pad_ms) as f64;
    let w = |a: f64| t.stopped_for_segmented(a);
    assert_eq!(w(2.0 * span), w(60_000.0));
    assert_eq!(w(60_000.0), w(300_000.0));
    // ADR 0118's numbers: 6.9 s for 60 s (streaming 23.7 s), 4.6 s for 5 s (streaming 19.5 s).
    assert_eq!((w(60_000.0), t.stopped_for(60_000.0)), (6926, 23_690));
    assert_eq!((w(5_000.0), t.stopped_for(5_000.0)), (4554, 19_490));
    assert!(t.stopped_for(60_000.0) > 20_000, "the streaming bound for 60 s");
    assert!(
        w(5_000.0) <= t.stopped_for(5_000.0),
        "never longer than streaming"
    );
    // Monotonic: a longer dictation never waits less.
    let mut last = 0;
    for a in (0..=60_000).step_by(100) {
        assert!(w(a as f64) >= last);
        last = w(a as f64);
    }
}

/// The cap is kept, and no overlap is budgeted (the chosen rule's overlap is 0, tuning §11).
#[test]
fn the_cap_is_kept() {
    let t = Timeouts::default();
    let h = HelperTail::default();
    let big = HelperTail {
        segment_decode_ms_per_audio_s: 10_000,
        ..h
    };
    let tb = Timeouts {
        helper_tail: Some(big),
        ..t
    };
    let w = tb.stopped_for_segmented(600_000.0);
    assert!(
        w <= big.tail_ms + big.max_wait_ms + big.pad_ms + big.slack_ms,
        "{w}"
    );
    assert_eq!(h.segment_overlap_ms, SEGMENT_OVERLAP_MS);
    assert_eq!(SEGMENT_OVERLAP_MS, 0);
    assert!(!format!("{:?}", t.stopped_for_segmented(f64::NAN)).is_empty());
}

/// The recogniser fields of `dictation.stopped` parse; an empty or missing `asr` means none (an older helper).
#[test]
fn the_stopped_fields_parse() {
    let a = AsrInfo::from_stopped(
        &json!({"type": "dictation.stopped", "id": "d3", "text": "x", "durationMs": 4180,
        "asr": "batch-v2", "segments": 3, "tailDecodeMs": 86.4}),
    )
    .unwrap();
    assert_eq!((a.segments, a.tail_decode_ms), (Some(3), Some(86.4)));
    assert!(a.segmented());
    let s = AsrInfo::from_stopped(&json!({"asr": "streaming", "text": ""})).unwrap();
    assert!(!s.segmented());
    assert!(AsrInfo::from_stopped(&json!({"asr": "", "text": ""})).is_none());
    assert!(AsrInfo::from_stopped(&json!({"text": "hi"})).is_none());
    let long = AsrInfo::from_stopped(&json!({"asr": "x".repeat(500)})).unwrap();
    assert_eq!(long.asr.len(), 48);
}

fn segmented() -> serde_json::Value {
    json!({"asr": "batch-v2", "segments": 4, "tailDecodeMs": 90.0})
}

fn helper(extra: Option<serde_json::Value>) -> Rig {
    rig(RigOpts {
        helper: HelperOpts {
            audio_every_ms: 100,
            final_tail_words: 3,
            stopped_extra: extra,
            ..Default::default()
        },
        timeouts: Timeouts::default(),
        ..Default::default()
    })
}

async fn dictate(r: &Rig, ms: u64) {
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(ms).await;
    r.helper.key("up");
    until(|| r.view().phase != TalkPhase::Listening, 2000).await;
}

async fn done(r: &Rig) {
    until(|| !r.view().active(), 30_000).await;
}

/// End to end: after one dictation that said it was segmented, a 60 s dictation whose helper is 9 s late is past
/// talk's wait (talk went ahead, the late final is kept), while a streaming helper still gets the streaming bound
/// and is waited on in full. Nano's recordings never see this (talk only).
#[tokio::test(start_paused = true)]
async fn a_segmented_helper_is_waited_on_for_its_last_segment_only() {
    let seg = helper(Some(segmented()));
    dictate(&seg, 2000).await;
    done(&seg).await;
    seg.helper.set_opts(|o| o.stopped_delay_ms = 9000);
    dictate(&seg, 60_000).await;
    sleep(8500).await;
    assert_eq!(
        seg.helper.sent_of("insert.text").len(),
        2,
        "the segmented wait ran out before the 9 s final"
    );

    let streaming = helper(Some(json!({"asr": "streaming"})));
    dictate(&streaming, 2000).await;
    done(&streaming).await;
    streaming.helper.set_opts(|o| o.stopped_delay_ms = 9000);
    dictate(&streaming, 60_000).await;
    sleep(8500).await;
    assert_eq!(
        streaming.helper.sent_of("insert.text").len(),
        1,
        "the streaming bound still waits"
    );
    done(&streaming).await;
    let h = streaming.svc.history();
    assert_eq!(h[0].raw, streaming.helper.finals()[1], "the real final");
}

/// The first dictation after a helper start (or a restart) gets the streaming bound: nothing has said the new
/// helper is segmented yet.
#[tokio::test(start_paused = true)]
async fn a_restarted_helper_starts_from_the_streaming_bound() {
    let r = helper(Some(segmented()));
    dictate(&r, 2000).await;
    done(&r).await;
    r.helper.restart();
    sleep(500).await;
    r.helper.set_opts(|o| o.stopped_delay_ms = 9000);
    dictate(&r, 60_000).await;
    sleep(8500).await;
    assert_eq!(
        r.helper.sent_of("insert.text").len(),
        1,
        "still the streaming bound"
    );
    done(&r).await;
}
