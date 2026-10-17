//! Hardening cycle 24 (N112; ADR 0077): `resummarize` waits for the retried summary step as long as the step can
//! actually run (every attempt's timeout plus its backoff), not a fixed 120 s left from the cloud timeout.
use qrtz_lifecycle::policies;
use qrtz_lifecycle::runtime::{settle_budget_ms, SUMMARY_TIMEOUT_MS};
use qrtz_model::StepId;

/// qrtz-app `summary::ON_DEVICE_SUMMARY_TIMEOUT_MS` (ADR 0075/0078). qrtz-lifecycle can't depend on qrtz-app, so
/// qrtz-app's `tests/hardening24.rs` asserts this equals the real constant (it fails if the two drift).
pub const ON_DEVICE_SUMMARY_TIMEOUT_MS: u64 = 3_660_000;

/// The settle budget for `t`: every attempt's timeout plus its backoff at +20 % jitter (2.4 s, 4.8 s, 9.6 s; base
/// 2 s, cap 30 s) and 10 s of slack.
fn expected_budget(t: u64) -> u64 {
    policies::SUMMARY.max_attempts as u64 * t + 2_400 + 4_800 + 9_600 + 10_000
}

#[test]
fn the_resummarize_wait_covers_every_on_device_attempt_and_backoff() {
    // The current constant, the old 15-min one, and a few around them: the formula, not one literal.
    for on_device in [ON_DEVICE_SUMMARY_TIMEOUT_MS, 900_000, 120_001, 7_200_000] {
        let b = settle_budget_ms(StepId::Summary, on_device);
        let attempts = policies::SUMMARY.max_attempts as u64;
        assert!(b >= attempts * on_device, "{on_device}: {b}");
        assert_eq!(b, expected_budget(on_device), "{on_device}");
        // The old fixed bound (2400 × 50 ms) was shorter than one on-device attempt.
        assert!(120_000 < on_device);
    }
}

#[test]
fn the_budget_scales_with_the_configured_timeout_and_never_overflows() {
    let cloud = settle_budget_ms(StepId::Summary, SUMMARY_TIMEOUT_MS);
    assert!(cloud > SUMMARY_TIMEOUT_MS * 3, "{cloud}");
    assert!(settle_budget_ms(StepId::Summary, 1) < cloud);
    assert_eq!(settle_budget_ms(StepId::Summary, u64::MAX), u64::MAX);
}
