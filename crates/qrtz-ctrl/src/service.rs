//! The `ctrl:*` channels of ctrl do (ADR 0214). Payloads arrive already validated against the IPC contract
//! (`packages/core/src/ipc/contract.ts`); answers are the contract's shapes (camelCase).
//!
//! - `ctrl:do {text}`: record the command, start its run and drive the chain until it asks, waits for a confirm,
//!   or ends. One run drives at a time; a new command cancels a run that was still waiting on Jared.
//! - `ctrl:runs`, `ctrl:run {runId, at?}`: the history and one run's view (at any point of its log).
//! - `ctrl:answer {runId, decisionId, optionId | null}`: Jared's pick for an ask (`null` cancels the run).
//! - `ctrl:confirm {runId, gateId, answer}`: approve or deny a gated action (approve acts once, re-checked).
//! - `ctrl:stop {runId}`: the stop button (the stop key is Esc, watched by the helper).
//!
//! The store lock is never held across a helper call: the run's log is loaded, driven in memory, then its new
//! records are written. Readable labels (element titles, app names) live in memory only, per run.
use crate::chain::resolve;
use crate::decide::{ask_options, Heuristic, Node, Scorer};
use crate::driver::{Driver, Step};
use crate::gate::ConfirmAnswer;
use crate::guard::StopReason;
use crate::helper::HelperClient;
use crate::log::{ActionStatus, CtrlState, Event, EventLog, TaskStatus};
use crate::runner::Clock;
use crate::store::{Store, StoreError};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlErrorCode {
    Validation,
    NotFound,
    Unavailable,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtrlError {
    pub code: CtrlErrorCode,
    pub message: String,
}

impl CtrlError {
    fn new(code: CtrlErrorCode, message: impl Into<String>) -> Self {
        CtrlError {
            code,
            message: message.into(),
        }
    }
}

impl From<StoreError> for CtrlError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound => CtrlError::new(CtrlErrorCode::NotFound, "that run is gone"),
            StoreError::Invalid(t) => CtrlError::new(CtrlErrorCode::Validation, t.message()),
            StoreError::Damaged { file, line } => CtrlError::new(
                CtrlErrorCode::Internal,
                format!("ctrl's history file {file} is damaged at line {line}; nothing was changed"),
            ),
            StoreError::Io(k) => CtrlError::new(
                CtrlErrorCode::Internal,
                format!("couldn't save ctrl's history ({k})"),
            ),
        }
    }
}

/// The real clock (monotonic since the service started, and wall time).
pub struct SystemClock(std::time::Instant);
impl Default for SystemClock {
    fn default() -> Self {
        SystemClock(std::time::Instant::now())
    }
}
impl Clock for SystemClock {
    fn mono_ns(&self) -> u64 {
        self.0.elapsed().as_nanos() as u64
    }
    fn wall_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
}

pub struct CtrlService {
    /// `Err` = the store didn't open (damaged or unreadable): every channel answers that error, nothing writes.
    store: Mutex<Result<Store, StoreError>>,
    clock: Box<dyn Clock>,
    scorer: Box<dyn Scorer>,
    /// The ctrl helper. `None` (or down): `ctrl:do` answers unavailable; history still reads.
    helper: Option<Arc<HelperClient>>,
    /// run id → readable labels. Memory only.
    labels: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    /// The run being driven right now.
    active: Mutex<Option<String>>,
    stop: AtomicBool,
    /// (do id, run id) of gates that started waiting, for the shell's non-activating notification.
    notices: Mutex<Vec<(String, String)>>,
}

fn str_field<'a>(p: &'a Value, k: &str) -> Result<&'a str, CtrlError> {
    p[k].as_str()
        .ok_or_else(|| CtrlError::new(CtrlErrorCode::Validation, format!("{k} is required")))
}

/// What a run shows.
pub fn run_status(s: &CtrlState) -> &'static str {
    match s.status {
        Some(TaskStatus::Done) => "done",
        Some(TaskStatus::Failed) => "failed",
        Some(TaskStatus::Cancelled) => "cancelled",
        Some(TaskStatus::AskedOut) => "asked-out",
        Some(TaskStatus::Stopped) => "stopped",
        Some(TaskStatus::OutOfSteps) => "out-of-steps",
        None if s.ask.is_some() => "asking",
        None if s.confirm.as_ref().is_some_and(|g| g.answer.is_none()) => "needs-confirm",
        None => "running",
    }
}

