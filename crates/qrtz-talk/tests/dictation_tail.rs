#![allow(clippy::unwrap_used, clippy::expect_used)] // QRT-288: test code may panic on setup failure.
//! N124 (ADR 0081): Jared, on a real Mac: releasing fn often cut off the last words. The helper's real final (its
//! release tail, drained buffers and the recogniser's end-of-stream flush) carries words no partial ever showed, so
//! talk must wait for that final, long enough for on-device ASR and scaled to the dictation's length, and never
//! fall back to the last partial before that bound. A final that still comes after the bound is kept, not dropped.
mod common;
use common::*;
use qrtz_talk::*;

/// The helper's final arrives 1.2 s after release with 3 words past the last partial.
fn slow_final(t: Timeouts) -> RigOpts {
    RigOpts {
        helper: HelperOpts {
            audio_every_ms: 100,
            stopped_delay_ms: 1200,
            final_tail_words: 3,
            ..Default::default()
        },
        timeouts: t,
        ..Default::default()
    }
}

async fn hold(r: &Rig, ms: u64) {
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(ms).await;
    r.helper.key("up");
}

fn inserted(r: &Rig) -> Vec<(String, String)> {
    r.helper
        .sent_of("insert.text")
        .iter()
        .map(|m| {
            (m["id"].as_str().unwrap_or_default().to_owned(), {
                // The pasted text carries echo's one trailing space; compare the words.
                let t = m["text"].as_str().unwrap_or_default();
                assert!(t.ends_with(' '), "pasted text ends in a space: {t:?}");
                t.strip_suffix(' ').unwrap_or(t).to_owned()
            })
        })
        .collect()
}

fn words(s: &str) -> usize {
    s.split_whitespace().count()
}

#[test]
fn the_stop_wait_covers_the_tail_and_flush_and_scales_with_the_dictation() {
    // The scaled part on its own (N130 adds the helper's worst case on top: tests/hardening27.rs).
    let t = Timeouts {
        helper_tail: None,
        ..Timeouts::default()
    };
    // Tail (~50 ms, then up to one tap block) + at least 1.5 s of flush, for any dictation.
    assert!(t.stopped >= 1750, "{}", t.stopped);
    assert_eq!(t.stopped_for(0.0), t.stopped);
    assert_eq!(t.stopped_for(-5.0), t.stopped);
    assert_eq!(t.stopped_for(f64::NAN), t.stopped);
    // Longer dictations get more time for the recogniser's backlog, up to the cap.
    assert_eq!(t.stopped_for(30_000.0), t.stopped + 30 * t.stopped_per_audio_s);
    assert!(t.stopped_for(30_000.0) > t.stopped_for(3_000.0));
    assert_eq!(t.stopped_for(10.0 * 3_600_000.0), t.stopped_max);
    // A cap set below the floor never shortens the floor.
    let low = Timeouts {
        stopped_max: 100,
        ..t
    };
    assert_eq!(low.stopped_for(60_000.0), t.stopped);
}

/// Before the fix's wait was in place, anything that went ahead before the final pasted the last partial: the tail
/// words were gone. With the production timeouts, a final 1.2 s after release is waited for and pasted in full.
#[tokio::test(start_paused = true)]
async fn a_final_arriving_1200ms_after_release_is_pasted_in_full() {
    let r = rig(slow_final(Timeouts::default()));
    hold(&r, 500).await;
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    let partial_words = words(&r.view().partial);
    // Still waiting well past the old test bound, with nothing pasted yet.
    sleep(1000).await;
    assert_eq!(
        r.view().phase,
        TalkPhase::Finalizing,
        "went ahead before the final"
    );
    assert!(inserted(&r).is_empty(), "pasted before the final");
    until(|| !r.view().active(), 5000).await;
    let finals = r.helper.finals();
    assert_eq!(finals.len(), 1);
    assert_eq!(words(&finals[0]), partial_words + 3, "the final carries the tail");
    let e = r.last_entry().expect("kept");
    assert_eq!(e.raw, finals[0], "the real final, not the last partial");
    assert!(e.inserted && e.error.is_none(), "{:?}", e.error);
    let ins = inserted(&r);
    assert_eq!(ins.len(), 1, "pasted once");
    assert_eq!(ins[0].1, e.text);
    // The cleaned paste ends with the final's last words (the tail), not the partial's.
    let last = finals[0].split_whitespace().last().unwrap().to_lowercase();
    assert!(
        ins[0].1.to_lowercase().trim_end_matches('.').ends_with(&last),
        "{:?} vs {:?}",
        ins[0].1,
        finals[0]
    );
    assert!(r.clip.0.lock().unwrap().is_empty());
}

