//! The index step (port of the worker's `note.stepIndex` and `indexVectors`) and the shared embed loop.
//!
//! - **Deadline (ADR 0007, 45b37cd):** one attempt, model load included, is bounded by [`INDEX_DEADLINE_MS`]
//!   (280 s, inside the runtime's 300 s RPC timeout). Past it the batch still embedding is dropped and never saved,
//!   no batch starts, and the attempt fails with a retryable E_TIMEOUT; batches saved before stay, so the retry
//!   resumes where it stopped (`missing_vectors` is the cursor).
//! - **No embedder** (none configured, or it failed to load): `{indexed: 0, skipped: true}` and the meeting is
//!   searchable by full text only. The backfill embeds it once a model is ready.
//! - **A model still loading (N104, ADR 0068):** the step waits at most [`INDEX_MODEL_WAIT_MS`] for the first load
//!   to settle, then skips the same way. The load can include the first-run search-model download (90 MB), and
//!   summary and index share one queue across meetings, so waiting the whole deadline (three attempts) would hold
//!   every later meeting's summary behind a download.
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
/// N104: how long one index attempt waits for a model that is still loading before it skips (a local load takes
/// a second or two; a download can take minutes). The backfill embeds a skipped meeting once the model is ready.
pub const INDEX_MODEL_WAIT_MS: u64 = 15_000;

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
    index_vectors_paused(db, e, cap, meeting_id, stop, None).await
}

/// [`index_vectors`] that also returns early, before starting a batch, while `pause()` is true (the backfill's busy
/// gate, N82). Unlike `stop`, a pause never drops a batch that was already embedded: it is saved.
pub async fn index_vectors_paused<E: DbExec>(
    db: &E,
    e: &Arc<dyn Embedder>,
    cap: u32,
    meeting_id: Option<String>,
    stop: Option<&StopFn>,
    pause: Option<&crate::backfill::BusyFn>,
) -> Result<u32, LifecycleError> {
    let mut done = 0u32;
    let mut seen = 0u32;
    while seen < cap {
        if pause.is_some_and(|p| p()) {
            break;
        }
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
            .map(|(b, mut v)| {
                if v.iter().any(|x| !x.is_finite()) {
                    // A NaN/inf vector (a zero-norm line in a model that divides by the norm) would rank
                    // arbitrarily forever; store a zero vector so the line is indexed but never similar (N79).
                    tracing::warn!(
                        rowid = b.rowid,
                        "embedder returned a non-finite vector; stored as zero"
                    );
                    v.iter_mut().for_each(|x| *x = 0.0);
                }
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
    /// N104: the longest wait for a model that is still loading ([`INDEX_MODEL_WAIT_MS`]).
    pub model_wait: Duration,
}

impl<E: DbExec> SearchSteps<E> {
    pub fn new(db: E, inner: Arc<dyn StepExecutor>, embedder: Arc<LazyEmbedder>) -> Self {
        SearchSteps {
            db,
            inner,
            embedder,
            deadline: Duration::from_millis(INDEX_DEADLINE_MS),
            model_wait: Duration::from_millis(INDEX_MODEL_WAIT_MS),
        }
    }

    async fn index_inner(&self, id: String) -> Result<IndexOutcome, LifecycleError> {
        let skipped = IndexOutcome {
            indexed: 0,
            skipped: true,
        };
        // N104: a model that is still loading (or downloading) is not waited for past `model_wait`; the load keeps
        // going and the backfill embeds this meeting once it is ready.
        let e = match tokio::time::timeout(self.model_wait, self.embedder.get()).await {
            Ok(Some(e)) => e,
            Ok(None) => return Ok(skipped),
            Err(_) => {
                tracing::info!(
                    id = %id,
                    "index: search model still loading; skipped, the backfill embeds this meeting when it is ready"
                );
                return Ok(skipped);
            }
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