fn stop_str(r: StopReason) -> &'static str {
    match r {
        StopReason::StopKey => "stop-key",
        StopReason::UserInput => "user-input",
        StopReason::StopButton => "stop-button",
        StopReason::NoWatcher => "no-watcher",
    }
}

fn node_str(n: Node) -> &'static str {
    match n {
        Node::ChooseApp => "choose-app",
        Node::ChooseMove => "choose-move",
        Node::ChooseElement => "choose-element",
        Node::ChooseText => "choose-text",
        Node::Verify => "verify",
    }
}

fn question(n: Node) -> &'static str {
    match n {
        Node::ChooseApp => "which app should do it?",
        Node::ChooseMove => "what should ctrl do next?",
        Node::ChooseElement => "which one?",
        Node::ChooseText => "what should ctrl type?",
        Node::Verify => "is it done?",
    }
}

/// A readable label for an option (lowercase): from the run's label cache and the goal, never from the log.
pub fn option_label(o: &crate::decide::Opt, labels: &BTreeMap<String, String>, goal: &str) -> String {
    let id = o.id.as_str();
    let l = |k: &str| labels.get(k).map(|s| s.to_lowercase());
    let role = || {
        o.role
            .clone()
            .unwrap_or_else(|| "element".into())
            .replace('_', " ")
    };
    let text = || {
        o.label_ref
            .as_deref()
            .and_then(|r| resolve(r, goal))
            .unwrap_or_default()
    };
    if id == "none" {
        return "none of these".into();
    }
    if let Some(a) = id.strip_prefix("app.") {
        return format!("open {}", l(a).unwrap_or_else(|| a.to_lowercase()));
    }
    if let Some(h) = id.strip_prefix("menu.") {
        return format!("{} (menu)", l(h).unwrap_or_else(|| "a menu item".into()));
    }
    if let Some(c) = id.strip_prefix("key.") {
        let named = crate::act::BUILTIN_SHORTCUTS.iter().find(|(k, _, _)| *k == c);
        return match named {
            Some((_, n, _)) => format!("{n} ({c})"),
            None => format!("press {c}"),
        };
    }
    if id.starts_with("url.") {
        return format!("open {}", text());
    }
    if let Some(h) = id.strip_prefix("branch.") {
        return format!("look in {}", l(h).unwrap_or_else(role));
    }
    if let Some(g) = id.strip_prefix("group.") {
        let (r, p) = g.rsplit_once('~').unwrap_or((g, "0"));
        let page: u32 = p.parse().unwrap_or(0);
        return format!("{}s, page {}", r.replace('_', " "), page + 1);
    }
    if let Some(rest) = id.strip_prefix("ax.") {
        let (h, act) = rest.rsplit_once('.').unwrap_or((rest, "press"));
        let what = l(h).unwrap_or_else(role);
        return match act {
            "focus" => format!("click into {what}"),
            "show_menu" => format!("open the menu of {what}"),
            "scroll_down" => format!("scroll down {what}"),
            "scroll_up" => format!("scroll up {what}"),
            a => format!("{a} {what}"),
        };
    }
    if id.starts_with("text.") {
        let go = if id.ends_with(".go") {
            " and press return"
        } else {
            ""
        };
        return format!("type “{}”{go}", text());
    }
    match id {
        "verify.done" => "it's done".into(),
        "verify.continue" => "keep going".into(),
        "verify.failed" => "that didn't work".into(),
        _ => id.into(),
    }
}

fn action_label(a: &crate::act::Action, labels: &BTreeMap<String, String>, goal: &str) -> String {
    use crate::act::Action as A;
    let l = |k: &str| {
        labels
            .get(k)
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| "an element".into())
    };
    match a {
        A::OpenApp { app } => format!(
            "open {}",
            labels.get(app).map_or(app.to_lowercase(), |s| s.to_lowercase())
        ),
        A::Press { handle } => format!("press {}", l(handle)),
        A::SetValue { text_ref, submit, .. } => format!(
            "type “{}”{}",
            resolve(text_ref, goal).unwrap_or_default(),
            if *submit { " and press return" } else { "" }
        ),
        A::Increment { handle } => format!("increase {}", l(handle)),
        A::Decrement { handle } => format!("decrease {}", l(handle)),
        A::ShowMenu { handle } => format!("open the menu of {}", l(handle)),
        A::Focus { handle } => format!("click into {}", l(handle)),
        A::Scroll { handle, .. } => format!("scroll {}", l(handle)),
        A::KeyCombo { combo, .. } => format!("press {combo}"),
        A::OpenUrl { url_ref, .. } => format!("open {}", resolve(url_ref, goal).unwrap_or_default()),
    }
}

