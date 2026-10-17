//! Hardening cycle 10 (N45; ADR 0037): part offsets for helper-lost / helper-restored come from a monotonic clock,
//! so a wall jump during a reconnect cannot slide the next part's transcript while the WAV pad stays capped.

use std::sync::Arc;

use qrtz_lifecycle::runtime::{Clock, TokioClock};

#[tokio::test(start_paused = true)]
async fn tokio_clock_mono_ignores_wall_jump() {
    let clock = Arc::new(TokioClock::new(1_760_000_000_000));
    let m0 = clock.mono_ms();
    let w0 = clock.now_ms();
    assert_eq!(m0, w0);

    tokio::time::advance(std::time::Duration::from_millis(2_000)).await;
    let m1 = clock.mono_ms();
    let w1 = clock.now_ms();
    assert_eq!(m1 - m0, 2_000);
    assert_eq!(w1 - w0, 2_000);

    clock.jump(3_600_000);
    let m2 = clock.mono_ms();
    let w2 = clock.now_ms();
    assert_eq!(m2, m1, "mono must ignore clockJump");
    assert_eq!(w2 - w1, 3_600_000);

    tokio::time::advance(std::time::Duration::from_millis(1_000)).await;
    let m3 = clock.mono_ms();
    let w3 = clock.now_ms();
    assert_eq!(m3 - m0, 3_000);
    assert_eq!(w3 - w0, 3_600_000 + 3_000);

    // Meeting-relative restore offset: startedAt + (mono_now - mono_origin).
    let started = w0;
    let origin = m0;
    let at = started + (m3 - origin).max(0);
    assert_eq!(at - started, 3_000);
}
