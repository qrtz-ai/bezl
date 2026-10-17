//! Hardening cycle 19 (ADR 0062). N95: a failed speech session tells the live notice why (`asr_unavailable`), and
//! the reason never outlives the `asr-unavailable` flag, whichever event drops the flag.
use qrtz_lifecycle::model::{
    AsrUnavailableReason, Degradation, LifecycleEvent, RecordingPhase, RecordingState,
};
use qrtz_lifecycle::{initial_state, reduce};
use serde_json::{json, Value};

fn ev(v: Value) -> LifecycleEvent {
    serde_json::from_value(v).unwrap()
}

fn step(s: &RecordingState, e: Value) -> RecordingState {
    reduce(s, &ev(e)).unwrap_or_else(|| s.clone())
}

fn recording() -> RecordingState {
    let s = step(
        &initial_state(),
        json!({"type": "start-requested", "meetingId": "m1", "at": 1_000, "audioPath": "/r/m1.wav"}),
    );
    let s = step(&s, json!({"type": "start-acked"}));
    assert_eq!(s.phase, RecordingPhase::Recording);
    s
}

fn asr(reason: &str) -> Value {
    json!({"type": "asr-unavailable", "reason": reason, "at": 2_000})
}

#[test]
fn n95_a_speech_failure_records_its_reason_with_the_flag() {
    let s = step(&recording(), asr("not-installed"));
    assert_eq!(s.degraded, vec![Degradation::AsrUnavailable]);
    assert_eq!(s.asr_unavailable, Some(AsrUnavailableReason::NotInstalled));
    let j = serde_json::to_value(&s).unwrap();
    assert_eq!(j["asrUnavailable"], "not-installed");
    // A row written before N95 has no key and loads as unknown.
    let mut old = j.clone();
    old.as_object_mut().unwrap().remove("asrUnavailable");
    let back: RecordingState = serde_json::from_value(old).unwrap();
    assert_eq!(back.asr_unavailable, None);
    assert!(back.degraded.contains(&Degradation::AsrUnavailable));
}

#[test]
fn n95_the_helpers_own_report_has_no_reason() {
    let s = step(
        &recording(),
        json!({"type": "degraded", "flag": "asr-unavailable", "at": 2_000}),
    );
    assert!(s.degraded.contains(&Degradation::AsrUnavailable));
    assert_eq!(s.asr_unavailable, None);
    assert!(serde_json::to_value(&s).unwrap().get("asrUnavailable").is_none());
}

#[test]
fn n95_the_latest_reason_wins_and_a_repeat_is_a_no_op() {
    let s = step(&recording(), asr("crashed"));
    let r = reduce(&s, &ev(asr("crashed")));
    assert!(r.is_none(), "same reason again changes nothing");
    let s = reduce(&s, &ev(asr("revoked"))).expect("a new reason is a change");
    assert_eq!(s.asr_unavailable, Some(AsrUnavailableReason::Revoked));
    assert_eq!(
        s.degraded,
        vec![Degradation::AsrUnavailable],
        "flag not duplicated"
    );
}

#[test]
fn n95_not_capturing_records_nothing() {
    assert!(reduce(&initial_state(), &ev(asr("stale"))).is_none());
}

#[test]
fn n95_speech_ready_drops_flag_and_reason() {
    let s = step(&recording(), asr("timeout"));
    let s = step(&s, json!({"type": "speech-ready"}));
    assert!(s.degraded.is_empty());
    assert_eq!(s.asr_unavailable, None);
}

#[test]
fn n95_a_helper_loss_drops_flag_and_reason() {
    let s = step(&recording(), asr("crashed"));
    let s = step(
        &s,
        json!({"type": "helper-lost", "at": 3_000, "deadlineAt": 33_000}),
    );
    assert_eq!(s.phase, RecordingPhase::Reconnecting);
    assert!(!s.degraded.contains(&Degradation::AsrUnavailable));
    assert_eq!(s.asr_unavailable, None);
}

/// Property: over random event sequences the reason is set only while the flag is.
#[test]
fn n95_reason_never_outlives_the_flag_over_random_sequences() {
    let alphabet = [
        json!({"type": "start-requested", "meetingId": "m1", "at": 1_000, "audioPath": "/r/a.wav"}),
        json!({"type": "start-acked"}),
        asr("not-installed"),
        asr("crashed"),
        json!({"type": "degraded", "flag": "asr-unavailable", "at": 2_000}),
        json!({"type": "degraded", "flag": "audio-unavailable", "at": 2_000}),
        json!({"type": "capture-degraded", "degraded": ["asr-unavailable"], "reason": "device", "at": 2_100}),
        json!({"type": "speech-ready"}),
        json!({"type": "helper-lost", "at": 3_000, "deadlineAt": 33_000}),
        json!({"type": "helper-restored", "at": 4_000, "audioPath": "/r/b.wav"}),
        json!({"type": "stop-requested", "reason": "manual", "at": 5_000}),
        json!({"type": "stopped", "at": 5_100}),
        json!({"type": "processing-done"}),
        json!({"type": "reset"}),
    ];
    let events: Vec<LifecycleEvent> = alphabet.into_iter().map(ev).collect();
    let mut seed: u64 = 0x5eed_0019;
    for _ in 0..2_000 {
        let mut s = initial_state();
        for _ in 0..40 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let e = &events[(seed >> 33) as usize % events.len()];
            if let Some(n) = reduce(&s, e) {
                s = n;
            }
            assert!(
                s.asr_unavailable.is_none() || s.degraded.contains(&Degradation::AsrUnavailable),
                "reason without the flag after {e:?}: {s:?}"
            );
        }
    }
}
