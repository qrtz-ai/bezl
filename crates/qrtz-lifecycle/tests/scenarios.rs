//! Tier-2 runtime scenarios (docs/rust-core-api.md §5): every spec/scenarios/lifecycle/*.json is replayed against
//! [`NoteRuntime`] on a paused tokio clock with the same scripted helper, step outcomes and store faults as the TS
//! runner (apps/desktop/src/main/note-service.scenarios.test.ts, the oracle that wrote `expect`). The trace,
//! call results and snapshots (view, live transcript, DB rows, recordings dir) must match exactly.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use qrtz_lifecycle::runtime::*;
use qrtz_model::*;
use qrtz_storage::Db;
use serde_json::{json, Value};

const T0: i64 = 1_760_000_000_000;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/scenarios/lifecycle")
}

fn compact(steps: &PipelineSteps) -> String {
    [StepId::Finalize, StepId::Summary, StepId::Index]
        .iter()
        .map(|k| {
            let s = steps.get(*k);
            format!(
                "{}:{}/{}{}{}",
                k.as_str(),
                s.status.as_str(),
                s.attempts,
                if s.fallback == Some(true) { "+fb" } else { "" },
                s.error
                    .as_ref()
                    .map(|e| format!("!{}", e.code))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `compactCapture` (TS runner): "" when absent.
fn compact_capture(c: &Option<CaptureReport>) -> String {
    let Some(c) = c else { return String::new() };
    c.silent_spans
        .iter()
        .map(|x| {
            format!(
                "{}:{}-{}",
                x.source.as_str(),
                x.start_ms,
                x.end_ms.map_or("?".to_string(), |e| e.to_string())
            )
        })
        .chain(
            c.fallbacks
                .iter()
                .map(|f| format!("fb:{}>{}@{}", f.from, f.to.as_deref().unwrap_or("-"), f.at_ms)),
        )
        .collect::<Vec<_>>()
        .join(" ")
}

/// `compactSilent` (TS runner): "" when nothing is silent.
fn compact_silent(v: &SessionView) -> String {
    if v.silent.is_empty() {
        return String::new();
    }
    format!(
        "s:{}{}",
        v.silent
            .iter()
            .map(|x| format!("{}@{}", x.source.as_str(), x.since_ms))
            .collect::<Vec<_>>()
            .join(","),
        if v.no_audio { "!" } else { "" }
    )
}

fn perm_str(p: &PermissionsStatus) -> String {
    let g = |k| p.get(&k).map_or("unknown", |v: &PermissionState| v.as_str());
    format!(
        "{}|{}|{}",
        g(OsPermission::Accessibility),
        g(OsPermission::ScreenRecording),
        g(OsPermission::Microphone)
    )
}

// ------------------------------------------------------------------ fakes

#[derive(Default)]
struct Trace {
    session: Vec<String>,
    pipelines: BTreeMap<String, Vec<String>>,
    helper: Vec<String>,
    transcript: Vec<Value>,
    changed: Vec<String>,
    permissions: Vec<String>,
    prompts: Vec<String>,
}

struct Shared {
    rec: String,
    trace: Mutex<Trace>,
    up: AtomicBool,
    failing: Mutex<HashSet<String>>,
    /// Undo tombstones (`pendingDelete` op).
    tombstones: Arc<Mutex<HashSet<String>>>,
    outcomes: Mutex<HashMap<&'static str, VecDeque<String>>>,
}

impl Shared {
    fn rel(&self, s: &str) -> String {
        s.replace(&self.rec, "$REC")
    }
    fn fail(&self, method: &str) -> Result<(), LifecycleError> {
        if self.failing.lock().unwrap().contains(method) {
            return Err(LifecycleError {
                code: "E_UNAVAILABLE".into(),
                message: format!("{method} unavailable"),
                retryable: true,
            });
        }
        Ok(())
    }
}

/// One process lifetime: dropped (dead) at `restart`.
struct Life {
    sh: Arc<Shared>,
    dead: AtomicBool,
    last_session: Mutex<String>,
}

impl EventSink for Life {
    fn emit(&self, e: CoreEvent) {
        if self.dead.load(Ordering::SeqCst) {
            return;
        }
        let mut t = self.sh.trace.lock().unwrap();
        match e {
            CoreEvent::Session(v) => {
                let k = [
                    v.phase.as_str().to_string(),
                    v.meeting_id.clone().unwrap_or("-".into()),
                    if v.degraded.is_empty() {
                        "-".into()
                    } else {
                        v.degraded
                            .iter()
                            .map(|d| d.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    },
                    v.error_info.as_ref().map_or("-".into(), |e| e.code.clone()),
                    v.reconnect
                        .as_ref()
                        .map_or("-".into(), |r| format!("r{}", r.attempt)),
                    if v.paused { "paused".into() } else { "-".into() },
                    v.helper.as_str().into(),
                ]
                .into_iter()
                .chain(Some(compact_silent(&v)).filter(|x| !x.is_empty()))
                .collect::<Vec<_>>()
                .join("|");
                let mut last = self.last_session.lock().unwrap();
                if *last != k {
                    t.session.push(k.clone());
                    *last = k;
                }
            }
            CoreEvent::Pipeline(v) => {
                let cap = compact_capture(&v.capture);
                let k = format!(
                    "{} {}{}",
                    v.stop_reason.as_str(),
                    compact(&v.steps),
                    if cap.is_empty() {
                        String::new()
                    } else {
                        format!(" [{cap}]")
                    }
                );
                let l = t.pipelines.entry(v.meeting_id.clone()).or_default();
                if l.last() != Some(&k) {
                    l.push(k);
                }
            }
            CoreEvent::Transcript(d) => t.transcript.push(json!([
                d.base_rev,
                d.rev,
                d.append
                    .iter()
                    .map(|a| format!(
                        "{}{}",
                        a.id,
                        a.speaker.as_ref().map(|s| format!("@{s}")).unwrap_or_default()
                    ))
                    .collect::<Vec<_>>(),
                d.partial.as_ref().map(|p| p.text.clone()),
            ])),
            CoreEvent::Changed { id } => t.changed.push(id),
            CoreEvent::PermissionsChanged(p) => t.permissions.push(perm_str(&p)),
            CoreEvent::DetectedPrompt { title, platform } => {
                t.prompts.push(format!("{title}|{}", platform.as_str()))
            }
        }
    }
}

impl HelperPort for Life {
    fn send(&self, cmd: &HelperCommand) -> bool {
        if self.dead.load(Ordering::SeqCst) || !self.sh.up.load(Ordering::SeqCst) {
            return false;
        }
        let j = cmd.to_json();
        let mut s = j["type"].as_str().unwrap().to_string();
        if let Some(x) = j.get("audioPath").or(j.get("permission")).and_then(Value::as_str) {
            s = format!("{s} {}", self.sh.rel(x));
        }
        self.sh.trace.lock().unwrap().helper.push(s);
        true
    }
    fn restart(&self) {
        if !self.dead.load(Ordering::SeqCst) {
            self.sh.trace.lock().unwrap().helper.push("restart".into());
        }
    }
}

/// The real store with the scenario's `store.fail` set (TS: the worker wrapper's fault set).
struct FaultyStore {
    sh: Arc<Shared>,
    inner: Store<InlineDb>,
}

macro_rules! guarded {
    ($self:ident, $m:literal, $call:expr) => {{
        $self.sh.fail($m)?;
        $call.await
    }};
}

#[async_trait]
impl LifecycleStore for FaultyStore {
    async fn create_meeting(&self, m: Meeting) -> StoreResult<()> {
        guarded!(self, "note.create", self.inner.create_meeting(m))
    }
    async fn delete_meeting(&self, id: String) -> StoreResult<Vec<String>> {
        guarded!(self, "note.delete", self.inner.delete_meeting(id))
    }
    async fn append_live(&self, meeting_id: String, s: Segment) -> StoreResult<()> {
        guarded!(self, "note.appendLive", self.inner.append_live(meeting_id, s))
    }
    async fn mark_failed(&self, meeting_id: String) -> StoreResult<()> {
        guarded!(self, "note.fail", self.inner.mark_failed(meeting_id))
    }
    async fn add_note(
        &self,
        meeting_id: String,
        text: String,
        at_ms: Ms,
        created_at: EpochMs,
        ids: IdGen,
    ) -> StoreResult<NoteWithMeeting> {
        guarded!(
            self,
            "note.addNote",
            self.inner.add_note(meeting_id, text, at_ms, created_at, ids)
        )
    }
    async fn get_summary(&self, meeting_id: String) -> StoreResult<Option<MeetingSummary>> {
        guarded!(self, "note.get", self.inner.get_summary(meeting_id))
    }
    async fn save_session(&self, s: RecordingState, now: EpochMs) -> StoreResult<()> {
        guarded!(self, "lifecycle.saveSession", self.inner.save_session(s, now))
    }
    async fn load_session(&self) -> StoreResult<Option<RecordingState>> {
        guarded!(self, "lifecycle.loadSession", self.inner.load_session())
    }
    async fn save_pipeline(&self, p: PipelineState, settled: bool) -> StoreResult<bool> {
        guarded!(self, "pipeline.save", self.inner.save_pipeline(p, settled))
    }
    async fn get_pipeline(&self, meeting_id: String) -> StoreResult<Option<PipelineState>> {
        guarded!(self, "pipeline.get", self.inner.get_pipeline(meeting_id))
    }
    async fn list_unsettled(&self) -> StoreResult<Vec<PipelineState>> {
        guarded!(self, "pipeline.listUnsettled", self.inner.list_unsettled())
    }
    async fn orphans(&self) -> StoreResult<Vec<Meeting>> {
        guarded!(self, "pipeline.orphans", self.inner.orphans())
    }
    async fn prune_audio(&self, days: u32, now: EpochMs) -> StoreResult<Vec<String>> {
        guarded!(self, "note.pruneAudio", self.inner.prune_audio(days, now))
    }
}

/// Scripted summary/index (and finalize failures), the real finalize: the TS runner's `scripted`.
struct ScriptedSteps {
    sh: Arc<Shared>,
    db: InlineDb,
}

impl ScriptedSteps {
    fn next(&self, step: &'static str) -> String {
        self.sh
            .outcomes
            .lock()
            .unwrap()
            .entry(step)
            .or_default()
            .pop_front()
            .unwrap_or("ok".into())
    }
    async fn outcome(&self, method: &str, step: &'static str) -> Result<String, LifecycleError> {
        self.sh.fail(method)?;
        let mut o = self.next(step);
        if let Some(rest) = o.strip_prefix("slow-") {
            // The step takes SLOW_MS of (paused tokio) time, then succeeds or fails retryably (N27).
            tokio::time::sleep(Duration::from_millis(5000)).await;
            o = rest.to_string();
        }
        match o.as_str() {
            "hang" => std::future::pending().await,
            "fail" => Err(LifecycleError {
                code: "E_UNAVAILABLE".into(),
                message: format!("{method} unavailable"),
                retryable: true,
            }),
            "fatal" => Err(LifecycleError {
                code: "E_INTERNAL".into(),
                message: format!("{method} failed"),
                retryable: false,
            }),
            _ => Ok(o),
        }
    }
    fn db<R>(&self, f: impl FnOnce(&Db) -> qrtz_storage::Result<R>) -> Result<R, LifecycleError> {
        f(&self.db.0.lock().unwrap()).map_err(lifecycle_error)
    }
    fn save(
        &self,
        id: &str,
        summary: &str,
        provider: &str,
        llm_error: Option<String>,
    ) -> Result<MeetingSummary, LifecycleError> {
        let s = MeetingSummary {
            summary: summary.into(),
            action_items: vec![],
            qa: vec![],
            provider: provider.into(),
            template: None,
            llm_error,
        };
        self.db(|db| {
            db.notes().need(id)?;
            db.notes().save_summary(id, &s, T0)?;
            db.notes().set_status(id, MeetingStatus::Done)
        })?;
        Ok(s)
    }
}

#[async_trait]
impl StepExecutor for ScriptedSteps {
    async fn finalize(
        &self,
        meeting_id: String,
        ctx: PipelineContext,
    ) -> Result<FinalizeOutcome, LifecycleError> {
        self.outcome("note.stepFinalize", "finalize").await?;
        self.db(|db| finalize_sync(db, &meeting_id, &ctx))
    }
    async fn summary(
        &self,
        meeting_id: String,
        req: SummaryRequest,
    ) -> Result<Option<MeetingSummary>, LifecycleError> {
        match req {
            SummaryRequest::Skip => {
                self.sh.fail("note.stepSummary")?;
                self.db(|db| {
                    db.notes().need(&meeting_id)?;
                    db.notes().set_status(&meeting_id, MeetingStatus::Done)
                })?;
                Ok(None)
            }
            SummaryRequest::Fallback { error } => {
                self.sh.fail("note.stepSummary")?;
                let e: String = error.chars().take(500).collect();
                self.save(&meeting_id, "FALLBACK summary", "extractive", Some(e))
                    .map(Some)
            }
            SummaryRequest::Llm => {
                self.outcome("note.stepSummary", "summary").await?;
                self.save(&meeting_id, "STUB summary", "stub", None).map(Some)
            }
        }
    }
    async fn index(&self, meeting_id: String) -> Result<IndexOutcome, LifecycleError> {
        let o = self.outcome("note.stepIndex", "index").await?;
        self.db(|db| db.notes().need(&meeting_id))?;
        Ok(IndexOutcome {
            indexed: 0,
            skipped: o == "skip",
        })
    }
    async fn resummarize(&self, _id: String, _t: Option<String>) -> Result<MeetingSummary, LifecycleError> {
        Err(LifecycleError {
            code: "E_UNAVAILABLE".into(),
            message: "note.resummarize unavailable".into(),
            retryable: true,
        })
    }
}

// ------------------------------------------------------------------ runner

struct Run {
    sc: Value,
    sh: Arc<Shared>,
    db: InlineDb,
    rec: PathBuf,
    ids: Arc<AtomicU64>,
    clock: Arc<TokioClock>,
    config: RuntimeConfig,
    results: Arc<Mutex<BTreeMap<String, Value>>>,
    snapshots: BTreeMap<String, Value>,
}

struct Live {
    rt: NoteRuntime,
    life: Arc<Life>,
    calls: Vec<tokio::task::AbortHandle>,
}

async fn settle() {
    for _ in 0..25 {
        tokio::time::sleep(Duration::ZERO).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }
}

fn view_json(v: &SessionView) -> Value {
    let mut j = view_json_base(v);
    if !v.silent.is_empty() {
        j["silent"] = json!(v
            .silent
            .iter()
            .map(|x| json!([x.source.as_str(), x.since_ms]))
            .collect::<Vec<_>>());
        j["noAudio"] = json!(v.no_audio);
    }
    j
}

fn view_json_base(v: &SessionView) -> Value {
    json!({
        "phase": v.phase.as_str(),
        "meetingId": v.meeting_id,
        "startedAt": v.started_at,
        "elapsedMs": v.elapsed_ms,
        "stopReason": v.stop_reason.map(|r| r.as_str()),
        "degraded": v.degraded.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
        "error": v.error_info.as_ref().map(|e| json!([e.code, e.retryable])),
        "paused": v.paused,
        "progress": v.progress,
        "reconnect": v.reconnect.as_ref().map(|r| json!({"attempt": r.attempt, "since": r.since, "deadlineAt": r.deadline_at})),
        "helper": v.helper.as_str(),
        "participants": v.participants,
        "activeSpeaker": v.active_speaker,
        "lines": v.lines.iter().map(|l| json!([l.id, l.speaker, l.text, l.start_ms])).collect::<Vec<_>>(),
        "partial": v.partial,
        "pipeline": v.pipeline.as_ref().map(|p| compact(&p.steps)),
    })
}

fn brief(v: &SessionView) -> Value {
    json!({"phase": v.phase.as_str(), "meetingId": v.meeting_id})
}

fn err_json(e: &CoreError) -> Value {
    json!({"err": [e.code, e.message]})
}

fn write_wav(path: &Path, ms: f64, broken: bool) {
    let data = (ms * 32.0).round() as u32;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(if broken { 36 } else { 36 + data }).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&16000u32.to_le_bytes());
    b.extend_from_slice(&32000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(if broken { 0 } else { data }).to_le_bytes());
    b.resize(44 + data as usize, 0);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b).unwrap();
}

fn sql_value(v: rusqlite::types::Value) -> Value {
    use rusqlite::types::Value as V;
    match v {
        V::Null => Value::Null,
        V::Integer(i) => json!(i),
        V::Real(f) => json!(f),
        V::Text(s) => json!(s),
        V::Blob(_) => json!("<blob>"),
    }
}

impl Run {
    fn boot(&self) -> Live {
        let life = Arc::new(Life {
            sh: self.sh.clone(),
            dead: AtomicBool::new(false),
            last_session: Mutex::new(String::new()),
        });
        let ids = self.ids.clone();
        let config = self.config.clone();
        let random = self.sc["random"].as_f64().unwrap_or(0.5);
        let deps = RuntimeDeps {
            store: Arc::new(FaultyStore {
                sh: self.sh.clone(),
                inner: Store(self.db.clone()),
            }),
            steps: Arc::new(ScriptedSteps {
                sh: self.sh.clone(),
                db: self.db.clone(),
            }),
            helper: life.clone(),
            events: life.clone(),
            config: Arc::new(move || config.clone()),
            clock: self.clock.clone(),
            ids: Arc::new(move || format!("id{}", ids.fetch_add(1, Ordering::SeqCst) + 1)),
            random: Arc::new(move || random),
            recordings_dir: self.rec.clone(),
            retry_scale: self.sc["retryScale"].as_f64().unwrap_or(1.0),
            fault_step: self.sc["faultStep"].as_str().and_then(StepId::parse),
            default_title: Arc::new(utc_recording_title),
            pending_delete: Some({
                let t = self.sh.tombstones.clone();
                Arc::new(move |id: &str| t.lock().unwrap().contains(id))
            }),
        };
        let rt = NoteRuntime::new(deps);
        let r2 = rt.clone();
        let h = tokio::spawn(async move {
            let _ = r2.recover().await;
        });
        Live {
            rt,
            life,
            calls: vec![h.abort_handle()],
        }
    }

    fn record<T: Send + 'static>(
        &self,
        live: &mut Live,
        label: &str,
        f: impl std::future::Future<Output = Result<T, CoreError>> + Send + 'static,
        map: impl FnOnce(T) -> Value + Send + 'static,
    ) {
        let results = self.results.clone();
        let label = label.to_string();
        results.lock().unwrap().insert(label.clone(), json!("pending"));
        let h = tokio::spawn(async move {
            let v = match f.await {
                Ok(v) => json!({"ok": map(v)}),
                Err(e) => err_json(&e),
            };
            results.lock().unwrap().insert(label, v);
        });
        live.calls.push(h.abort_handle());
    }

    fn call(&self, live: &mut Live, op: &Value, label: &str) {
        let a = op.get("args").cloned().unwrap_or(json!({}));
        let rt = live.rt.clone();
        let s = |k: &str| a[k].as_str().map(str::to_string);
        match op["fn"].as_str().unwrap() {
            "start" => {
                let t = s("title");
                self.record(live, label, async move { rt.start(t).await }, |v| brief(&v))
            }
            "stop" => {
                let r = s("reason")
                    .and_then(|r| StopReason::parse(&r))
                    .unwrap_or(StopReason::Manual);
                self.record(live, label, async move { rt.stop(r).await }, |v| brief(&v))
            }
            "toggle" => self.record(live, label, async move { rt.toggle().await }, |v| brief(&v)),
            "dismiss" => {
                let v = rt.dismiss();
                self.results
                    .lock()
                    .unwrap()
                    .insert(label.into(), json!({"ok": brief(&v)}));
            }
            "pause" => {
                let v = match rt.pause(a["want"].as_bool()) {
                    Ok(v) => json!({"ok": brief(&v)}),
                    Err(e) => err_json(&e),
                };
                self.results.lock().unwrap().insert(label.into(), v);
            }
            "retryStep" => {
                let (id, step) = (
                    s("id").unwrap(),
                    StepId::parse(a["step"].as_str().unwrap()).unwrap(),
                );
                self.record(live, label, async move { rt.retry_step(&id, step).await }, |p| {
                    json!(compact(&p.steps))
                })
            }
            "deleteMeeting" => {
                let id = s("id").unwrap();
                self.record(live, label, async move { rt.delete_meeting(&id).await }, |r| {
                    let mut names: Vec<String> = r
                        .iter()
                        .map(|f| {
                            std::path::Path::new(f)
                                .file_name()
                                .unwrap()
                                .to_string_lossy()
                                .into_owned()
                        })
                        .collect();
                    names.sort();
                    json!(names)
                })
            }
            "pipeline" => {
                let id = s("id").unwrap();
                self.record(live, label, async move { rt.pipeline(&id).await }, |p| {
                    p.map_or(Value::Null, |p| json!(compact(&p.steps)))
                })
            }
            "addNote" => {
                let req: AddNoteRequest = serde_json::from_value(a.clone()).unwrap();
                self.record(live, label, async move { rt.add_note(req).await }, |n| {
                    json!([n.note.id, n.meeting_id, n.note.at_ms, n.note.text])
                })
            }
            "resummarize" => {
                let id = s("id").unwrap();
                self.record(live, label, async move { rt.resummarize(&id).await }, |s| {
                    s.map_or(Value::Null, |s| json!(s.summary))
                })
            }
            "requestPermission" => {
                let p = OsPermission::parse(a["permission"].as_str().unwrap()).unwrap();
                self.record(live, label, async move { rt.request_permission(p).await }, |p| {
                    json!(perm_str(&p))
                })
            }
            other => panic!("unknown call {other}"),
        }
    }

    fn snapshot(&self, live: &Live) -> Value {
        let db = self.db.0.lock().unwrap();
        let conn = db.connection();
        let all = |sql: &str| -> Vec<Vec<Value>> {
            let mut st = conn.prepare(sql).unwrap();
            let n = st.column_count();
            st.query_map([], |r| {
                (0..n)
                    .map(|i| r.get::<_, rusqlite::types::Value>(i).map(sql_value))
                    .collect()
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
        };
        let rel = |v: &Value| v.as_str().map_or(Value::Null, |s| json!(self.sh.rel(s)));
        let meetings: Vec<Value> = all("SELECT id, title, platform, status, started_at, ended_at, duration_ms, audio_path, template FROM meetings ORDER BY id")
            .into_iter()
            .map(|mut r| {
                r[7] = rel(&r[7]);
                json!(r)
            })
            .collect();
        let segments = all("SELECT meeting_id, id, source, start_ms, end_ms, speaker_name, text FROM segments ORDER BY meeting_id, source, start_ms, id");
        let summaries: Vec<Value> =
            all("SELECT meeting_id, provider, json FROM summaries ORDER BY meeting_id")
                .into_iter()
                .map(|r| {
                    let j: Value = serde_json::from_str(r[2].as_str().unwrap()).unwrap();
                    json!([
                        r[0],
                        r[1],
                        j["summary"],
                        j.get("llmError").cloned().unwrap_or(Value::Null)
                    ])
                })
                .collect();
        let notes = all("SELECT id, meeting_id, at_ms, text FROM notes ORDER BY meeting_id, at_ms, id");
        let pipelines: Vec<Value> =
            all("SELECT meeting_id, json, settled FROM pipeline_state ORDER BY meeting_id")
                .into_iter()
                .map(|r| {
                    let p: PipelineState = serde_json::from_str(r[1].as_str().unwrap()).unwrap();
                    let mut row = json!([
                        p.meeting_id,
                        r[2],
                        p.stop_reason.as_str(),
                        compact(&p.steps),
                        p.ctx.duration_ms,
                        p.ctx.helper_crashed,
                        p.ctx
                            .audio_parts
                            .iter()
                            .map(|a| self.sh.rel(a))
                            .collect::<Vec<_>>(),
                        p.ctx.active.len(),
                    ]);
                    if p.capture.is_some() {
                        row.as_array_mut()
                            .unwrap()
                            .push(json!(compact_capture(&p.capture)));
                    }
                    row
                })
                .collect();
        let session = all("SELECT json FROM lifecycle_state WHERE key='session'")
            .into_iter()
            .next()
            .map(|r| {
                let s: Value = serde_json::from_str(r[0].as_str().unwrap()).unwrap();
                json!([s["phase"], s.get("meetingId").cloned().unwrap_or(Value::Null)])
            });
        drop(db);
        let mut files: Vec<Value> = match std::fs::read_dir(&self.rec) {
            Ok(rd) => rd
                .map(|e| {
                    let e = e.unwrap();
                    json!([e.file_name().to_string_lossy(), e.metadata().unwrap().len()])
                })
                .collect(),
            Err(_) => vec![],
        };
        files.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));
        let live_t = live.rt.live_transcript(None).map(|lt| {
            json!({
                "meetingId": lt.meeting_id,
                "rev": lt.rev,
                "segments": lt.segments.iter().map(|s| json!([s.id, s.speaker, s.start_ms, s.end_ms])).collect::<Vec<_>>(),
                "partial": lt.partial.map(|p| p.text),
            })
        });
        json!({
            "view": view_json(&live.rt.session()),
            "live": live_t,
            "db": {"meetings": meetings, "segments": segments, "summaries": summaries, "notes": notes, "pipelines": pipelines, "session": session},
            "files": files,
        })
    }

    async fn exec(mut self) -> Value {
        let mut live = self.boot();
        settle().await;
        let script = self.sc["script"].as_array().unwrap().clone();
        let mut n = 0;
        for op in &script {
            n += 1;
            let name = op["op"].as_str().unwrap();
            let label = op["label"].as_str().map_or(format!("{name}#{n}"), str::to_string);
            match name {
                "helper" => {
                    let raw = serde_json::to_string(&op["msg"])
                        .unwrap()
                        .replace("$REC", &self.sh.rec);
                    let v: Value = serde_json::from_str(&raw).unwrap();
                    live.rt.helper_json(&v);
                }
                "call" => self.call(&mut live, op, &label),
                "advance" => tokio::time::sleep(Duration::from_millis(op["ms"].as_u64().unwrap())).await,
                "clockJump" => self.clock.jump(op["ms"].as_i64().unwrap()),
                "helperDown" => {
                    self.sh.up.store(false, Ordering::SeqCst);
                    live.rt.helper_down(op["reason"].as_str().unwrap_or("exited"));
                }
                "helperUp" => self.sh.up.store(true, Ordering::SeqCst),
                "helperFailed" => live.rt.helper_failed("failed"),
                "helperHealth" => live.rt.set_helper_health(
                    HelperHealth::parse(op["health"].as_str().unwrap()).unwrap(),
                    op["detail"].as_str().map(str::to_string),
                ),
                "steps" => {
                    let mut o = self.sh.outcomes.lock().unwrap();
                    for k in ["finalize", "summary", "index"] {
                        for x in op[k].as_array().into_iter().flatten() {
                            o.entry(k).or_default().push_back(x.as_str().unwrap().into());
                        }
                    }
                }
                "pendingDelete" => {
                    self.sh
                        .tombstones
                        .lock()
                        .unwrap()
                        .insert(op["id"].as_str().unwrap().to_string());
                }
                "store" => {
                    let mut f = self.sh.failing.lock().unwrap();
                    for m in op["fail"].as_array().into_iter().flatten() {
                        f.insert(m.as_str().unwrap().into());
                    }
                    for m in op["heal"].as_array().into_iter().flatten() {
                        f.remove(m.as_str().unwrap());
                    }
                }
                "wav" => write_wav(
                    &self.rec.join(op["file"].as_str().unwrap()),
                    op["ms"].as_f64().unwrap(),
                    op["broken"].as_bool().unwrap_or(false),
                ),
                "db" => {
                    let db = self.db.0.lock().unwrap();
                    if let Some(id) = op["deleteMeeting"].as_str() {
                        db.notes().delete_meeting(id).unwrap();
                    }
                    if !op["seedMeeting"].is_null() {
                        let raw = serde_json::to_string(&op["seedMeeting"])
                            .unwrap()
                            .replace("$REC", &self.sh.rec);
                        let m: Meeting = serde_json::from_str(&raw).unwrap();
                        db.notes().upsert_meeting(&m).unwrap();
                    }
                }
                "restart" => {
                    live.life.dead.store(true, Ordering::SeqCst);
                    live.rt.kill();
                    for c in live.calls.drain(..) {
                        c.abort();
                    }
                    self.sh.up.store(true, Ordering::SeqCst);
                    live = self.boot();
                }
                "snapshot" => {
                    let s = self.snapshot(&live);
                    self.snapshots.insert(label.clone(), s);
                }
                other => panic!("unknown op {other}"),
            }
            settle().await;
        }
        let end = self.snapshot(&live);
        self.snapshots.insert("end".into(), end);
        live.life.dead.store(true, Ordering::SeqCst);
        live.rt.kill();
        let t = self.sh.trace.lock().unwrap();
        json!({
            "trace": {
                "session": t.session,
                "pipelines": t.pipelines,
                "helper": t.helper,
                "transcript": t.transcript,
                "changed": t.changed,
                "permissions": t.permissions,
                "prompts": t.prompts,
            },
            "results": *self.results.lock().unwrap(),
            "snapshots": self.snapshots,
        })
    }
}

/// JSON equality with numbers compared by value; returns the first differing path.
fn diff(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            (x.as_f64() != y.as_f64()).then(|| format!("{path}: {a} != {b}"))
        }
        (Value::Array(x), Value::Array(y)) => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                if let Some(d) = diff(p, q, &format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
            (x.len() != y.len()).then(|| {
                format!(
                    "{path}: length {} != {}\n  got:  {a}\n  want: {b}",
                    x.len(),
                    y.len()
                )
            })
        }
        (Value::Object(x), Value::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let (p, q) = (x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null));
                if let Some(d) = diff(p, q, &format!("{path}.{k}")) {
                    return Some(d);
                }
            }
            None
        }
        _ => (a != b).then(|| format!("{path}: {a} != {b}")),
    }
}

