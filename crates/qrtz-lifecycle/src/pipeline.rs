use qrtz_model::{
    EpochMs, LifecycleError, PipelineContext, PipelineState, PipelineSteps, PipelineView, StepId, StepState,
    StepStatus, StopReason,
};
use serde::{Deserialize, Serialize};

use crate::policies;

/// Order the steps run in.
pub const STEP_ORDER: [StepId; 3] = [StepId::Finalize, StepId::Summary, StepId::Index];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_ms: u64,
    pub max_ms: u64,
}

/// Exponential backoff with ±20% jitter: `round(min(max, base·2^(attempt-1)) · (0.8 + random·0.4) · scale)`.
/// `random` is in [0, 1]; `scale` shrinks every delay in tests (`QRTZ_RETRY_SCALE`). Same float operation order
/// as the TS `retryDelay`, so results are identical.
pub fn retry_delay(p: &RetryPolicy, attempt: u32, random: f64, scale: f64) -> u64 {
    let exp = (p.base_ms as f64 * 2f64.powi(attempt.saturating_sub(1) as i32)).min(p.max_ms as f64);
    let v = (exp * (0.8 + random * 0.4) * scale).round();
    if v <= 0.0 {
        0
    } else {
        v as u64
    }
}

/// `maxRetryDelay`: the longest delay [`retry_delay`] can draw for this attempt (jitter at +20%). The runtime never
/// waits longer than this for a persisted (wall-clock) `next_retry_at`, so a clock that jumped back cannot stall a
/// retry (N12).
pub fn max_retry_delay(p: &RetryPolicy, attempt: u32, scale: f64) -> u64 {
    retry_delay(p, attempt, 1.0, scale)
}

/// One pipeline step transition (`StepPatch` in lifecycle.ts). Serde shape = the vector JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum StepPatch {
    Start {
        now: EpochMs,
    },
    Success {
        now: EpochMs,
        /// Accepted and ignored, like TS.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fallback: bool,
    },
    Skip {
        now: EpochMs,
    },
    Failure {
        now: EpochMs,
        error: LifecycleError,
        delay_ms: u64,
        /// Final failure regardless of attempts left (e.g. 401).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        terminal: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fallback: bool,
    },
    ManualRetry {
        now: EpochMs,
    },
}

impl StepPatch {
    pub fn now(&self) -> EpochMs {
        match self {
            StepPatch::Start { now }
            | StepPatch::Success { now, .. }
            | StepPatch::Skip { now }
            | StepPatch::Failure { now, .. }
            | StepPatch::ManualRetry { now } => *now,
        }
    }
}

/// `newPipeline`: every step `pending` with its policy's attempt budget.
pub fn new_pipeline(
    meeting_id: &str,
    stop_reason: StopReason,
    ctx: PipelineContext,
    now: EpochMs,
) -> PipelineState {
    PipelineState {
        meeting_id: meeting_id.to_string(),
        stop_reason,
        ctx,
        updated_at: now,
        capture: None,
        steps: PipelineSteps {
            finalize: StepState::pending(policies::FINALIZE.max_attempts),
            summary: StepState::pending(policies::SUMMARY.max_attempts),
            index: StepState::pending(policies::INDEX.max_attempts),
        },
    }
}

/// `pipelineStep`: pure step transition. Failure with attempts left → `retrying` + `nextRetryAt`; otherwise
/// `failed`. Quirks kept for parity: `start` keeps the previous error; `success` keeps `startedAt` only when
/// non-zero (TS truthiness); `manual-retry` on finalize re-arms every later step, even `done` ones.
pub fn pipeline_step(p: &PipelineState, id: StepId, patch: StepPatch) -> PipelineState {
    let cur = p.steps.get(id).clone();
    let next = match &patch {
        StepPatch::Start { now } => StepState {
            status: StepStatus::Running,
            attempts: cur.attempts + 1,
            started_at: Some(*now),
            next_retry_at: None,
            ..cur
        },
        StepPatch::Success { now, .. } => StepState {
            status: StepStatus::Done,
            finished_at: Some(*now),
            started_at: cur.started_at.filter(|t| *t != 0),
            ..StepState::pending(cur.max_attempts).with_attempts(cur.attempts)
        },
        StepPatch::Skip { now } => StepState {
            status: StepStatus::Skipped,
            finished_at: Some(*now),
            ..StepState::pending(cur.max_attempts).with_attempts(cur.attempts)
        },
        StepPatch::Failure {
            now,
            error,
            delay_ms,
            terminal,
            fallback,
        } => {
            let left = !terminal && error.retryable && cur.attempts < cur.max_attempts;
            if left {
                StepState {
                    status: StepStatus::Retrying,
                    error: Some(error.clone()),
                    next_retry_at: Some(now + *delay_ms as i64),
                    ..cur
                }
            } else {
                StepState {
                    status: StepStatus::Failed,
                    error: Some(error.clone()),
                    finished_at: Some(*now),
                    fallback: if *fallback { Some(true) } else { cur.fallback },
                    next_retry_at: None,
                    ..cur
                }
            }
        }
        // A user retry grants a fresh budget for this step and re-arms every later step.
        StepPatch::ManualRetry { .. } => StepState::pending(cur.max_attempts),
    };
    let mut steps = p.steps.clone();
    *steps.get_mut(id) = next;
    if matches!(patch, StepPatch::ManualRetry { .. }) {
        let after = STEP_ORDER
            .iter()
            .position(|s| *s == id)
            .unwrap_or(STEP_ORDER.len())
            + 1;
        for later in &STEP_ORDER[after..] {
            let st = steps.get_mut(*later);
            if st.status != StepStatus::Done || id == StepId::Finalize {
                *st = StepState::pending(st.max_attempts);
            }
        }
    }
    PipelineState {
        steps,
        updated_at: patch.now(),
        ..p.clone()
    }
}

