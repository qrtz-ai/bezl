//! The summary and resummarize [`StepExecutor`] (port of the worker's `note.stepSummary` / `note.resummarize`
//! and `summarizeMeeting`, apps/desktop/src/worker/handlers.ts + packages/core/src/note/llm.ts). Finalize and index
//! are delegated to an inner executor (`CoreSteps`, later qrtz-search).
//!
//! On-device only for now ([`LlmSteps::on_device`]), which is what TS does with no usable provider: the strict
//! step and resummarize save the extractive summary, and [`SummaryRequest::Fallback`] (the pipeline's last word
//! after the LLM retries ran out) saves it with the LLM error. The OpenRouter client plugs in behind the same type.
//! DB errors keep the storage classification: a full disk or a busy DB is retryable (N13, 913c7fc).
use std::sync::Arc;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::{
    lifecycle_error, DbExec, FinalizeOutcome, IndexOutcome, StepExecutor, SummaryRequest,
};
use qrtz_model::{LifecycleError, MeetingNote, MeetingStatus, MeetingSummary, PipelineContext, Segment};
use qrtz_storage::Db;

use crate::extractive::extractive_summary;
use crate::js::cut_utf16;

pub struct LlmSteps<E: DbExec> {
    pub db: E,
    /// Finalize and index.
    pub inner: Arc<dyn StepExecutor>,
    /// Wall clock for `summaries.created_at`.
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl<E: DbExec> LlmSteps<E> {
    /// Summaries from the on-device extractive summarizer only (no provider, no keys, no network).
    pub fn on_device(db: E, inner: Arc<dyn StepExecutor>, now: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        LlmSteps { db, inner, now }
    }

    async fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> qrtz_storage::Result<R> + Send + 'static,
    ) -> Result<R, LifecycleError> {
        self.db.run(f).await.map_err(lifecycle_error)
    }

    async fn load(&self, id: String, set_template: Option<String>) -> Result<Loaded, LifecycleError> {
        self.run(move |db| load_sync(db, &id, set_template.as_deref()))
            .await
    }

    async fn save(&self, id: &str, s: &MeetingSummary, done: bool) -> Result<(), LifecycleError> {
        let (id, s, now) = (id.to_string(), s.clone(), (self.now)());
        self.run(move |db| {
            db.notes().save_summary(&id, &s, now)?;
            if done {
                db.notes().set_status(&id, MeetingStatus::Done)?;
            }
            Ok(())
        })
        .await
    }
}

/// What the summary is built from.
pub struct Loaded {
    pub id: String,
    pub title: String,
    pub segments: Vec<Segment>,
    pub notes: Vec<MeetingNote>,
    pub template: Option<String>,
}

impl Loaded {
    pub fn extractive(&self) -> MeetingSummary {
        extractive_summary(&self.segments, None, &self.notes, self.template.as_deref())
    }
}

/// `need(id)`, its segments, notes and template. With `set_template`, the template is stored first
/// (`note.resummarize {template}`).
pub fn load_sync(db: &Db, id: &str, set_template: Option<&str>) -> qrtz_storage::Result<Loaded> {
    let repo = db.notes();
    let mut m = repo.need(id)?;
    if let Some(t) = set_template.filter(|t| !t.is_empty()) {
        repo.set_template(id, t)?;
        m.template = Some(t.to_string());
    }
    Ok(Loaded {
        id: id.to_string(),
        title: m.title,
        segments: repo.get_segments(id)?,
        notes: repo.get_notes(id)?,
        template: m.template.filter(|t| !t.is_empty()),
    })
}

#[async_trait]
impl<E: DbExec> StepExecutor for LlmSteps<E> {
    async fn finalize(
        &self,
        meeting_id: String,
        ctx: PipelineContext,
    ) -> Result<FinalizeOutcome, LifecycleError> {
        self.inner.finalize(meeting_id, ctx).await
    }

    /// `note.stepSummary`: Skip marks the meeting done without a summary; Fallback saves the on-device summary
    /// with the LLM error; Llm is the strict attempt (on-device here). A settled summary marks the meeting done.
    async fn summary(
        &self,
        meeting_id: String,
        req: SummaryRequest,
    ) -> Result<Option<MeetingSummary>, LifecycleError> {
        if req == SummaryRequest::Skip {
            let id = meeting_id.clone();
            return self
                .run(move |db| {
                    db.notes().need(&id)?;
                    db.notes().set_status(&id, MeetingStatus::Done)?;
                    Ok(None)
                })
                .await;
        }
        let m = self.load(meeting_id.clone(), None).await?;
        let mut summary = m.extractive();
        if let SummaryRequest::Fallback { error } = req {
            summary.llm_error = Some(cut_utf16(&error, 500).to_string());
        }
        self.save(&meeting_id, &summary, true).await?;
        Ok(Some(summary))
    }

    async fn index(&self, meeting_id: String) -> Result<IndexOutcome, LifecycleError> {
        self.inner.index(meeting_id).await
    }

    /// `note.resummarize`: the summary is replaced (status unchanged).
    async fn resummarize(
        &self,
        meeting_id: String,
        template: Option<String>,
    ) -> Result<MeetingSummary, LifecycleError> {
        let m = self.load(meeting_id.clone(), template).await?;
        let summary = m.extractive();
        self.save(&meeting_id, &summary, false).await?;
        Ok(summary)
    }
}
