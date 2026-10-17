//! The step executor the app runs today: the real finalize (WAV stitch/repair + speaker fusion, a port of the
//! worker's `note.stepFinalize`), a placeholder summary (qrtz-llm wraps it) and the index step, a no-op: search
//! is full-text and there are no embeddings (ADR 0073).

use async_trait::async_trait;
use qrtz_model::{LifecycleError, Meeting, MeetingStatus, MeetingSummary, PipelineContext, SegmentSource};
use qrtz_storage::{Db, StorageError};

use super::api::{FinalizeOutcome, IndexOutcome, StepExecutor, SummaryRequest};
use super::store::{lifecycle_error, DbExec};
use crate::speakers::attribute;
use crate::wav::stitch_parts;

/// `note.stepFinalize`, idempotent: stitch the WAV parts (one per helper restart) into the first part, repair
/// the header, record duration, attribute speakers (per-channel diarization, AX names) on the live segments.
pub fn finalize_sync(
    db: &Db,
    meeting_id: &str,
    ctx: &PipelineContext,
) -> qrtz_storage::Result<FinalizeOutcome> {
    let repo = db.notes();
    let m = repo.need(meeting_id)?;
    // H7: reconnect gaps are padded with silence when the context has the part offsets (ADR 0032).
    let audio_ms =
        stitch_parts(&ctx.audio_parts, ctx.part_offsets_ms.as_deref()).map_err(StorageError::Io)?;
    let segs = repo.get_segments(meeting_id)?;
    let truthy = |v: Option<i64>| v.filter(|x| *x != 0);
    // `(helperCrashed && audioMs) || ctx.durationMs || audioMs || Math.max(0, ...segment ends)`
    let duration_ms = truthy(audio_ms.filter(|_| ctx.helper_crashed))
        .or(truthy(Some(ctx.duration_ms)))
        .or(truthy(audio_ms))
        .unwrap_or_else(|| segs.iter().map(|s| s.end_ms).fold(0, i64::max));
    let main = ctx
        .audio_parts
        .first()
        .filter(|p| std::path::Path::new(p).exists())
        .cloned();
    repo.upsert_meeting(&Meeting {
        ended_at: Some(ctx.ended_at),
        duration_ms: Some(duration_ms),
        status: MeetingStatus::Processing,
        audio_path: main.or(m.audio_path.clone()),
        ..m
    })?;
    // Per-channel diarized speakers ("me" by default for the mic's main voice) and AX names (ADR 0058 §12); with
    // no turns, AX fusion only, as before.
    if !ctx.active.is_empty() || !ctx.speaker_turns.is_empty() {
        repo.replace_segments(
            meeting_id,
            SegmentSource::Live,
            &attribute(&segs, &ctx.speaker_turns, &ctx.active, duration_ms),
        )?;
    }
    Ok(FinalizeOutcome {
        duration_ms,
        segments: segs.len() as u32,
    })
}

/// Production executor for the Tauri build without qrtz-llm (and the inner executor of `LlmSteps`):
/// - finalize: real;
/// - summary: no LLM yet, so a strict request fails non-retryably and the runtime falls back; the fallback (and
///   `Skip`) mark the meeting done without a summary (the on-device extractive summary is not ported yet);
/// - index: always `skipped`. Search is full-text (`NotesRepo::search`, FTS5 kept current by the storage writes);
///   there are no embeddings since the chat was removed (ADR 0073).
pub struct CoreSteps<E: DbExec> {
    pub db: E,
}

const NO_LLM: &str = "Summaries are not available in this build yet";

impl<E: DbExec> CoreSteps<E> {
    async fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> qrtz_storage::Result<R> + Send + 'static,
    ) -> Result<R, LifecycleError> {
        self.db.run(f).await.map_err(lifecycle_error)
    }
}

#[async_trait]
impl<E: DbExec> StepExecutor for CoreSteps<E> {
    async fn finalize(
        &self,
        meeting_id: String,
        ctx: PipelineContext,
    ) -> Result<FinalizeOutcome, LifecycleError> {
        self.run(move |db| finalize_sync(db, &meeting_id, &ctx)).await
    }
    async fn summary(
        &self,
        meeting_id: String,
        req: SummaryRequest,
    ) -> Result<Option<MeetingSummary>, LifecycleError> {
        if req == SummaryRequest::Llm {
            self.run(move |db| db.notes().need(&meeting_id).map(|_| ()))
                .await?;
            return Err(LifecycleError {
                code: "E_UNAVAILABLE".into(),
                message: NO_LLM.into(),
                retryable: false,
            });
        }
        self.run(move |db| {
            db.notes().need(&meeting_id)?;
            db.notes().set_status(&meeting_id, MeetingStatus::Done)?;
            Ok(None)
        })
        .await
    }
    async fn index(&self, meeting_id: String) -> Result<IndexOutcome, LifecycleError> {
        self.run(move |db| {
            db.notes().need(&meeting_id).map(|_| IndexOutcome {
                indexed: 0,
                skipped: true,
            })
        })
        .await
    }
    async fn resummarize(
        &self,
        meeting_id: String,
        template: Option<String>,
    ) -> Result<MeetingSummary, LifecycleError> {
        self.run(move |db| {
            db.notes().need(&meeting_id)?;
            if let Some(t) = template {
                db.notes().set_template(&meeting_id, &t)?;
            }
            Ok(())
        })
        .await?;
        Err(LifecycleError {
            code: "E_UNAVAILABLE".into(),
            message: NO_LLM.into(),
            retryable: false,
        })
    }
}