trait WithAttempts {
    fn with_attempts(self, attempts: u32) -> Self;
}
impl WithAttempts for StepState {
    fn with_attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts;
        self
    }
}

fn is_final(s: StepStatus) -> bool {
    matches!(s, StepStatus::Done | StepStatus::Failed | StepStatus::Skipped)
}

/// `nextRunnableStep`: the next step to run, or `None` when the pipeline is settled.
pub fn next_runnable_step(p: &PipelineState) -> Option<StepId> {
    for id in STEP_ORDER {
        match p.steps.get(id).status {
            StepStatus::Done | StepStatus::Skipped => continue,
            StepStatus::Failed => {
                // Summary failure is soft (fallback summary saved): indexing still runs. Finalize failure blocks.
                if id == StepId::Summary {
                    continue;
                }
                return None;
            }
            _ => return Some(id),
        }
    }
    None
}

/// `pipelineSettled`.
pub fn pipeline_settled(p: &PipelineState) -> bool {
    STEP_ORDER.iter().all(|id| is_final(p.steps.get(*id).status))
        || p.steps.finalize.status == StepStatus::Failed
}

/// `pipelineUsable`: transcript final and some summary settled.
pub fn pipeline_usable(p: &PipelineState) -> bool {
    p.steps.finalize.status == StepStatus::Done && is_final(p.steps.summary.status)
}

/// `toPipelineView`: what the renderer gets (ctx stays in the core).
pub fn to_view(p: &PipelineState) -> PipelineView {
    PipelineView {
        meeting_id: p.meeting_id.clone(),
        stop_reason: p.stop_reason,
        steps: p.steps.clone(),
        updated_at: p.updated_at,
        capture: p.capture.clone(),
    }
}

/// Crash recovery R2: a step caught mid-run by the crash is re-run (every step is idempotent); attempts are kept.
pub fn resume_after_restart(mut p: PipelineState) -> PipelineState {
    for id in STEP_ORDER {
        let s = p.steps.get_mut(id);
        if s.status == StepStatus::Running {
            s.status = StepStatus::Retrying;
        }
    }
    p
}

/// `progressLabel` (note-service.ts): the session's processing label.
pub fn progress_label(p: Option<&PipelineState>) -> String {
    let Some(p) = p else { return "Finishing".into() };
    let Some(step) = next_runnable_step(p) else {
        return "Finishing".into();
    };
    let name = match step {
        StepId::Finalize => "Saving transcript",
        StepId::Summary => "Summarizing",
        StepId::Index => "Indexing",
    };
    if p.steps.get(step).status == StepStatus::Retrying {
        format!("{name} (retrying)")
    } else {
        name.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> PipelineContext {
        PipelineContext {
            started_at: 0,
            ended_at: 1,
            duration_ms: 1,
            audio_parts: vec![],
            part_offsets_ms: None,
            active: vec![],
            helper_crashed: false,
            speaker_turns: vec![],
        }
    }

    #[test]
    fn restart_turns_running_into_retrying_and_keeps_attempts() {
        let p = new_pipeline("m", StopReason::Manual, ctx(), 0);
        let p = pipeline_step(&p, StepId::Finalize, StepPatch::Start { now: 5 });
        let r = resume_after_restart(p);
        assert_eq!(r.steps.finalize.status, StepStatus::Retrying);
        assert_eq!(r.steps.finalize.attempts, 1);
        assert_eq!(next_runnable_step(&r), Some(StepId::Finalize));
    }

    #[test]
    fn progress_labels_match_note_service() {
        assert_eq!(progress_label(None), "Finishing");
        let p = new_pipeline("m", StopReason::Manual, ctx(), 0);
        assert_eq!(progress_label(Some(&p)), "Saving transcript");
        let p = pipeline_step(&p, StepId::Finalize, StepPatch::Start { now: 1 });
        let err = LifecycleError {
            code: "E".into(),
            message: "x".into(),
            retryable: true,
        };
        let p = pipeline_step(
            &p,
            StepId::Finalize,
            StepPatch::Failure {
                now: 2,
                error: err,
                delay_ms: 10,
                terminal: false,
                fallback: false,
            },
        );
        assert_eq!(progress_label(Some(&p)), "Saving transcript (retrying)");
        assert_eq!(to_view(&p).updated_at, 2);
    }
}
