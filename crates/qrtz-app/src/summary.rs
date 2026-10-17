//! Meeting summaries, on this Mac only (ADR 0075): Apple Intelligence (FoundationModels) through the helper,
//! chunked map-reduce for the model's 4,096-token context, behind the pipeline's summary step.
//!
//! - **Offline**: qrtz has no cloud summary engine. [`SummarySteps`] never makes a network request; when the model
//!   is unavailable the step fails with an error that says what to do, and after the attempts the meeting keeps
//!   the extractive summary with that error as `llmError` (shown with a retry button).
//! - **What reaches the helper**: one N69-fenced prompt per request, built from the same cleaned pieces as the old
//!   provider prompt (`prompt_safe` lines, `fence_block`). The helper's instructions are fixed per stage; it has no
//!   network, file or fd authority for this request beyond its stdio line.
//! - **What comes back**: guided-generation data only (`summarize.result`), schema-checked by the helper contract
//!   and re-capped here ([`clean_reply`]) before anything is stored or shown.
use std::sync::Arc;
use std::time::Duration;
// tokio's clock (the same monotonic clock in the app) so the deadline is testable in paused time.
use tokio::time::Instant;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::{
    lifecycle_error, DbExec, FinalizeOutcome, IndexOutcome, StepExecutor, SummaryRequest,
};
use qrtz_llm::js::{cut_utf16, js_trim, u16len};
use qrtz_llm::prompt::{
    capture_gap_guidance, capture_gaps_for_prompt, fence_block, fence_nonce, format_notes_for_prompt,
    format_transcript_for_prompt, parse_summary_reply, prompt_safe, SummaryReply,
};
use qrtz_llm::steps::{load_sync, Loaded};
use qrtz_llm::template_by_id;
use qrtz_model::{
    ActionItem, LifecycleError, MeetingNote, MeetingStatus, MeetingSummary, PipelineContext, QaItem,
};
use serde_json::{json, Value};

/// `MeetingSummary.provider` of an on-device summary.
pub const PROVIDER_ID: &str = "apple-intelligence";
/// The runtime's summary RPC timeout while the engine is on-device (a long meeting is many model calls).
pub const ON_DEVICE_SUMMARY_TIMEOUT_MS: u64 = 900_000;
/// The engine's own budget for one meeting, inside [`ON_DEVICE_SUMMARY_TIMEOUT_MS`].
pub const ON_DEVICE_DEADLINE: Duration = Duration::from_secs(840);
/// One model call (a 1,800-word part took 26–34 s on an M1 Pro; ADR 0075 §measurements).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(180);
/// Transcript text per part, in UTF-16 units (~2,000 tokens: leaves room for instructions, schema and output in
/// the 4,096-token window). A part the model still rejects is split in half.
pub const PART_CHARS: usize = 8_000;
const MAX_SPLITS: u32 = 3;
const NOTES_CHARS: usize = 2_000;

/// Caps on the model's output (contracts/helper-protocol.json `summarize.result`, `SUMMARIZE_LIMITS`).
pub mod limits {
    pub const PROMPT: usize = 40_000;
    pub const SUMMARY: usize = 4_000;
    pub const ACTION_ITEMS: usize = 12;
    pub const ITEM_TEXT: usize = 400;
    pub const NAME: usize = 80;
    pub const QA: usize = 8;
    pub const QUESTION: usize = 300;
    pub const ANSWER: usize = 600;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The whole meeting fits one prompt.
    Single,
    /// One part of a long meeting.
    Map,
    /// Merge the parts' summaries.
    Reduce,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Single => "single",
            Stage::Map => "map",
            Stage::Reduce => "reduce",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    AppleIntelligenceOff,
    DeviceNotEligible,
    ModelNotReady,
    OsUnsupported,
    /// Not a Mac, or a helper built without FoundationModels.
    Unsupported,
}

