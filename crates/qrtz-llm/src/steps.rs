//! The summary and resummarize [`StepExecutor`] (port of the worker's `note.stepSummary` / `note.resummarize`
//! and `summarizeMeeting`, apps/desktop/src/worker/handlers.ts + packages/core/src/note/llm.ts). Finalize and index
//! are delegated to an inner executor (`CoreSteps`, later qrtz-search).
//!
//! - **No provider** ([`LlmSteps::on_device`], `none`, local-only, no key): the extractive summary is the summary,
//!   as in TS (N53: a meeting always ends with one).
//! - **Deadline (N1, ADR 0005):** one LLM attempt is bounded by [`SUMMARY_DEADLINE_MS`] (110 s, inside the
//!   runtime's 120 s RPC timeout). Past it the request is dropped (cancelled), nothing is saved, and the call fails
//!   with a retryable E_TIMEOUT, so a reply that lands late can never overwrite a later attempt or the fallback. The
//!   deadline is a monotonic tokio timer: a wall-clock jump cannot stretch or cut it (like the runtime's backoff,
//!   N12/3cb2b86).
//! - **Strict vs not:** the pipeline step is strict (provider failures surface with their `retryable`, the runtime
//!   retries with backoff, then sends [`SummaryRequest::Fallback`], which saves the extractive summary with the
//!   LLM error); resummarize is not (a provider failure gives the extractive summary with `llmError`).
//! - **DB errors** keep the storage classification: a full disk or a busy DB is retryable (N13, 913c7fc).
//! - **Capture notes (N29, N34, N39):** the prompt lists the recording's silent and lost stretches from
//!   `PipelineState.capture` (`captureOf`).
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::{
    lifecycle_error, DbExec, FinalizeOutcome, IndexOutcome, StepExecutor, SummaryRequest,
};
use qrtz_model::{LifecycleError, MeetingNote, MeetingStatus, MeetingSummary, PipelineContext, Segment};
use qrtz_storage::Db;

use crate::extractive::extractive_summary;
use crate::js::{cut_utf16, js_trim};
use crate::prompt::{build_summary_prompt, parse_summary_reply, SummaryOptions};
use crate::provider::{create_provider, LlmProvider, ProviderId};
use crate::secrets::SecretSource;
use crate::templates::template_by_id;

/// Worker-side deadline for one LLM summary attempt (TS `SUMMARY_DEADLINE_MS`).
pub const SUMMARY_DEADLINE_MS: u64 = 110_000;

/// The LLM settings (`note.llmProvider`, `note.llmModel`, `privacy.localOnly`, `QRTZ_LLM_BASE_URL`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmConfig {
    /// `openrouter` (default), `openai`, `anthropic`, `google`, `ollama`, `nvidia` or `none`.
    pub provider: String,
    /// Empty: the provider's default (OpenRouter: [`crate::OPENROUTER_DEFAULT_MODEL`]).
    pub model: String,
    pub base_url: Option<String>,
    pub local_only: bool,
}

impl Default for LlmConfig {
    fn default() -> Self {
        LlmConfig {
            provider: "openrouter".into(),
            model: String::new(),
            base_url: None,
            local_only: false,
        }
    }
}

impl LlmConfig {
    /// On-device only.
    pub fn none() -> Self {
        LlmConfig {
            provider: "none".into(),
            ..Default::default()
        }
    }
    /// `effectiveLlmProvider`: local-only replaces cloud providers by the on-device path (keys are never read).
    pub fn effective_provider(&self) -> &str {
        if self.local_only && !matches!(self.provider.as_str(), "none" | "ollama") {
            "none"
        } else {
            &self.provider
        }
    }
}

type ConfigFn = Arc<dyn Fn() -> LlmConfig + Send + Sync>;

pub struct LlmSteps<E: DbExec> {
    pub db: E,
    /// Finalize and index.
    pub inner: Arc<dyn StepExecutor>,
    /// Wall clock for `summaries.created_at`.
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Read per attempt, so a settings change applies to the next attempt.
    pub config: ConfigFn,
    pub secrets: Option<Arc<dyn SecretSource>>,
    pub client: reqwest::Client,
    pub deadline: Duration,
}

impl<E: DbExec> LlmSteps<E> {
    /// Summaries from the on-device extractive summarizer only (no provider, no keys, no network).
    pub fn on_device(db: E, inner: Arc<dyn StepExecutor>, now: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        LlmSteps {
            db,
            inner,
            now,
            config: Arc::new(LlmConfig::none),
            secrets: None,
            client: reqwest::Client::new(),
            deadline: Duration::from_millis(SUMMARY_DEADLINE_MS),
        }
    }

    /// With the configured provider (OpenRouter by default), keys from `secrets` (the app's 0600 store).
    pub fn new(
        db: E,
        inner: Arc<dyn StepExecutor>,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
        config: ConfigFn,
        secrets: Arc<dyn SecretSource>,
    ) -> Self {
        LlmSteps {
            config,
            secrets: Some(secrets),
            ..Self::on_device(db, inner, now)
        }
    }

