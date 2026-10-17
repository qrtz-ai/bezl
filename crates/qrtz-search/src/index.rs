//! The index step (port of the worker's `note.stepIndex` and `indexVectors`) and the shared embed loop.
//!
//! - **Deadline (ADR 0007, 45b37cd):** one attempt, model load included, is bounded by [`INDEX_DEADLINE_MS`]
//!   (280 s, inside the runtime's 300 s RPC timeout). Past it the batch still embedding is dropped and never saved,
//!   no batch starts, and the attempt fails with a retryable E_TIMEOUT; batches saved before stay, so the retry
//!   resumes where it stopped (`missing_vectors` is the cursor).
//! - **No embedder** (none configured, or it failed to load): `{indexed: 0, skipped: true}` and the meeting is
//!   searchable by full text only. The backfill embeds it once a model is ready.
//! - **Only current lines are saved:** a vector is stored only if its line still exists with the embedded text
//!   (`save_vectors_if_current`), so a delete or re-finalize during a batch never attaches it to another line.
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::{
    lifecycle_error, DbExec, FinalizeOutcome, IndexOutcome, StepExecutor, SummaryRequest,
};
use qrtz_model::{LifecycleError, MeetingSummary, PipelineContext};
use qrtz_storage::VectorRow;

use crate::embedder::{vec_to_blob, Embedder, LazyEmbedder};

/// Worker-side deadline for one index attempt (TS `INDEX_DEADLINE_MS`).
pub const INDEX_DEADLINE_MS: u64 = 280_000;
/// Lines per embed call (TS: 64).
pub const BATCH: u32 = 64;
/// Lines one index step embeds at most (TS: 100 000).
pub const STEP_CAP: u32 = 100_000;

pub(crate) fn deadline_error(what: &str) -> LifecycleError {
    LifecycleError {
        code: "E_TIMEOUT".into(),
        message: format!("{what} deadline exceeded"),
        retryable: true,
    }
}

/// Should the loop stop before the next batch (backfill cancel, a tombstone)?
pub type StopFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Embeds up to `cap` lines without a vector for `e` (one meeting, or all), in batches of [`BATCH`]. Returns the
/// number saved. `stop(meeting_id)` is asked before each batch and, for the meeting of each line, before saving.
pub async fn index_vectors<E: DbExec>(
    db: &E,
    e: &Arc<dyn Embedder>,
    cap: u32,
    meeting_id: Option<String>,
    stop: Option<&StopFn>,
) -> Result<u32, LifecycleError> {
    let mut done = 0u32;
    let mut seen = 0u32;
    while seen < cap {
        let (model, m) = (e.id().to_string(), meeting_id.clone());
        let want = BATCH.min(cap - seen);
        let mut batch = db
            .run(move |db| db.notes().missing_vectors(&model, want, m.as_deref()))
            .await
            .map_err(lifecycle_error)?;
        if let Some(stop) = stop {
            batch.retain(|b| !stop(&b.meeting_id));
        }
        if batch.is_empty() {
            break;
        }
        seen += batch.len() as u32;
        let texts: Vec<String> = batch.iter().map(|b| b.text.clone()).collect();
        let vecs = e.embed(&texts).await?;
        if vecs.len() != batch.len() {
            return Err(LifecycleError {
                code: "E_INTERNAL".into(),
                message: format!(
                    "embedder returned {} vectors for {} lines",
                    vecs.len(),
                    batch.len()
                ),
                retryable: false,
            });
        }
        let rows: Vec<(VectorRow, String)> = batch
            .into_iter()
            .zip(vecs)
            .filter(|(b, _)| stop.is_none_or(|s| !s(&b.meeting_id)))
            .map(|(b, v)| {
                (
                    VectorRow {
                        rowid: b.rowid,
                        meeting_id: b.meeting_id,
                        vec: vec_to_blob(&v),
                    },
                    b.text,
                )
            })
            .collect();
        let model = e.id().to_string();
        let saved = db
            .run(move |db| db.notes().save_vectors_if_current(&model, &rows))
            .await
            .map_err(lifecycle_error)?;
        done += saved;
        if saved == 0 && meeting_id.is_some() {
            // Every line of this batch changed under us (re-finalized): the next query sees the new rows.
            continue;
        }
    }
    Ok(done)
}

/// The index [`StepExecutor`]; finalize, summary and resummarize go to `inner` (`LlmSteps`).
pub struct SearchSteps<E: DbExec> {
    pub db: E,
    pub inner: Arc<dyn StepExecutor>,
    pub embedder: Arc<LazyEmbedder>,
    pub deadline: Duration,
}

impl<E: DbExec> SearchSteps<E> {
    pub fn new(db: E, inner: Arc<dyn StepExecutor>, embedder: Arc<LazyEmbedder>) -> Self {
        SearchSteps {
            db,
            inner,
            embedder,
            deadline: Duration::from_millis(INDEX_DEADLINE_MS),
        }
    }

    async fn index_inner(&self, id: String) -> Result<IndexOutcome, LifecycleError> {
        let Some(e) = self.embedder.get().await else {
            return Ok(IndexOutcome {
                indexed: 0,
                skipped: true,
            });
        };
        let indexed = index_vectors(&self.db, &e, STEP_CAP, Some(id), None).await?;
        Ok(IndexOutcome {
            indexed,
            skipped: false,
        })
    }
}

#[async_trait]
impl<E: DbExec> StepExecutor for SearchSteps<E> {
    async fn finalize(
        &self,
        meeting_id: String,
        ctx: PipelineContext,
    ) -> Result<FinalizeOutcome, LifecycleError> {
        self.inner.finalize(meeting_id, ctx).await
    }

    async fn summary(
        &self,
        meeting_id: String,
        req: SummaryRequest,
    ) -> Result<Option<MeetingSummary>, LifecycleError> {
        self.inner.summary(meeting_id, req).await
    }

    /// `note.stepIndex`: `need(id)`, then the embed loop for this meeting under the deadline.
    async fn index(&self, meeting_id: String) -> Result<IndexOutcome, LifecycleError> {
        let id = meeting_id.clone();
        self.db
            .run(move |db| db.notes().need(&id).map(|_| ()))
            .await
            .map_err(lifecycle_error)?;
        tokio::time::timeout(self.deadline, self.index_inner(meeting_id))
            .await
            .unwrap_or_else(|_| Err(deadline_error("index")))
    }

    async fn resummarize(
        &self,
        meeting_id: String,
        template: Option<String>,
    ) -> Result<MeetingSummary, LifecycleError> {
        self.inner.resummarize(meeting_id, template).await
    }
}