impl Unavailable {
    pub fn parse(s: &str) -> Unavailable {
        match s {
            "apple-intelligence-off" => Unavailable::AppleIntelligenceOff,
            "device-not-eligible" => Unavailable::DeviceNotEligible,
            "model-not-ready" => Unavailable::ModelNotReady,
            "os-unsupported" => Unavailable::OsUnsupported,
            _ => Unavailable::Unsupported,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnDeviceError {
    Unavailable(Unavailable),
    /// The prompt is over the model's context window.
    Context,
    /// The model refused (Apple's guardrails).
    Guardrail,
    Busy,
    /// The helper isn't running (starting, restarting, or no helper in this build).
    NoHelper,
    Timeout,
    Failed(String),
}

/// The model behind the engine: the helper in the app, fakes in tests.
#[async_trait]
pub trait OnDeviceModel: Send + Sync {
    async fn summarize(&self, stage: Stage, prompt: String) -> Result<SummaryReply, OnDeviceError>;
}

/// No on-device model in this build (tests and headless boots without a helper).
pub struct NoModel;

#[async_trait]
impl OnDeviceModel for NoModel {
    async fn summarize(&self, _: Stage, _: String) -> Result<SummaryReply, OnDeviceError> {
        Err(OnDeviceError::Unavailable(Unavailable::Unsupported))
    }
}

/// Display text from the model: one line, no control/bidi/tag characters (N69 `prompt_safe`), trimmed, capped.
fn clean(s: &str, max: usize) -> String {
    let one = prompt_safe(s);
    let squeezed = one.split_whitespace().collect::<Vec<_>>().join(" ");
    cut_utf16(&squeezed, max).trim().to_string()
}

fn clean_opt(s: Option<String>) -> Option<String> {
    s.map(|o| clean(&o, limits::NAME)).filter(|o| !o.is_empty())
}

/// The model's reply as stored data: every string cleaned and capped, empty items dropped, counts capped.
pub fn clean_reply(r: SummaryReply) -> SummaryReply {
    SummaryReply {
        summary: clean(&r.summary, limits::SUMMARY),
        action_items: r
            .action_items
            .into_iter()
            .map(|a| ActionItem {
                text: clean(&a.text, limits::ITEM_TEXT),
                owner: clean_opt(a.owner),
            })
            .filter(|a| !a.text.is_empty())
            .take(limits::ACTION_ITEMS)
            .collect(),
        qa: r
            .qa
            .into_iter()
            .map(|q| QaItem {
                question: clean(&q.question, limits::QUESTION),
                answer: clean(&q.answer, limits::ANSWER),
                asked_by: clean_opt(q.asked_by),
            })
            .filter(|q| !q.question.is_empty() && !q.answer.is_empty())
            .take(limits::QA)
            .collect(),
        template: None,
    }
}

/// A `summarize.result` line (already validated against the helper contract) → the reply shape.
pub fn reply_from_result(v: &Value) -> Result<SummaryReply, OnDeviceError> {
    let body = json!({"summary": v["summary"], "actionItems": v["actionItems"], "qa": v["qa"]});
    parse_summary_reply(&body.to_string())
        .map(clean_reply)
        .map_err(|e| OnDeviceError::Failed(e.message))
}

/// A `summarize.error` line → the error.
pub fn error_from_line(v: &Value) -> OnDeviceError {
    match v["code"].as_str().unwrap_or_default() {
        "unavailable" => {
            OnDeviceError::Unavailable(Unavailable::parse(v["reason"].as_str().unwrap_or_default()))
        }
        "context" => OnDeviceError::Context,
        "guardrail" => OnDeviceError::Guardrail,
        "busy" => OnDeviceError::Busy,
        "cancelled" => OnDeviceError::Timeout,
        _ => OnDeviceError::Failed(clean(
            v["message"].as_str().unwrap_or("on-device model failed"),
            200,
        )),
    }
}

/// The error the meeting shows (lowercase UI text, ≤ 500 UTF-16 units like `llmError`). qrtz is offline: no
/// error ever suggests a cloud service.
pub fn to_lifecycle(e: &OnDeviceError) -> LifecycleError {
    let (code, msg, retryable) = match e {
        OnDeviceError::Unavailable(Unavailable::AppleIntelligenceOff) => (
            "E_SUMMARY_UNAVAILABLE",
            "summary unavailable: apple intelligence is turned off. turn it on in system settings › apple intelligence & siri, then retry.".to_string(),
            false,
        ),
        OnDeviceError::Unavailable(Unavailable::DeviceNotEligible) => (
            "E_SUMMARY_UNAVAILABLE",
            "summary unavailable: this mac can't run apple intelligence.".to_string(),
            false,
        ),
        OnDeviceError::Unavailable(Unavailable::ModelNotReady) => (
            "E_SUMMARY_UNAVAILABLE",
            "summary unavailable: apple intelligence is still downloading its model. retry in a few minutes.".to_string(),
            true,
        ),
        OnDeviceError::Unavailable(Unavailable::OsUnsupported) => (
            "E_SUMMARY_UNAVAILABLE",
            "summary unavailable: on-device summaries need macos 26 or later with apple intelligence.".to_string(),
            false,
        ),
        OnDeviceError::Unavailable(Unavailable::Unsupported) => (
            "E_SUMMARY_UNAVAILABLE",
            "summary unavailable: on-device summaries need apple intelligence on a mac.".to_string(),
            false,
        ),
        OnDeviceError::NoHelper => (
            "E_SUMMARY_UNAVAILABLE",
            "summary unavailable: the qrtz helper isn't running. retry in a moment.".to_string(),
            true,
        ),
        OnDeviceError::Context => (
            "E_SUMMARY_FAILED",
            "summary unavailable: part of this meeting is too long for the on-device model.".to_string(),
            false,
        ),
        OnDeviceError::Guardrail => (
            "E_SUMMARY_REFUSED",
            "summary unavailable: apple intelligence declined to summarize this meeting.".to_string(),
            false,
        ),
        OnDeviceError::Busy => (
            "E_SUMMARY_BUSY",
            "summary unavailable: the on-device model is busy. retry in a moment.".to_string(),
            true,
        ),
        OnDeviceError::Timeout => (
            "E_TIMEOUT",
            "summary unavailable: the on-device model took too long. retry.".to_string(),
            true,
        ),
        OnDeviceError::Failed(m) => (
            "E_SUMMARY_FAILED",
            format!("summary unavailable: the on-device model failed ({}). retry.", cut_utf16(m, 120)),
            true,
        ),
    };
    LifecycleError {
        code: code.into(),
        message: cut_utf16(&msg, 500).to_string(),
        retryable,
    }
}

/// The map-reduce engine over an [`OnDeviceModel`].
pub struct OnDeviceSummaries {
    pub model: Arc<dyn OnDeviceModel>,
    pub part_chars: usize,
    pub deadline: Duration,
    /// Tests pin the fence nonce.
    pub nonce: Option<Arc<dyn Fn() -> String + Send + Sync>>,
}

struct Ctx {
    title: String,
    guidance: String,
    notes: String,
    gaps: Vec<String>,
}

impl OnDeviceSummaries {
    pub fn new(model: Arc<dyn OnDeviceModel>) -> Self {
        OnDeviceSummaries {
            model,
            part_chars: PART_CHARS,
            deadline: ON_DEVICE_DEADLINE,
            nonce: None,
        }
    }

    fn nonce(&self) -> String {
        self.nonce.as_ref().map(|f| f()).unwrap_or_else(fence_nonce)
    }

    /// Split the transcript into parts of at most `part_chars` formatted UTF-16 units (whole lines).
    pub fn parts(&self, m: &Loaded) -> Vec<String> {
        let mut parts = Vec::new();
        let (mut cur, mut used) = (Vec::new(), 0usize);
        for seg in &m.segments {
            let line = format_transcript_for_prompt(std::slice::from_ref(seg), self.part_chars);
            let n = u16len(&line) + 1;
            if used + n > self.part_chars && !cur.is_empty() {
                parts.push(std::mem::take(&mut cur).join("\n"));
                used = 0;
            }
            used += n;
            cur.push(line);
        }
        if !cur.is_empty() {
            parts.push(cur.join("\n"));
        }
        parts
    }

    fn prompt(&self, c: &Ctx, stage: Stage, body: &str, part: Option<(usize, usize)>) -> String {
        let nonce = self.nonce();
        let (tag, what) = match stage {
            Stage::Reduce => ("part-summaries", "list of part summaries"),
            _ => ("transcript", "transcript"),
        };
        let (block, rule) = fence_block(tag, body, &nonce, what);
        let mut p = String::new();
        if stage != Stage::Map {
            p.push_str(js_trim(&format!(
                "{}{}",
                c.guidance,
                capture_gap_guidance(&c.gaps)
            )));
        }
        p.push_str(&rule);
        p.push_str(&format!("\n\nMeeting: {}\n\n", c.title));
        if stage != Stage::Map && !c.notes.is_empty() {
            p.push_str(&format!(
                "My notes (the user's priorities; keep their to-dos as action items):\n{}\n\n",
                c.notes
            ));
        }
        if stage != Stage::Map && !c.gaps.is_empty() {
            p.push_str(&format!("Capture gaps:\n{}\n\n", c.gaps.join("\n")));
        }
        match (stage, part) {
            (Stage::Map, Some((i, n))) => p.push_str(&format!("Transcript, part {i} of {n}:\n")),
            (Stage::Reduce, _) => p.push_str("Summaries of consecutive parts of the meeting, in order:\n"),
            _ => p.push_str("Transcript:\n"),
        }
        p.push_str(&block);
        cut_utf16(&p, limits::PROMPT).to_string()
    }

    async fn call(
        &self,
        stage: Stage,
        prompt: String,
        until: Instant,
    ) -> Result<SummaryReply, OnDeviceError> {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(OnDeviceError::Timeout);
        }
        match tokio::time::timeout(left.min(CALL_TIMEOUT), self.model.summarize(stage, prompt)).await {
            Ok(r) => r,
            Err(_) => Err(OnDeviceError::Timeout),
        }
    }

    /// One transcript part; over the context window it is split in half (by lines), up to [`MAX_SPLITS`] times.
    fn map_part<'a>(
        &'a self,
        c: &'a Ctx,
        body: String,
        label: (usize, usize),
        depth: u32,
        until: Instant,
    ) -> futures_lite::BoxedFut<'a, Result<Vec<SummaryReply>, OnDeviceError>> {
        Box::pin(async move {
            let stage = if label.1 == 1 { Stage::Single } else { Stage::Map };
            match self
                .call(stage, self.prompt(c, stage, &body, Some(label)), until)
                .await
            {
                Err(OnDeviceError::Context) if depth < MAX_SPLITS => {
                    let lines: Vec<&str> = body.lines().collect();
                    let (a, b) = if lines.len() >= 2 {
                        let h = lines.len() / 2;
                        (lines[..h].join("\n"), lines[h..].join("\n"))
                    } else {
                        // One very long line: keep both halves of it.
                        let half = u16len(&body) / 2;
                        let head = cut_utf16(&body, half).to_string();
                        let tail = body[head.len()..].to_string();
                        (head, tail)
                    };
                    let n = label.1.max(2);
                    let mut out = self.map_part(c, a, (label.0, n), depth + 1, until).await?;
                    out.extend(self.map_part(c, b, (label.0, n), depth + 1, until).await?);
                    Ok(out)
                }
                Err(e) => Err(e),
                Ok(r) => Ok(vec![r]),
            }
        })
    }