impl CtrlService {
    /// Open ctrl do's store under `dir` (`<app data>/ctrl`; the store itself is `dir/do`).
    pub fn open(dir: impl Into<std::path::PathBuf>, clock: Box<dyn Clock>) -> Self {
        CtrlService {
            store: Mutex::new(Store::open(dir.into().join("do"))),
            clock,
            scorer: Box::new(Heuristic),
            helper: None,
            labels: Mutex::new(BTreeMap::new()),
            active: Mutex::new(None),
            stop: AtomicBool::new(false),
            notices: Mutex::new(Vec::new()),
        }
    }

    pub fn with_helper(mut self, helper: Arc<HelperClient>) -> Self {
        self.helper = Some(helper);
        self
    }

    /// The chain's scorer (default: the heuristic layer; see `laya::select_scorer`).
    pub fn with_scorer(mut self, scorer: Box<dyn Scorer>) -> Self {
        self.scorer = scorer;
        self
    }

    fn live_helper(&self) -> Option<Arc<HelperClient>> {
        self.helper.clone().filter(|h| h.is_up())
    }

    /// (do id, run id) of the gates that started waiting since the last call.
    pub fn take_confirm_notices(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.notices.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn with_store<R>(&self, f: impl FnOnce(&mut Store) -> Result<R, CtrlError>) -> Result<R, CtrlError> {
        let mut g = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let s = g.as_mut().map_err(|e| CtrlError::from(e.clone()))?;
        f(s)
    }

    fn load(&self, run_id: &str) -> Result<(EventLog, String, String), CtrlError> {
        self.with_store(|s| {
            let log = s.run(run_id)?;
            let do_id = s.run_meta(run_id).map(|m| m.0.clone()).unwrap_or_default();
            let text = s.do_entry(&do_id)?.text.clone();
            Ok((log, do_id, text))
        })
    }

    fn append(&self, log: &mut EventLog, e: Event) -> Result<(), CtrlError> {
        let (m, w) = (self.clock.mono_ns(), self.clock.wall_ms());
        self.with_store(|s| s.append_run_record(log, m, w, e).map_err(CtrlError::from))
    }

    /// Drive a run (outside the store lock) and write what it appended.
    fn drive(&self, log: &mut EventLog, do_id: &str, goal: &str) -> Result<(), CtrlError> {
        let Some(helper) = self.live_helper() else {
            return Err(CtrlError::new(
                CtrlErrorCode::Unavailable,
                "ctrl's helper isn't running",
            ));
        };
        {
            let mut a = self.active.lock().unwrap_or_else(|e| e.into_inner());
            if a.is_some() {
                return Err(CtrlError::new(
                    CtrlErrorCode::Validation,
                    "ctrl is already doing something",
                ));
            }
            *a = Some(log.run_id().to_string());
        }
        self.stop.store(false, Ordering::SeqCst);
        let from = log.records().len();
        let mut labels = self
            .labels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(log.run_id())
            .unwrap_or_default();
        let step = Driver {
            log,
            helper: &helper,
            scorer: self.scorer.as_ref(),
            clock: self.clock.as_ref(),
            goal,
            labels: &mut labels,
            stop: &self.stop,
        }
        .drive();
        self.labels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(log.run_id().to_string(), labels);
        *self.active.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if step == Step::Confirming {
            self.notices
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((do_id.to_string(), log.run_id().to_string()));
        }
        self.with_store(|s| s.persist_tail(log, from).map_err(CtrlError::from))
    }

    /// The run's view (`ctrl:run`): the state after `at` records, with labels from memory and the goal.
    fn run_view(&self, log: &EventLog, do_id: &str, text: &str, at: Option<u64>) -> Value {
        let total = log.records().len();
        let at = at.map_or(total, |a| (a as usize).min(total));
        let s = log.state_at(at);
        let all = self.labels.lock().unwrap_or_else(|e| e.into_inner());
        let empty = BTreeMap::new();
        let labels = all.get(log.run_id()).unwrap_or(&empty);
        let decisions: Vec<Value> = s
            .decisions
            .iter()
            .map(|d| {
                let chosen = d.options.iter().find(|o| o.id == d.chosen);
                json!({
                    "id": d.id,
                    "node": node_str(d.node),
                    "question": question(d.node),
                    "chosen": d.chosen,
                    "chosenLabel": chosen.map(|o| option_label(o, labels, text)).unwrap_or_default(),
                    "p": d.p_chosen_bp as f64 / 10_000.0,
                    "outcome": format!("{:?}", d.outcome).to_lowercase(),
                    "answered": d.answered,
                    "backtracks": d.backtracks,
                    "model": d.model.to_string(),
                    "options": d.options.len(),
                })
            })
            .collect();
        let ask = s
            .ask
            .as_ref()
            .and_then(|id| s.decisions.iter().find(|d| &d.id == id))
            .map(|d| {
                json!({
                    "decisionId": d.id,
                    "node": node_str(d.node),
                    "question": question(d.node),
                    "options": ask_options(d).into_iter().map(|i| json!({
                        "id": d.options[i].id,
                        "label": option_label(&d.options[i], labels, text),
                        "p": d.p_bp[i] as f64 / 10_000.0,
                        "gated": d.options[i].gated,
                    })).collect::<Vec<_>>(),
                })
            });
        let confirm = s.confirm.as_ref().filter(|g| g.answer.is_none()).map(|g| {
            let action = match &s.phase {
                crate::log::Phase::Act { action } => action_label(action, labels, text),
                _ => String::new(),
            };
            json!({
                "gateId": g.id,
                "rules": g.rules,
                "action": action,
                "app": labels.get(&g.binding.app).map_or(g.binding.app.to_lowercase(), |l| l.to_lowercase()),
            })
        });
        let actions: Vec<Value> = s
            .actions
            .iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "kind": a.action.kind(),
                    "label": action_label(&a.action, labels, text),
                    "status": match a.status {
                        ActionStatus::Requested => "requested",
                        ActionStatus::Refused => "refused",
                        ActionStatus::Done => "done",
                        ActionStatus::Failed => "failed",
                    },
                    "err": a.err,
                })
            })
            .collect();
        let started = log.records().first().map_or(0, |r| r.t_wall_ms);
        // The timeline: kinds only (`decision.made`, `action.executed`), never content.
        let events: Vec<Value> = log
            .records()
            .iter()
            .map(|r| {
                let kind = serde_json::to_value(&r.event)
                    .ok()
                    .and_then(|v| v["kind"].as_str().map(str::to_string))
                    .unwrap_or_default();
                json!({ "seq": r.seq, "kind": kind })
            })
            .collect();
        json!({
            "runId": log.run_id(),
            "doId": do_id,
            "text": text,
            "status": run_status(&s),
            "stop": s.stop.map(stop_str),
            "app": s.app.as_ref().map(|a| labels.get(a).map_or(a.to_lowercase(), |l| l.to_lowercase())),
            "startedAt": started,
            "at": at,
            "total": total,
            "events": events,
            "decisions": decisions,
            "actions": actions,
            "ask": ask,
            "confirm": confirm,
            "backtracks": s.backtracks,
        })
    }

    fn summary(&self, store: &Store, run_id: &str) -> Option<Value> {
        let log = store.run(run_id).ok()?;
        let (do_id, at) = store.run_meta(run_id)?.clone();
        let text = store.do_entry(&do_id).ok()?.text.clone();
        let s = log.state();
        Some(json!({
            "runId": run_id,
            "doId": do_id,
            "text": text,
            "status": run_status(&s),
            "startedAt": at,
            "actions": s.executed(),
        }))
    }

    /// One `ctrl:*` call.
    pub fn call(&self, channel: &str, p: Value) -> Result<Value, CtrlError> {
        match channel {
            "ctrl:runs" => self.with_store(|s| {
                let mut out = Vec::new();
                for d in s.dos() {
                    for r in d.runs.iter().rev() {
                        if let Some(v) = self.summary(s, r) {
                            out.push(v);
                        }
                    }
                }
                Ok(Value::Array(out))
            }),
            "ctrl:run" => {
                let (log, do_id, text) = self.load(str_field(&p, "runId")?)?;
                Ok(self.run_view(&log, &do_id, &text, p["at"].as_u64()))
            }
            "ctrl:do" => {
                if self.live_helper().is_none() {
                    return Err(CtrlError::new(
                        CtrlErrorCode::Unavailable,
                        "ctrl's helper isn't running, so ctrl can't act right now",
                    ));
                }
                if self.active.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
                    return Err(CtrlError::new(
                        CtrlErrorCode::Validation,
                        "ctrl is already doing something",
                    ));
                }
                let text = str_field(&p, "text")?.to_string();
                let (m, now) = (self.clock.mono_ns(), self.clock.wall_ms());
                // A run still waiting on Jared is cancelled by a new command.
                // (Only the latest run can be unfinished: every earlier one was ended this way.)
                let waiting: Vec<String> = self.with_store(|s| {
                    Ok(s.dos()
                        .first()
                        .and_then(|d| d.runs.last())
                        .filter(|r| s.run(r).is_ok_and(|l| l.state().status.is_none()))
                        .cloned()
                        .into_iter()
                        .collect())
                })?;
                for r in waiting {
                    let (mut log, _, _) = self.load(&r)?;
                    self.append(
                        &mut log,
                        Event::TaskFinished {
                            status: TaskStatus::Cancelled,
                            stop: None,
                        },
                    )?;
                }
                let (mut log, do_id) = self.with_store(|s| {
                    let d = s.create_do(now, &text)?;
                    let log = s.start_run(now, &d.id, &[(m, Event::DoStarted { do_id: d.id.clone() })])?;
                    Ok((log, d.id))
                })?;
                let text = self.with_store(|s| Ok(s.do_entry(&do_id)?.text.clone()))?;
                self.drive(&mut log, &do_id, &text)?;
                Ok(self.run_view(&log, &do_id, &text, None))
            }
            "ctrl:answer" => {
                let (mut log, do_id, text) = self.load(str_field(&p, "runId")?)?;
                let decision_id = str_field(&p, "decisionId")?.to_string();
                let s = log.state();
                let Some(d) = s
                    .decisions
                    .iter()
                    .find(|d| d.id == decision_id && s.ask.as_ref() == Some(&d.id))
                else {
                    return Err(CtrlError::new(
                        CtrlErrorCode::Validation,
                        "that question was already answered",
                    ));
                };
                let choice = match p["optionId"].as_str() {
                    None => None,
                    Some(o) if d.options.iter().any(|x| x.id == o) => Some(o.to_string()),
                    Some(_) => {
                        return Err(CtrlError::new(
                            CtrlErrorCode::Validation,
                            "that isn't one of the choices",
                        ))
                    }
                };
                let cancel = choice.is_none();
                self.append(&mut log, Event::DecisionAnswered { decision_id, choice })?;
                if cancel {
                    self.append(
                        &mut log,
                        Event::TaskFinished {
                            status: TaskStatus::Cancelled,
                            stop: None,
                        },
                    )?;
                } else {
                    self.drive(&mut log, &do_id, &text)?;
                }
                Ok(self.run_view(&log, &do_id, &text, None))
            }
            "ctrl:confirm" => {
                let (mut log, do_id, text) = self.load(str_field(&p, "runId")?)?;
                let gate_id = str_field(&p, "gateId")?.to_string();
                let s = log.state();
                if !s
                    .confirm
                    .as_ref()
                    .is_some_and(|g| g.id == gate_id && g.answer.is_none())
                {
                    return Err(CtrlError::new(
                        CtrlErrorCode::Validation,
                        "that request was already answered",
                    ));
                }
                let answer = match str_field(&p, "answer")? {
                    "approve" => ConfirmAnswer::Approve,
                    "deny" => ConfirmAnswer::Deny,
                    _ => {
                        return Err(CtrlError::new(
                            CtrlErrorCode::Validation,
                            "answer approve or deny",
                        ))
                    }
                };
                self.append(&mut log, Event::ConfirmResolved { gate_id, answer })?;
                match answer {
                    ConfirmAnswer::Approve => self.drive(&mut log, &do_id, &text)?,
                    ConfirmAnswer::Deny => self.append(
                        &mut log,
                        Event::TaskFinished {
                            status: TaskStatus::Cancelled,
                            stop: None,
                        },
                    )?,
                }
                Ok(self.run_view(&log, &do_id, &text, None))
            }
            "ctrl:stop" => {
                let run_id = str_field(&p, "runId")?.to_string();
                let driving = self.active.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if driving.as_deref() == Some(run_id.as_str()) {
                    self.stop.store(true, Ordering::SeqCst);
                    return Ok(json!({ "runId": run_id, "status": "stopping" }));
                }
                let (mut log, do_id, text) = self.load(&run_id)?;
                if log.state().status.is_none() {
                    self.append(
                        &mut log,
                        Event::TaskFinished {
                            status: TaskStatus::Stopped,
                            stop: Some(StopReason::StopButton),
                        },
                    )?;
                }
                let view = self.run_view(&log, &do_id, &text, None);
                Ok(json!({ "runId": run_id, "status": view["status"] }))
            }
            _ => Err(CtrlError::new(
                CtrlErrorCode::Unavailable,
                format!("{channel} is not a ctrl channel"),
            )),
        }
    }
}