fn run_one(file: &Path) -> Result<(), String> {
    let sc: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let rec = tmp.path().join("recordings");
    let db = InlineDb::new(Db::open(tmp.path().join("qrtz.db")).unwrap());
    let mut config = RuntimeConfig {
        retain_audio_days: 0,
        ..RuntimeConfig::default()
    };
    if let Some(c) = sc.get("config") {
        let mut base = serde_json::to_value(&config).unwrap();
        for (k, v) in c.as_object().unwrap() {
            base[k] = v.clone();
        }
        config = serde_json::from_value(base).unwrap();
    }
    config.excluded_apps = sc["excludedApps"]
        .as_array()
        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    let want = sc["expect"].clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    let got = rt.block_on(async {
        let run = Run {
            sc,
            sh: Arc::new(Shared {
                rec: rec.to_string_lossy().into_owned(),
                trace: Mutex::new(Trace::default()),
                up: AtomicBool::new(true),
                failing: Mutex::new(HashSet::new()),
                tombstones: Arc::new(Mutex::new(HashSet::new())),
                outcomes: Mutex::new(HashMap::new()),
            }),
            db,
            rec: rec.clone(),
            ids: Arc::new(AtomicU64::new(0)),
            clock: Arc::new(TokioClock::new(T0)),
            config,
            results: Arc::new(Mutex::new(BTreeMap::new())),
            snapshots: BTreeMap::new(),
        };
        run.exec().await
    });
    if std::env::var_os("QRTZ_SCENARIO_DUMP").is_some() {
        let out = std::env::temp_dir().join(file.file_name().unwrap());
        std::fs::write(&out, serde_json::to_string_pretty(&got).unwrap()).unwrap();
    }
    match diff(&got, &want, "") {
        Some(d) => Err(d),
        None => Ok(()),
    }
}

#[test]
fn tier2_lifecycle_scenarios() {
    let filter = std::env::var("QRTZ_SCENARIO").ok();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter(|p| {
            filter
                .as_ref()
                .is_none_or(|f| p.to_string_lossy().contains(f.as_str()))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no scenarios in {}", dir().display());
    let mut failures = vec![];
    for f in &files {
        if let Err(d) = run_one(f) {
            failures.push(format!("{}: {d}", f.file_name().unwrap().to_string_lossy()));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} scenarios differ from the TS oracle:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}
