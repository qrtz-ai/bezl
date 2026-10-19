//! Hardening cycle 27, N130 (ADR 0084): talk's stop wait was shorter than the helper's own release budget whenever
//! the recogniser had a backlog at the release (a long dictation, system audio and a meeting's ASR sharing the
//! Neural Engine). The helper (`DictationTail`, `QrtzEcho/Dictation.swift`) scales its drain and finalize waits
//! to what is still queued, which talk can't see; talk scaled to the audio length only (3 s + 100 ms/s, cap 20 s).
//! A 1 s dictation with its second still queued may take the helper ~5 s; talk gave up at 3.1 s and pasted the last
//! partial: the tail Jared hears cut off, with the full text only fixing history (MSG_LATE_FINAL). Now talk's bound
//! is the helper's worst case for that dictation (queued = audio) plus the pad and slack.
mod common;
use common::*;
use qrtz_talk::*;

fn swift_default(src: &str, name: &str) -> u64 {
    let at = src
        .find(&format!("{name}: Int = "))
        .unwrap_or_else(|| panic!("DictationTail.init has no {name} default"));
    let rest = &src[at + name.len() + ": Int = ".len()..];
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '_')).unwrap();
    rest[..end].replace('_', "").parse().unwrap()
}

/// The host's copy of the helper's numbers is the helper's: read from the Swift source, like playback_csp.rs.
#[test]
fn the_host_copy_of_the_helper_tail_matches_dictation_swift() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../native/qrtz-helper/Sources/QrtzEcho/Dictation.swift"),
    )
    .unwrap();
    let init = &src[src.find("public init(tailMs:").expect("DictationTail.init")..];
    let init = &init[..init.find('{').unwrap()];
    let t = HelperTail::default();
    assert_eq!(swift_default(init, "tailMs"), t.tail_ms);
    assert_eq!(swift_default(init, "drainTimeoutMs"), t.drain_timeout_ms);
    assert_eq!(swift_default(init, "padMs"), t.pad_ms);
    assert_eq!(swift_default(init, "finalizeTimeoutMs"), t.finalize_timeout_ms);
    assert_eq!(swift_default(init, "maxWaitMs"), t.max_wait_ms);
    // The formulas the host mirrors (a change there must change HelperTail too).
    for f in [
        "min(maxWaitMs, drainTimeoutMs + 2 * max(0, queuedMs))",
        "finalizeTimeoutMs + max(0, audioMs) / 10 + max(0, queuedMs)",
        "min(want, max(finalizeTimeoutMs, maxWaitMs - drainLimitMs(queuedMs: queuedMs)))",
        "tailMs + drainLimitMs(queuedMs: queuedMs) + finalizeLimitMs(audioMs: audioMs, queuedMs: queuedMs)",
    ] {
        assert!(
            src.contains(f),
            "Dictation.swift no longer has `{f}`: update HelperTail"
        );
    }
    // The production timeouts use it.
    assert_eq!(Timeouts::default().helper_tail, Some(t));
}

/// For every dictation length and every backlog it could have (queued ≤ captured), talk waits at least as long as
/// the helper may take. Before: a 1 s dictation fully queued (helper 4950 ms with the 50 ms tail, 5150 ms with the
/// old 250 ms one) got 3100 ms, and anything past ~3 min hit the 20 s cap under the helper's 21.55 s (was 21.75 s).
#[test]
fn talk_never_gives_up_before_the_helper_does() {
    let t = Timeouts::default();
    let h = HelperTail::default();
    let old = Timeouts {
        helper_tail: None,
        ..t
    };
    let mut old_short = 0;
    for a in (0..=600_000u64).step_by(250) {
        let wait = t.stopped_for(a as f64);
        for q in [0, a / 4, a / 2, a] {
            let need = h.budget_ms(a, q) + h.pad_ms;
            assert!(
                wait >= need,
                "audio {a} ms queued {q} ms: talk {wait} < helper {need}"
            );
            if old.stopped_for(a as f64) < need {
                old_short += 1;
            }
        }
    }
    assert!(old_short > 0, "the old bound was short somewhere (the finding)");
    assert!(old.stopped_for(1000.0) < h.budget_ms(1000, 1000));
    assert!(old.stopped_for(240_000.0) < h.budget_ms(240_000, 0));
    // Still bounded, and a short tap with a helper that keeps up still ends in a few seconds.
    assert!(t.stopped_for(f64::INFINITY) <= 25_000);
    assert!(t.stopped_for(f64::NAN) < 5_000);
    assert!(t.stopped_for(-1.0) < 5_000);
    assert!(t.stopped_for(10.0 * 3_600_000.0) <= 25_000);
    assert!(t.stopped_for(200.0) < 5_000, "{}", t.stopped_for(200.0));
}