    fn part_notes(replies: &[SummaryReply]) -> Vec<String> {
        replies
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let mut s = format!("Part {}: {}", i + 1, prompt_safe(&r.summary));
                if !r.action_items.is_empty() {
                    let items: Vec<String> = r
                        .action_items
                        .iter()
                        .map(|a| match &a.owner {
                            Some(o) => format!("{} ({})", prompt_safe(&a.text), prompt_safe(o)),
                            None => prompt_safe(&a.text),
                        })
                        .collect();
                    s.push_str(&format!(" Action items: {}.", items.join("; ")));
                }
                if !r.qa.is_empty() {
                    let qa: Vec<String> =
                        r.qa.iter()
                            .map(|q| format!("{} {}", prompt_safe(&q.question), prompt_safe(&q.answer)))
                            .collect();
                    s.push_str(&format!(" Questions: {}.", qa.join(" ")));
                }
                s
            })
            .collect()
    }

    /// Merge part summaries; more than one prompt's worth is merged in groups first.
    async fn reduce(
        &self,
        c: &Ctx,
        replies: Vec<SummaryReply>,
        until: Instant,
    ) -> Result<SummaryReply, OnDeviceError> {
        let mut replies = replies;
        loop {
            let notes = Self::part_notes(&replies);
            let total: usize = notes.iter().map(|n| u16len(n) + 1).sum();
            if total <= self.part_chars || replies.len() <= 2 {
                let body = notes.join("\n");
                return self
                    .call(Stage::Reduce, self.prompt(c, Stage::Reduce, &body, None), until)
                    .await;
            }
            // Group to about one prompt each, keep order.
            let mut next = Vec::new();
            let (mut group, mut used) = (Vec::new(), 0usize);
            for (r, n) in replies.into_iter().zip(notes) {
                let len = u16len(&n) + 1;
                if used + len > self.part_chars && group.len() >= 2 {
                    next.push(self.reduce_group(c, std::mem::take(&mut group), until).await?);
                    used = 0;
                }
                used += len;
                group.push(r);
            }
            if group.len() == 1 && !next.is_empty() {
                next.push(group.pop().expect("one"));
            } else if !group.is_empty() {
                next.push(self.reduce_group(c, group, until).await?);
            }
            replies = next;
        }
    }

    async fn reduce_group(
        &self,
        c: &Ctx,
        g: Vec<SummaryReply>,
        until: Instant,
    ) -> Result<SummaryReply, OnDeviceError> {
        let body = Self::part_notes(&g).join("\n");
        self.call(Stage::Reduce, self.prompt(c, Stage::Reduce, &body, None), until)
            .await
    }

    /// The whole meeting → a stored-shape summary (strict: errors are returned, never replaced by a cloud call).
    pub async fn summarize(&self, m: &Loaded) -> Result<MeetingSummary, OnDeviceError> {
        let until = Instant::now() + self.deadline;
        let tpl = template_by_id(m.opts.template.as_deref());
        let notes: Vec<MeetingNote> = m
            .opts
            .notes
            .iter()
            .filter(|n| !js_trim(&n.text).is_empty())
            .map(|n| MeetingNote {
                text: prompt_safe(&n.text),
                ..n.clone()
            })
            .collect();
        let c = Ctx {
            title: prompt_safe(&m.title),
            guidance: tpl.guidance.to_string(),
            notes: cut_utf16(&format_notes_for_prompt(&notes), NOTES_CHARS).to_string(),
            gaps: capture_gaps_for_prompt(&m.opts.silent_spans, m.opts.duration_ms, &m.opts.lost_spans),
        };
        let parts = self.parts(m);
        let reply = if parts.is_empty() {
            // Notes only: one call over the notes.
            self.call(Stage::Single, self.prompt(&c, Stage::Single, "", None), until)
                .await?
        } else {
            let n = parts.len();
            let mut replies = Vec::new();
            for (i, p) in parts.into_iter().enumerate() {
                replies.extend(self.map_part(&c, p, (i + 1, n), 0, until).await?);
            }
            if replies.len() == 1 {
                replies.pop().expect("one")
            } else {
                self.reduce(&c, replies, until).await?
            }
        };
        let r = clean_reply(reply);
        if r.summary.is_empty() {
            return Err(OnDeviceError::Failed("empty summary".into()));
        }
        Ok(r.into_summary(PROVIDER_ID, tpl.id))
    }
}

