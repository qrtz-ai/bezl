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
//! - **Robustness (review §12.23)**: a request waits up to [`HELPER_WAIT`] for the helper and is resent once
//!   after a restart (N113); one meeting holds the model at a time, before its deadline starts (N115); an answer
//!   for our id that fails the contract fails at once (N116), and other lines are skipped unparsed (N119); the
//!   deadline is sized from the part count and unfinished meetings keep their part summaries in memory so a retry
//!   resumes (N114); labels are renumbered after a context split (N117).
use std::collections::{HashMap, VecDeque};
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
/// The engine's least budget for one meeting (a short meeting, or the notes alone).
pub const ON_DEVICE_DEADLINE: Duration = Duration::from_secs(840);
/// The most the engine gives one meeting in one attempt (N114). A longer meeting that runs out resumes from its
/// cached part summaries on the next attempt.
pub const MAX_ON_DEVICE_DEADLINE: Duration = Duration::from_secs(3_600);
/// Budget per expected model call when sizing a meeting's deadline: twice the measured ~30 s per part (ADR 0075).
pub const PER_CALL_BUDGET: Duration = Duration::from_secs(60);
/// The runtime's summary RPC timeout while the engine is on-device: the largest meeting deadline plus a minute for
/// the load, the save and the model lock (N114).
pub const ON_DEVICE_SUMMARY_TIMEOUT_MS: u64 = MAX_ON_DEVICE_DEADLINE.as_millis() as u64 + 60_000;
/// One model call (a 1,800-word part took 26–34 s on an M1 Pro; ADR 0075 §measurements).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(180);
/// How long a request waits for the helper to be running, before sending and after a restart mid-request (N113).
pub const HELPER_WAIT: Duration = Duration::from_secs(30);
/// Meetings whose part summaries are kept for a retry (N114); the oldest is dropped past this.
const CACHED_MEETINGS: usize = 4;
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
    /// An answer for our request that fails the helper contract (N116).
    InvalidReply,
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
        OnDeviceError::InvalidReply => (
            "E_SUMMARY_FAILED",
            "summary unavailable: invalid reply from the on-device model. retry.".to_string(),
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
    /// The least budget for one meeting; the real one is sized from its parts ([`Self::deadline_for`]).
    pub deadline: Duration,
    /// The most budget for one meeting in one attempt.
    pub max_deadline: Duration,
    /// Tests pin the fence nonce.
    pub nonce: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    /// One meeting at a time on the model (N115), taken before the meeting's deadline and any call timer starts.
    turn: tokio::sync::Mutex<()>,
    /// Map results of meetings that didn't finish, so a retry resumes (N114).
    cache: std::sync::Mutex<PartCache>,
    /// N123: meetings whose delete is pending (tombstoned, undo window open). Their part summaries are never
    /// read or written. `None`: deletes run at once.
    pending_delete: Option<PendingDelete>,
}

/// Is this meeting id pending delete? (The app wires the runtime's tombstone store.)
pub type PendingDelete = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// A transcript piece already handled in an unfinished attempt.
#[derive(Clone)]
enum Piece {
    Done(SummaryReply),
    /// Over the context window: go straight to its halves.
    Split,
}

/// Per meeting id: piece key → [`Piece`]. In memory only, bounded to [`CACHED_MEETINGS`], dropped on success
/// and on delete (N123).
#[derive(Default)]
struct PartCache {
    meetings: VecDeque<(String, HashMap<u64, Piece>)>,
    /// Per meeting id with an attempt in flight: (attempts in flight, deletes since the oldest of them began).
    /// An attempt that started before a delete never writes the deleted meeting's pieces back (N123). The entry
    /// goes with the last attempt (N128): a delete with no attempt in flight has nothing to fence off, and an
    /// attempt that starts after it can't see what it dropped.
    deletes: HashMap<String, (usize, u64)>,
}