/// A press during that 1.2 s wait doesn't cancel or cut the first dictation's final: the next dictation starts once
/// the first is past finalizing, and both are pasted in full, in order (N110 / ADR 0074 still hold).
#[tokio::test(start_paused = true)]
async fn a_press_while_waiting_for_the_final_keeps_both_texts_in_order() {
    let r = rig(slow_final(Timeouts::default()));
    hold(&r, 500).await;
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    let first = r.view().id.expect("first id");
    sleep(300).await;
    r.helper.key("down");
    sleep(300).await;
    assert_eq!(
        r.view().phase,
        TalkPhase::Finalizing,
        "the first is still waiting for its final"
    );
    assert!(
        r.helper.sent_of("dictation.cancel").is_empty(),
        "nothing cancels the final in flight"
    );
    until(
        || r.view().phase == TalkPhase::Listening && r.view().id.as_deref() != Some(first.as_str()),
        3000,
    )
    .await;
    let second = r.view().id.expect("second id");
    sleep(400).await;
    r.helper.key("up");
    until(|| !r.view().active(), 5000).await;
    let finals = r.helper.finals();
    assert_eq!(finals.len(), 2);
    let h = r.svc.history();
    assert_eq!(h.len(), 2);
    assert_eq!(
        (h[1].id.as_str(), h[1].raw.as_str()),
        (first.as_str(), finals[0].as_str())
    );
    assert_eq!(
        (h[0].id.as_str(), h[0].raw.as_str()),
        (second.as_str(), finals[1].as_str())
    );
    assert!(h.iter().all(|e| e.inserted && e.error.is_none()));
    let ins = inserted(&r);
    assert_eq!(
        ins.iter().map(|(i, _)| i.clone()).collect::<Vec<_>>(),
        [first, second]
    );
    assert_eq!(ins[0].1, h[1].text);
    assert_eq!(ins[1].1, h[0].text);
    assert!(r.helper.sent_of("dictation.cancel").is_empty());
    assert!(r.helper.active().is_none(), "the mic is closed");
}

fn short_wait() -> Timeouts {
    Timeouts {
        stopped: 500,
        stopped_per_audio_s: 0,
        stopped_max: 500,
        helper_tail: None,
        ..Timeouts::default()
    }
}

/// Past the bound the dictation goes ahead on what was shown; the real final that comes later replaces the cut-short
/// text in history (and says so) instead of being dropped. Nothing is typed twice.
#[tokio::test(start_paused = true)]
async fn a_final_after_the_bound_is_kept_in_history_not_dropped() {
    let r = rig(slow_final(short_wait()));
    hold(&r, 500).await;
    until(|| r.svc.history().len() == 1, 3000).await;
    let cut = r.last_entry().unwrap();
    assert!(cut.error.is_none());
    sleep(1500).await;
    let finals = r.helper.finals();
    let e = r.last_entry().unwrap();
    assert_eq!(e.id, cut.id);
    assert_eq!(e.raw, finals[0], "the late final replaced the cut-short text");
    assert!(words(&e.raw) > words(&cut.raw));
    assert_eq!(e.error.as_deref(), Some(MSG_LATE_FINAL));
    assert_eq!(
        r.view().last.as_ref().map(|l| l.raw.clone()),
        Some(finals[0].clone())
    );
    assert_eq!(inserted(&r).len(), 1, "nothing typed twice");
    assert!(r.clip.0.lock().unwrap().is_empty());
    // It persisted.
    let disk = std::fs::read_to_string(&r.store).unwrap();
    assert!(disk.contains(MSG_LATE_FINAL));
    // The next dictation is unaffected.
    let r2 = r.dictate(300).await;
    assert!(!r2.active());
}

/// A late final that lands while the cut-short text is still being inserted is applied once that ends.
#[tokio::test(start_paused = true)]
async fn a_late_final_during_the_insert_is_applied_when_it_ends() {
    let mut o = slow_final(Timeouts {
        insert: 3000,
        ..short_wait()
    });
    o.helper.stopped_delay_ms = 800;
    o.helper.insert_delay_ms = 1500;
    let r = rig(o);
    hold(&r, 500).await;
    until(|| r.view().phase == TalkPhase::Inserting, 3000).await;
    until(|| !r.view().active(), 5000).await;
    let e = r.last_entry().unwrap();
    assert_eq!(e.raw, r.helper.finals()[0]);
    assert_eq!(e.error.as_deref(), Some(MSG_LATE_FINAL));
    assert!(e.inserted);
}

/// History off: the late full text goes to the clipboard (never lose the words).
#[tokio::test(start_paused = true)]
async fn a_late_final_with_history_off_goes_to_the_clipboard() {
    let mut o = slow_final(short_wait());
    o.config = serde_json::json!({"talk": {"saveHistory": false, "autoCleanup": false}});
    let r = rig(o);
    hold(&r, 500).await;
    until(|| !r.view().active(), 3000).await;
    sleep(1500).await;
    assert!(r.svc.history().is_empty());
    assert_eq!(*r.clip.0.lock().unwrap(), vec![r.helper.finals()[0].clone()]);
}

/// A lost helper sends no final: nothing waits for one, and a stray line from a new helper changes nothing.
#[tokio::test(start_paused = true)]
async fn a_restarted_helper_forgets_late_finals() {
    let r = rig(slow_final(short_wait()));
    hold(&r, 500).await;
    until(|| r.svc.history().len() == 1, 3000).await;
    let cut = r.last_entry().unwrap();
    r.helper.restart();
    sleep(1500).await;
    let e = r.last_entry().unwrap();
    assert_eq!(e.raw, cut.raw);
    assert!(e.error.is_none());
}