/// Small boxed-future alias (no extra dependency).
mod futures_lite {
    pub type BoxedFut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
}

/// The summary [`StepExecutor`] (ADR 0075): every summary is written on this Mac by Apple Intelligence through
/// the helper. There is no cloud engine and no network path: an unavailable model is an error with retry, and
/// after the attempts the meeting keeps the extractive summary with that error as `llmError`. Finalize and index
/// pass through to `inner` (`CoreSteps`).
pub struct SummarySteps<E: DbExec> {
    pub db: E,
    pub inner: Arc<dyn StepExecutor>,
    /// Wall clock for `summaries.created_at`.
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub on_device: Arc<OnDeviceSummaries>,
}

impl<E: DbExec> SummarySteps<E> {
    async fn load(&self, id: String, set_template: Option<String>) -> Result<Loaded, LifecycleError> {
        self.db
            .run(move |db| load_sync(db, &id, set_template.as_deref()))
            .await
            .map_err(lifecycle_error)
    }

    async fn save(&self, id: &str, s: &MeetingSummary, done: bool) -> Result<(), LifecycleError> {
        let (id, s, now) = (id.to_string(), s.clone(), (self.now)());
        self.db
            .run(move |db| {
                db.notes().save_summary(&id, &s, now)?;
                if done {
                    db.notes().set_status(&id, MeetingStatus::Done)?;
                }
                Ok(())
            })
            .await
            .map_err(lifecycle_error)
    }