impl PartCache {
    /// An attempt for `id` begins: the delete count to compare its writes against.
    fn begin(&mut self, id: &str) -> u64 {
        let e = self.deletes.entry(id.to_string()).or_default();
        e.0 += 1;
        e.1
    }
    /// An attempt for `id` ended (finished, failed or dropped); the last one takes the entry with it.
    fn end(&mut self, id: &str) {
        if let Some(e) = self.deletes.get_mut(id) {
            e.0 = e.0.saturating_sub(1);
            if e.0 == 0 {
                self.deletes.remove(id);
            }
        }
    }
    fn deletes(&self, id: &str) -> u64 {
        self.deletes.get(id).map_or(0, |e| e.1)
    }
    fn get(&self, id: &str, key: u64) -> Option<Piece> {
        self.meetings
            .iter()
            .find(|(m, _)| m == id)
            .and_then(|(_, h)| h.get(&key).cloned())
    }
    /// Stores the piece unless the meeting was deleted since the attempt began (`deletes` then).
    fn put(&mut self, id: &str, deletes: u64, key: u64, p: Piece) {
        if self.deletes(id) != deletes {
            return;
        }
        if let Some((_, h)) = self.meetings.iter_mut().find(|(m, _)| m == id) {
            h.insert(key, p);
            return;
        }
        if self.meetings.len() >= CACHED_MEETINGS {
            self.meetings.pop_front();
        }
        self.meetings
            .push_back((id.to_string(), HashMap::from([(key, p)])));
    }
    fn forget(&mut self, id: &str) {
        self.meetings.retain(|(m, _)| m != id);
    }
    fn deleted(&mut self, id: &str) {
        self.forget(id);
        // Only attempts in flight need fencing (N128: nothing is kept for a meeting with none).
        if let Some(e) = self.deletes.get_mut(id) {
            e.1 += 1;
        }
    }
}

/// One map attempt's hold on its meeting's delete count; dropping it (also when the attempt's future is dropped
/// by a timeout) ends the attempt.
struct Attempt<'a> {
    cache: &'a std::sync::Mutex<PartCache>,
    id: &'a str,
    deletes: u64,
}

impl<'a> Attempt<'a> {
    fn begin(cache: &'a std::sync::Mutex<PartCache>, id: &'a str) -> Self {
        let deletes = cache.lock().unwrap_or_else(crate::poison::recover).begin(id);
        Attempt { cache, id, deletes }
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        self.cache
            .lock()
            .unwrap_or_else(crate::poison::recover)
            .end(self.id);
    }
}

/// A map piece's identity: the meeting title and the piece's transcript lines (all a map prompt is built from
/// besides its label and nonce), so an edited transcript or title is summarized again.
fn piece_key(title: &str, body: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (title, body).hash(&mut h);
    h.finish()
}