async fn hold(r: &Rig, ms: u64) {
    r.helper.key("down");
    until(|| r.view().phase == TalkPhase::Listening, 2000).await;
    sleep(ms).await;
    r.helper.key("up");
}

/// A 1 s dictation whose final comes 4.5 s after the release (the recogniser still had the whole second queued):
/// pasted in full with the production timeouts. Before N130 talk went ahead at 3.1 s on the last partial.
#[tokio::test(start_paused = true)]
async fn a_backlogged_final_is_pasted_in_full_not_the_last_partial() {
    let r = rig(RigOpts {
        helper: HelperOpts {
            audio_every_ms: 100,
            stopped_delay_ms: 4500,
            final_tail_words: 3,
            ..Default::default()
        },
        timeouts: Timeouts::default(),
        ..Default::default()
    });
    hold(&r, 1000).await;
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    sleep(3500).await;
    assert_eq!(
        r.view().phase,
        TalkPhase::Finalizing,
        "went ahead before the helper's final"
    );
    until(|| !r.view().active(), 8000).await;
    let finals = r.helper.finals();
    assert_eq!(finals.len(), 1);
    let e = r.last_entry().expect("kept");
    assert_eq!(e.raw, finals[0], "the real final, not the last partial");
    assert!(e.error.is_none(), "{:?}", e.error);
    let ins = r.helper.sent_of("insert.text");
    assert_eq!(ins.len(), 1, "pasted once");
    assert_eq!(ins[0]["text"].as_str().unwrap(), format!("{} ", e.text));
}

/// The bound still ends a dictation whose helper never answers (a hung recogniser past the helper's own deadline):
/// what was shown goes in, and the dictation is not stuck.
#[tokio::test(start_paused = true)]
async fn a_helper_that_never_answers_still_ends_the_dictation_inside_the_bound() {
    let t = Timeouts::default();
    let r = rig(RigOpts {
        helper: HelperOpts {
            audio_every_ms: 100,
            stopped_delay_ms: 600_000,
            ..Default::default()
        },
        timeouts: t,
        ..Default::default()
    });
    hold(&r, 400).await;
    until(|| r.view().phase == TalkPhase::Finalizing, 2000).await;
    let bound = t.stopped_for(400.0 + 200.0);
    until(|| !r.view().active(), bound + t.insert + 2000).await;
    assert_eq!(r.svc.history().len(), 1, "what was shown is kept");
}

/// ADR 0118: the segmented copy (`HelperTail::segment_*`) follows the helper's `SegmentRule` defaults
/// (`QrtzEcho/Segmented.swift`, Bobs) once that file is in the tree: the cap counts the segment pad, which is
/// the release pad, and the default overlap is the one talk budgets. Before it lands, only the host-side numbers
/// are pinned (the rule chosen in docs/echo/dictation-tuning.md §11).
#[test]
fn the_segmented_copy_matches_the_helper_segment_rule() {
    let t = HelperTail::default();
    assert_eq!(
        (t.segment_cap_ms, t.segment_overlap_ms),
        (15_000, SEGMENT_OVERLAP_MS)
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../native/qrtz-helper/Sources/QrtzEcho/Segmented.swift");
    let Ok(src) = std::fs::read_to_string(path) else {
        return;
    };
    let init = &src[src.find("public init(pauseMs:").expect("SegmentRule.init")..];
    let init = &init[..init.find('{').unwrap()];
    assert_eq!(swift_default(init, "capMs"), t.segment_cap_ms);
    assert_eq!(swift_default(init, "overlapMs"), t.segment_overlap_ms);
    assert_eq!(swift_default(init, "segmentPadMs"), t.pad_ms);
}