    async fn on_device(&self, m: &Loaded) -> Result<MeetingSummary, LifecycleError> {
        let has_notes = m.opts.notes.iter().any(|n| !js_trim(&n.text).is_empty());
        if m.segments.is_empty() && !has_notes {
            return Ok(m.extractive().await);
        }
        self.on_device.summarize(m).await.map_err(|e| {
            tracing::warn!(id = %m.id, err = ?e, "on-device summary failed");
            to_lifecycle(&e)
        })
    }
}

#[async_trait]
impl<E: DbExec> StepExecutor for SummarySteps<E> {
    async fn finalize(&self, id: String, ctx: PipelineContext) -> Result<FinalizeOutcome, LifecycleError> {
        self.inner.finalize(id, ctx).await
    }

    async fn summary(
        &self,
        id: String,
        req: SummaryRequest,
    ) -> Result<Option<MeetingSummary>, LifecycleError> {
        if req == SummaryRequest::Skip {
            let i = id.clone();
            return self
                .db
                .run(move |db| {
                    db.notes().need(&i)?;
                    db.notes().set_status(&i, MeetingStatus::Done)?;
                    Ok(None)
                })
                .await
                .map_err(lifecycle_error);
        }
        let m = self.load(id.clone(), None).await?;
        let s = match req {
            // Attempts exhausted: the extractive summary (from the transcript, no model) with the error to show.
            SummaryRequest::Fallback { error } => {
                let mut s = m.extractive().await;
                s.llm_error = Some(cut_utf16(&error, 500).to_string());
                s
            }
            _ => self.on_device(&m).await?,
        };
        self.save(&id, &s, true).await?;
        Ok(Some(s))
    }