/// A piece over the context window, cut in half by lines (one very long line: by characters).
fn halves(body: &str) -> (String, String) {
    let lines: Vec<&str> = body.lines().collect();
    if lines.len() >= 2 {
        let h = lines.len() / 2;
        (lines[..h].join("\n"), lines[h..].join("\n"))
    } else {
        let half = u16len(body) / 2;
        let head = cut_utf16(body, half).to_string();
        let tail = body[head.len()..].to_string();
        (head, tail)
    }
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
            max_deadline: MAX_ON_DEVICE_DEADLINE,
            nonce: None,
            turn: tokio::sync::Mutex::new(()),
            cache: std::sync::Mutex::default(),
            pending_delete: None,
        }
    }

    /// N123: never read or write the part summaries of a meeting this says is pending delete.
    pub fn with_pending_delete(mut self, f: PendingDelete) -> Self {
        self.pending_delete = Some(f);
        self
    }

    /// N123: the meeting was deleted (or its delete scheduled): drop its part summaries, and make sure an attempt
    /// still running for it doesn't store them again. A meeting with this id summarized later starts over.
    pub fn forget(&self, id: &str) {
        self.cache().deleted(id);
    }

    /// Meetings with cached part summaries (tests).
    #[doc(hidden)]
    pub fn cached_meetings(&self) -> usize {
        self.cache().meetings.len()
    }

    /// Meeting ids whose delete count is kept (tests, N128): only those with an attempt in flight.
    #[doc(hidden)]
    pub fn tracked_deletes(&self) -> usize {
        self.cache().deletes.len()
    }

    fn pending_delete(&self, id: &str) -> bool {
        self.pending_delete.as_ref().is_some_and(|f| f(id))
    }

    /// One meeting's budget from its part count (N114): every map call, about one reduce call per two parts and the
    /// final merge, at [`PER_CALL_BUDGET`] each, between `deadline` and `max_deadline`.
    pub fn deadline_for(&self, parts: usize) -> Duration {
        let calls = parts + parts.div_ceil(2) + 1;
        let want = PER_CALL_BUDGET.saturating_mul(u32::try_from(calls).unwrap_or(u32::MAX));
        want.clamp(self.deadline, self.max_deadline.max(self.deadline))
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, PartCache> {
        self.cache.lock().unwrap_or_else(crate::poison::recover)
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

    /// Every transcript part, in order. A part over the context window is split in half (by lines), up to
    /// [`MAX_SPLITS`] times; labels are renumbered after each split, so they always read "part k of n" with every k
    /// once (N117). Pieces done (or found too long) in an earlier, unfinished attempt come from the cache (N114).
    async fn map_all(
        &self,
        c: &Ctx,
        id: &str,
        parts: Vec<String>,
        until: Instant,
    ) -> Result<Vec<SummaryReply>, OnDeviceError> {
        let mut queue: VecDeque<(String, u32)> = parts.into_iter().map(|p| (p, 0)).collect();
        let mut done: Vec<SummaryReply> = Vec::new();
        let attempt = Attempt::begin(&self.cache, id);
        let deletes = attempt.deletes;
        while let Some((body, depth)) = queue.pop_front() {
            let key = piece_key(&c.title, &body);
            // N123: a meeting pending delete keeps nothing: no read, no write, and what it had is dropped.
            let hidden = self.pending_delete(id);
            let cached = if hidden {
                self.cache().forget(id);
                None
            } else {
                self.cache().get(id, key)
            };
            match cached {
                Some(Piece::Done(r)) => {
                    done.push(r);
                    continue;
                }
                Some(Piece::Split) if depth < MAX_SPLITS => {
                    let (a, b) = halves(&body);
                    queue.push_front((b, depth + 1));
                    queue.push_front((a, depth + 1));
                    continue;
                }
                _ => {}
            }
            let total = done.len() + 1 + queue.len();
            let stage = if total == 1 { Stage::Single } else { Stage::Map };
            let prompt = self.prompt(c, stage, &body, Some((done.len() + 1, total)));
            match self.call(stage, prompt, until).await {
                Ok(r) => {
                    // A whole-meeting (single) reply carries the template's guidance: not reused.
                    if stage == Stage::Map && !self.pending_delete(id) {
                        self.cache().put(id, deletes, key, Piece::Done(r.clone()));
                    }
                    done.push(r);
                }
                Err(OnDeviceError::Context) if depth < MAX_SPLITS => {
                    if !self.pending_delete(id) {
                        self.cache().put(id, deletes, key, Piece::Split);
                    }
                    let (a, b) = halves(&body);
                    queue.push_front((b, depth + 1));
                    queue.push_front((a, depth + 1));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(done)
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
                next.extend(group.pop());
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
        // N115: waiting for another meeting's turn is not this meeting's model time.
        let _turn = self.turn.lock().await;
        let parts = self.parts(m);
        let until = Instant::now() + self.deadline_for(parts.len());
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
        let reply = if parts.is_empty() {
            // Notes only: one call over the notes.
            self.call(Stage::Single, self.prompt(&c, Stage::Single, "", None), until)
                .await?
        } else {
            let mut replies = self.map_all(&c, &m.id, parts, until).await?;
            if replies.len() == 1 {
                replies.swap_remove(0)
            } else {
                self.reduce(&c, replies, until).await?
            }
        };
        let r = clean_reply(reply);
        if r.summary.is_empty() {
            return Err(OnDeviceError::Failed("empty summary".into()));
        }
        self.cache().forget(&m.id);
        Ok(r.into_summary(PROVIDER_ID, tpl.id))
    }
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
/// One request at a time: [`OnDeviceSummaries`] holds one meeting's turn on the model (N115), and the helper
/// also queues.
pub struct HelperModel {
    sup: Option<qrtz_helper_bridge::Supervisor>,
}

impl HelperModel {
    pub fn new(sup: Option<qrtz_helper_bridge::Supervisor>) -> Self {
        HelperModel { sup }
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

/// The helper went away while the request was out (crash, restart).
struct Interrupted;

/// Waits up to `wait` for the helper to be running (N113: a restart's backoff is longer than the summary's).
/// `false` when it isn't by then, or the supervisor gave up.
async fn helper_running(sup: &qrtz_helper_bridge::Supervisor, wait: Duration) -> bool {
    use qrtz_helper_bridge::HelperState;
    let mut st = sup.watch_status();
    let r = tokio::time::timeout(
        wait,
        st.wait_for(|s| matches!(s.state, HelperState::Running | HelperState::Failed)),
    )
    .await;
    matches!(r, Ok(Ok(s)) if s.state == HelperState::Running)
}

/// The invalid-answer check for a line the contract rejected (N116): our id and a `summarize.` type, read from
/// the raw JSON without the schema.
fn is_our_invalid_answer(line: &str, id: &str) -> bool {
    let Ok(raw) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    raw["id"].as_str() == Some(id) && raw["type"].as_str().is_some_and(|t| t.starts_with("summarize."))
}

impl HelperModel {
    async fn request(
        sup: &qrtz_helper_bridge::Supervisor,
        stage: Stage,
        prompt: &str,
    ) -> Result<Result<SummaryReply, OnDeviceError>, Interrupted> {
        use qrtz_helper_bridge::HelperState;
        let id = crate::wiring::uuid_v4();
        let line = match qrtz_helper_bridge::encode_host_message(
            &json!({"type": "summarize", "id": id, "stage": stage.as_str(), "prompt": prompt}),
        ) {
            Ok(l) => l,
            Err(e) => {
                return Ok(Err(OnDeviceError::Failed(
                    cut_utf16(&e.to_string(), 120).to_string(),
                )))
            }
        };
        let mut lines = sup.subscribe_lines();
        let mut status = sup.watch_status();
        // The process this request goes to: a later restart (even one already back to running) is another one.
        let sent_to = status.borrow_and_update().restarts;
        if !sup.send(line).await {
            return Err(Interrupted);
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
                        // N119: most lines while a summary waits are levels and partials; skip them unparsed.
                        let ours = l.contains("\"summarize.");
                        let old_helper = !ours && l.contains("unknown host message type summarize");
                        if !ours && !old_helper {
                            continue;
                        }
                        let v = match qrtz_helper_bridge::parse_helper_line(&l) {
                            Ok(v) => v,
                            Err(_) => {
                                if is_our_invalid_answer(&l, &id) {
                                    // N116: the helper answered, but not in the contract: fail now, not in 180 s.
                                    guard.done = true;
                                    return Ok(Err(OnDeviceError::InvalidReply));
                                }
                                continue;
                            }
                        };
                        // A helper older than ADR 0075 logs the unknown line instead of answering.
                        if old_helper
                            && v["type"] == "log"
                            && v["msg"].as_str().is_some_and(|m| m.contains("unknown host message type summarize"))
                        {
                            guard.done = true;
                            return Ok(Err(OnDeviceError::Unavailable(Unavailable::Unsupported)));
                        }
                        if v["id"].as_str() != Some(id.as_str()) {
                            continue;
                        }
                        match v["type"].as_str() {
                            Some("summarize.result") => {
                                guard.done = true;
                                return Ok(reply_from_result(&v));
                            }
                            Some("summarize.error") => {
                                guard.done = true;
                                return Ok(Err(error_from_line(&v)));
                            }
                            _ => {}
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => {
                        guard.done = true;
                        return Err(Interrupted);
                    }
                },
                c = status.changed() => {
                    let gone = c.is_err() || {
                        let s = status.borrow_and_update();
                        s.state != HelperState::Running || s.restarts != sent_to
                    };
                    if gone {
                        // That process is gone; its id dies with it.
                        guard.done = true;
                        return Err(Interrupted);
                    }
                }
            }
        }
    }
}

#[async_trait]
impl OnDeviceModel for HelperModel {
    /// N113: waits (up to [`HELPER_WAIT`]) for the helper before sending; if it restarts while the request is out,
    /// waits again and resends once, with a new id (a late answer to the old id is ignored by the id filter).
    async fn summarize(&self, stage: Stage, prompt: String) -> Result<SummaryReply, OnDeviceError> {
        let Some(sup) = &self.sup else {
            return Err(OnDeviceError::Unavailable(Unavailable::Unsupported));
        };
        for _ in 0..2 {
            if !helper_running(sup, HELPER_WAIT).await {
                return Err(OnDeviceError::NoHelper);
            }
            match Self::request(sup, stage, &prompt).await {
                Ok(r) => return r,
                Err(Interrupted) => tracing::info!("helper restarted mid-summary; resending"),
            }
        }
        Err(OnDeviceError::NoHelper)
    }
}
