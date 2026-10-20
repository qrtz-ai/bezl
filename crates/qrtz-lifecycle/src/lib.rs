//! The single authoritative recording lifecycle (docs/recording-lifecycle.md), ported from
//! `packages/core/src/note/lifecycle.ts`.
//!
//! Pure: no IO, no clock, no randomness. The reducers own every legal transition; the runtime owner feeds events
//! in, persists every accepted state and broadcasts the derived views. Behaviour, including the TS quirks kept
//! for parity, is pinned by the shared vectors in `spec/vectors/lifecycle` (run by vitest and `cargo test`).

#![forbid(unsafe_code)]
mod arbiter;
pub mod fusion;
pub(crate) mod js;
mod pipeline;
mod poison;
#[cfg(feature = "runtime")]
pub mod runtime;
mod session;
pub mod speakers;
pub mod transcript;
pub mod wav;

pub use arbiter::{
    arbiter_invariant_violation, arbiter_step, degradations_for_revocation, ArbiterCmd, ArbiterEvent,
    ArbiterResult, ArbiterState, Configure, Decision, FileFailed, MicSubscriber,
};

pub use pipeline::{
    max_retry_delay, new_pipeline, next_runnable_step, pipeline_settled, pipeline_step, pipeline_usable,
    progress_label, resume_after_restart, retry_delay, to_view, RetryPolicy, StepPatch, STEP_ORDER,
};
pub use qrtz_model as model;
pub use session::{
    can_start, capture_limits, capture_report, cut_chars, initial_state, is_capturing, reduce,
    silent_input_view, source_lost, Machine, CAPTURING,
};

/// `RETRY_POLICIES` (lifecycle.ts). Delays are milliseconds before jitter and `QRTZ_RETRY_SCALE`.
pub mod policies {
    use super::RetryPolicy;
    use qrtz_model::StepId;

    /// Helper restarts while recording: give up (stop with `helper-lost`) after this window.
    pub const HELPER_RECONNECT_WINDOW_MS: u64 = 30_000;
    /// `recording.started` ack.
    pub const START_ACK_MS: u64 = 5_000;
    /// `recording.stopped` ack (helper flushes recogniser + WAV).
    pub const STOP_ACK_MS: u64 = 10_000;
    /// N2: `recording.aborted` ack after a missed stop ack (hard deadline; then the helper is restarted).
    pub const ABORT_ACK_MS: u64 = 2_000;
    pub const APPEND_LIVE: RetryPolicy = RetryPolicy {
        max_attempts: 5,
        base_ms: 250,
        max_ms: 4_000,
    };
    pub const FINALIZE: RetryPolicy = RetryPolicy {
        max_attempts: 3,
        base_ms: 1_000,
        max_ms: 16_000,
    };
    pub const SUMMARY: RetryPolicy = RetryPolicy {
        max_attempts: 3,
        base_ms: 2_000,
        max_ms: 30_000,
    };
    pub const INDEX: RetryPolicy = RetryPolicy {
        max_attempts: 3,
        base_ms: 5_000,
        max_ms: 60_000,
    };

    /// The policy of one post-processing step.
    pub fn for_step(id: StepId) -> RetryPolicy {
        match id {
            StepId::Finalize => FINALIZE,
            StepId::Summary => SUMMARY,
            StepId::Index => INDEX,
        }
    }
}
