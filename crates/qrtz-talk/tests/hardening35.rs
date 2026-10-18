//! Hardening cycle 35, N165 (ADR 0109): `finish_inner` waited up to `final_ms` (1.5 s) for the speech-module
//! session's flush even when the helper's `dictation.stopped` text was non-empty and wins. With a module that is
//! slow to flush, insert waited on text it wouldn't use. Shipped builds use `HelperAsrSpeech` (finish is instant),
//! so no user effect today; the wait is skipped when the helper already sent the text.
mod common;
use common::*;
use qrtz_talk::qrtz_speech::protocol::SPEECH_PROTOCOL_VERSION;
use qrtz_talk::*;

fn with_slow_flush(finish_delay_ms: u64, final_tail_words: usize) -> Rig {
    rig(RigOpts {
        helper: HelperOpts {
            audio_every_ms: 50,
            module: Some((SPEECH_PROTOCOL_VERSION, "2.1.0".into())),
            final_tail_words,
            ..Default::default()
        },
        timeouts: Timeouts {
            started: 500,
            stopped: 500,
            stopped_per_audio_s: 0,
            stopped_max: 500,
            helper_tail: None,
            insert: 300,
            final_ms: 1500,
        },
        local_finish_delay_ms: finish_delay_ms,
        ..Default::default()
    })
}

/// Helper sent the full text: insert does not wait out the module's slow flush (or `final_ms`).
/// Fails without the fix: `latency.final_ms` hits the 1500 ms flush timeout.
#[tokio::test(start_paused = true)]
async fn a_non_empty_helper_final_skips_the_module_flush_wait() {
    let r = with_slow_flush(2000, 2);
    let v = r.dictate(400).await;
    let e = v.last.expect("history");
    assert!(!e.raw.is_empty(), "helper text was used");
    assert!(
        e.latency.final_ms.unwrap_or(9_999.0) < 500.0,
        "waited on a flush that does not win: final_ms={:?}",
        e.latency.final_ms
    );
    assert!(e.error.is_none(), "{:?}", e.error);
}

/// Helper sent nothing (no audio heard): still wait for the module flush so its last finals can fill in.
/// Fails if the skip is unconditional: stop→idle would be near-instant instead of ~800 ms.
#[tokio::test(start_paused = true)]
async fn an_empty_helper_final_still_waits_for_the_module_flush() {
    let r = with_slow_flush(800, 0);
    let me = r.svc.clone();
    tokio::spawn(async move { me.start().await });
    until(|| r.view().phase == TalkPhase::Listening, 1000).await;
    // No speak: heard=0, stopped text "".
    let t0 = tokio::time::Instant::now();
    r.svc.stop().await;
    until(|| !r.view().active(), 10_000).await;
    let waited = t0.elapsed().as_millis() as u64;
    assert!(
        waited >= 750,
        "should wait for the module flush when helper text is empty; waited {waited} ms"
    );
    assert!(
        waited < 1400,
        "should finish with the module, not burn the whole final_ms; waited {waited} ms"
    );
}
