//! N35 bounds (also packages/core/src/note/lifecycle.n35.test.ts): a helper that reconnects and fails its mic device
//! on every part keeps at most `DEVICE_FAILURES` records, each reason cut on code points, and every reconnect clears
//! the device-only flag.
use qrtz_lifecycle::*;
use qrtz_model::*;

fn step(s: RecordingState, e: LifecycleEvent) -> RecordingState {
    reduce(&s, &e).unwrap_or(s)
}

#[test]
fn device_failures_stay_bounded_across_a_reconnect_loop() {
    let t0: EpochMs = 1_760_000_000_000;
    let mut s = step(
        initial_state(),
        LifecycleEvent::StartRequested {
            meeting_id: "m".into(),
            at: t0,
            audio_path: "/r/p0.wav".into(),
        },
    );
    s = step(s, LifecycleEvent::StartAcked);
    for i in 0..20 {
        let at = t0 + i * 10_000;
        s = step(
            s,
            LifecycleEvent::CaptureDegraded {
                degraded: vec![Degradation::NoMic],
                reason: "🎙️ busy ".repeat(100),
                at: at + 1000,
            },
        );
        assert!(s.degraded.contains(&Degradation::NoMic));
        s = step(
            s,
            LifecycleEvent::HelperLost {
                at: at + 2000,
                deadline_at: at + 32000,
            },
        );
        s = step(
            s,
            LifecycleEvent::HelperRestored {
                at: at + 3000,
                audio_path: format!("/r/p{}.wav", i + 1),
            },
        );
        assert_eq!(s.degraded, vec![Degradation::TranscriptGap]);
        assert!(s.device_lost.is_empty());
    }
    let r = capture_report(&s).unwrap();
    assert_eq!(r.device_failures.len(), capture_limits::DEVICE_FAILURES);
    assert_eq!(r.device_failures[7].at_ms, 71_000);
    for d in &r.device_failures {
        assert_eq!(d.reason.chars().count(), capture_limits::REASON_CHARS);
    }
}