    /// The configured provider (`llmParams` + `createProvider`); `None` = on-device only.
    pub fn provider(&self) -> Option<Box<dyn LlmProvider>> {
        let c = (self.config)();
        let id = c.effective_provider();
        let secret = ProviderId::parse(id)?.defaults().secret;
        let key = secret.and_then(|n| self.secrets.as_ref()?.get(n));
        create_provider(&self.client, id, key, Some(&c.model), c.base_url.as_deref())
    }

    /// `summarizeMeeting(..., {strict, signal})` under the deadline: E_TIMEOUT past it, nothing returned.
    async fn summarize(
        &self,
        m: &Loaded,
        strict: bool,
        what: &str,
    ) -> Result<MeetingSummary, LifecycleError> {
        let template = template_by_id(m.opts.template.as_deref()).id;
        let has_notes = m.opts.notes.iter().any(|n| !js_trim(&n.text).is_empty());
        let Some(p) = self.provider().filter(|_| !m.segments.is_empty() || has_notes) else {
            return Ok(m.extractive().await);
        };
        let msgs = build_summary_prompt(&m.title, &m.segments, &m.opts);
        let attempt = async { parse_summary_reply(&p.complete(&msgs, true).await?) };
        // Late-reply discard: the request future is dropped with the timer, so no reply can land after this.
        let Ok(r) = tokio::time::timeout(self.deadline, attempt).await else {
            tracing::warn!(id = %m.id, "{what}: LLM summary deadline exceeded");
            return Err(LifecycleError {
                code: "E_TIMEOUT".into(),
                message: "LLM summary deadline exceeded".into(),
                retryable: true,
            });
        };
        match r {
            Ok(reply) => Ok(reply.into_summary(p.id(), template)),
            Err(e) => {
                tracing::warn!(id = %m.id, code = %e.code, err = %e.message, "{what}");
                if strict {
                    return Err(e);
                }
                let mut s = m.extractive().await;
                s.llm_error = Some(cut_utf16(&e.message, 500).to_string());
                Ok(s)
            }
        }
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
    /// Notes, template and the capture record.
    pub opts: SummaryOptions,
}

impl Loaded {
    pub fn extractive_sync(&self) -> MeetingSummary {
        extractive_summary(
            &self.segments,
            None,
            &self.opts.notes,
            self.opts.template.as_deref(),
        )
    }
    /// On a blocking thread, not the runtime's worker (N54: linear and capped, but a 3 h meeting is still work).
    pub async fn extractive(&self) -> MeetingSummary {
        let (segs, notes, tpl) = (
            self.segments.clone(),
            self.opts.notes.clone(),
            self.opts.template.clone(),
        );
        tokio::task::spawn_blocking(move || extractive_summary(&segs, None, &notes, tpl.as_deref()))
            .await
            .expect("extractive summary panicked")
    }
}

/// `need(id)`, its segments, notes, template and capture record (`captureOf`). With `set_template`, the template is stored first
/// (`note.resummarize {template}`).
pub fn load_sync(db: &Db, id: &str, set_template: Option<&str>) -> qrtz_storage::Result<Loaded> {
    let repo = db.notes();
    let mut m = repo.need(id)?;
    if let Some(t) = set_template.filter(|t| !t.is_empty()) {
        repo.set_template(id, t)?;
        m.template = Some(t.to_string());
    }
    let notes: Vec<MeetingNote> = repo.get_notes(id)?;
    let mut opts = SummaryOptions {
        notes,
        template: m.template.clone().filter(|t| !t.is_empty()),
        ..Default::default()
    };
    // `captureOf`: an unreadable pipeline row is no capture record.
    if let Ok(Some(p)) = db.lifecycle().get_pipeline(id) {
        if let Some(c) = p.capture {
            if !c.silent_spans.is_empty() || !c.lost_spans.is_empty() {
                let d = m.duration_ms.filter(|d| *d != 0).unwrap_or(p.ctx.duration_ms);
                opts.duration_ms = (d != 0).then_some(d);
                opts.silent_spans = c.silent_spans;
                opts.lost_spans = c.lost_spans;
            }
        }
    }
    Ok(Loaded {
        id: id.to_string(),
        title: m.title,
        segments: repo.get_segments(id)?,
        opts,
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
    /// with the LLM error; Llm is the strict attempt under the deadline. A settled summary marks the meeting done.
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
        let summary = match req {
            SummaryRequest::Fallback { error } => {
                let mut s = m.extractive().await;
                s.llm_error = Some(cut_utf16(&error, 500).to_string());
                s
            }
            _ => self.summarize(&m, true, "llm attempt failed").await?,
        };
        self.save(&meeting_id, &summary, true).await?;
        Ok(Some(summary))
    }

    async fn index(&self, meeting_id: String) -> Result<IndexOutcome, LifecycleError> {
        self.inner.index(meeting_id).await
    }

    /// `note.resummarize`: not strict, same deadline; past it nothing is saved and the old summary stays. The
    /// summary is replaced (status unchanged).
    async fn resummarize(
        &self,
        meeting_id: String,
        template: Option<String>,
    ) -> Result<MeetingSummary, LifecycleError> {
        let m = self.load(meeting_id.clone(), template).await?;
        let summary = self.summarize(&m, false, "llm retry failed").await?;
        self.save(&meeting_id, &summary, false).await?;
        Ok(summary)
    }
}