    async fn index(&self, id: String) -> Result<IndexOutcome, LifecycleError> {
        self.inner.index(id).await
    }

    /// Not strict: a failure gives the extractive summary with the error (shown with retry).
    async fn resummarize(
        &self,
        id: String,
        template: Option<String>,
    ) -> Result<MeetingSummary, LifecycleError> {
        let m = self.load(id.clone(), template).await?;
        let s = match self.on_device(&m).await {
            Ok(s) => s,
            Err(e) => {
                let mut s = m.extractive().await;
                s.llm_error = Some(e.message);
                s
            }
        };
        self.save(&id, &s, false).await?;
        Ok(s)
    }
}

/// [`OnDeviceModel`] over the helper: `summarize` → `summarize.result` / `summarize.error` with the same id.
/// One request at a time (one model session; the helper also queues).
pub struct HelperModel {
    sup: Option<qrtz_helper_bridge::Supervisor>,
    gate: tokio::sync::Mutex<()>,
}

impl HelperModel {
    pub fn new(sup: Option<qrtz_helper_bridge::Supervisor>) -> Self {
        HelperModel {
            sup,
            gate: tokio::sync::Mutex::new(()),
        }
    }
}

/// Sends `summarize.cancel` if the request is dropped before its answer (deadline, runtime timeout).
struct CancelOnDrop<'a> {
    sup: &'a qrtz_helper_bridge::Supervisor,
    id: String,
    done: bool,
}

impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        if !self.done {
            if let Ok(l) =
                qrtz_helper_bridge::encode_host_message(&json!({"type": "summarize.cancel", "id": self.id}))
            {
                let _ = self.sup.try_send(l);
            }
        }
    }
}

#[async_trait]
impl OnDeviceModel for HelperModel {
    async fn summarize(&self, stage: Stage, prompt: String) -> Result<SummaryReply, OnDeviceError> {
        use qrtz_helper_bridge::HelperState;
        let Some(sup) = &self.sup else {
            return Err(OnDeviceError::Unavailable(Unavailable::Unsupported));
        };
        let _g = self.gate.lock().await;
        if sup.status().state != HelperState::Running {
            return Err(OnDeviceError::NoHelper);
        }
        let id = crate::wiring::uuid_v4();
        let line = qrtz_helper_bridge::encode_host_message(
            &json!({"type": "summarize", "id": id, "stage": stage.as_str(), "prompt": prompt}),
        )
        .map_err(|e| OnDeviceError::Failed(cut_utf16(&e, 120).to_string()))?;
        let mut lines = sup.subscribe_lines();
        let mut status = sup.watch_status();
        if !sup.send(line).await {
            return Err(OnDeviceError::NoHelper);
        }
        let mut guard = CancelOnDrop {
            sup,
            id: id.clone(),
            done: false,
        };
        loop {
            tokio::select! {
                l = lines.recv() => match l {
                    Ok(l) => {
                        let Ok(v) = qrtz_helper_bridge::parse_helper_line(&l) else { continue };
                        // A helper older than ADR 0075 logs the unknown line instead of answering.
                        if v["type"] == "log"
                            && v["msg"].as_str().is_some_and(|m| m.contains("unknown host message type summarize"))
                        {
                            guard.done = true;
                            return Err(OnDeviceError::Unavailable(Unavailable::Unsupported));
                        }
                        if v["id"].as_str() != Some(id.as_str()) {
                            continue;
                        }
                        match v["type"].as_str() {
                            Some("summarize.result") => {
                                guard.done = true;
                                return reply_from_result(&v);
                            }
                            Some("summarize.error") => {
                                guard.done = true;
                                return Err(error_from_line(&v));
                            }
                            _ => {}
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return Err(OnDeviceError::NoHelper),
                },
                c = status.changed() => {
                    if c.is_err() || status.borrow().state != HelperState::Running {
                        guard.done = true;
                        return Err(OnDeviceError::NoHelper);
                    }
                }
            }
        }
    }
}
