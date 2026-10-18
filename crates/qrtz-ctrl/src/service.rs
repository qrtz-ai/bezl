//! The `ctrl:*` channels (slice 2) and the in-process scheduler ticker (slice 3, ADR 0105). Payloads arrive already
//! validated against the IPC contract (`packages/core/src/ipc/contract.ts`); answers are the contract's shapes
//! (camelCase). Nothing here executes a task yet: a due run (manual or scheduled) records `task.started` +
//! `intent.pending`, then the decision layers (slice 4, ADR 0106) observe the app catalog and decide the intent
//! (`decision.made` … `plan.produced`, or an ask the user answers through `ctrl:answer`). With a helper connected
//! (slice 5, ADR 0108) the catalog comes from it (running apps, the real default browser) and the plan executes
//! through the ctrl helper protocol (`exec::Driver`): routes, guard, verify, backtrack. Irreversible plans stop at
//! the confirm gate (slice 6, ADR 0111): `needs-confirm` until `ctrl:confirm` approves (once, or remembered for the
//! task), denies, or the gate expires on the scheduler's tick; `ctrl:revoke-approval` drops a remembered one.
use crate::decide::{ask_options, run_chain, Goal, Heuristic, Scorer};
use crate::exec::{Driver, HoldReason, Sink, VerifyOutcome};
use crate::files::{plan_undo, run_undo, UndoSkip, UndoWhy};
use crate::gate::{needs_confirm, ConfirmAnswer, ExpiredWhy, CONFIRM_TTL_MS};
use crate::helper::{HelperApps, HelperClient};
use crate::ladder::{FsOp, Route};
use crate::layers::{label, AppSource, ScanApps};
use crate::log::{fold, ActionStatus, CtrlState, DecisionState, Event, EventLog, TaskStatus};
use crate::runner::Clock;
use crate::schedule::{is_due, next_due, LocalZone, SystemLocalZone};
use crate::store::{Store, StoreError};
use crate::task::{Schedule, Task};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
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
            StoreError::NotFound => CtrlError::new(CtrlErrorCode::NotFound, "that task or run is gone"),
            StoreError::Invalid(t) => CtrlError::new(CtrlErrorCode::Validation, t.message()),
            StoreError::Damaged { file, line } => CtrlError::new(
                CtrlErrorCode::Internal,
                format!("ctrl's history file {file} is damaged at line {line}; nothing was changed"),
            ),
            StoreError::Io(k) => CtrlError::new(
                CtrlErrorCode::Internal,
                format!("couldn't save ctrl's tasks ({k})"),
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
    clock: Box<dyn Clock + Send + Sync>,
    zone: Box<dyn LocalZone>,
    apps: Box<dyn AppSource>,
    scorer: Box<dyn Scorer>,
    /// The ctrl helper (slice 5). `None`: runs stop at `planned` (nothing executes).
    helper: Option<Arc<HelperClient>>,
    /// Where file routes act (slice 7, ADR 0113): `~/Downloads` + `~/.Trash` on macOS, a temp dir in tests.
    files: Option<crate::files::FileRoots>,
    /// Gate preview item key → readable title (slice 6). Memory only: titles never reach a file.
    labels: Mutex<BTreeMap<String, String>>,
    /// Runs whose gate waits for the user; `None` until the first tick scanned the store.
    waiting: Mutex<Option<BTreeSet<String>>>,
    /// (task id, run id) of gates that started waiting, for the shell's non-activating notification.
    notices: Mutex<Vec<(String, String)>>,
}

/// Writes the driver's events to the run's file as they happen.
struct StoreSink<'a>(&'a Store, u64, &'a dyn Clock, &'a CtrlService, &'a str);
impl Sink for StoreSink<'_> {
    fn labels(&mut self, labels: Vec<(String, String)>) {
        self.3
            .labels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(labels);
    }
    fn confirm_waiting(&mut self, _gate_id: &str) {
        let run_id = self.4.to_string();
        let task_id = self.0.run_meta(&run_id).map(|m| m.0.clone()).unwrap_or_default();
        if let Some(w) = self.3.waiting.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            w.insert(run_id.clone());
        }
        self.3
            .notices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((task_id, run_id));
    }
    fn push(&mut self, log: &mut EventLog, e: Event) -> Result<(), String> {
        self.0
            .append_run_record(log, self.2.mono_ns(), self.1, e)
            .map_err(|e| format!("{e:?}"))
    }
    fn flush(&mut self, log: &EventLog, from: usize) -> Result<(), String> {
        self.0.persist_tail(log, from).map_err(|e| format!("{e:?}"))
    }
}

/// The status a run shows: its `task.finished`, else asking the user, else waiting for an intent, else planned
/// (a plan and no action yet: the executor is slice 5), else running.
pub fn run_status(s: &CtrlState) -> &'static str {
    match s.task_status {
        Some(TaskStatus::Done) => "done",
        Some(TaskStatus::Failed) => "failed",
        Some(TaskStatus::Cancelled) => "cancelled",
        Some(TaskStatus::AskedOut) => "asked-out",
        Some(TaskStatus::Deferred) => "deferred",
        Some(TaskStatus::Expired) => "expired",
        None if s.ask.is_some() => "asking",
        None if needs_confirm(s) => "needs-confirm",
        None if s.held == Some(HoldReason::Irreversible) => "needs-confirm",
        None if s.intent_pending => "pending-intent",
        None if s.plan.is_some() && s.actions.is_empty() => "planned",
        None => "running",
    }
}

fn action_status(a: ActionStatus) -> &'static str {
    match a {
        ActionStatus::Requested => "requested",
        ActionStatus::Executing => "executing",
        ActionStatus::Deferred => "deferred",
        ActionStatus::Asked => "asked",
        ActionStatus::Refused => "refused",
        ActionStatus::Done => "done",
        ActionStatus::Failed => "failed",
    }
}

fn event_action(e: &Event) -> Option<&str> {
    match e {
        Event::ActionRequested { action_id, .. }
        | Event::ActionDecided { action_id, .. }
        | Event::GuardSnapshot { action_id, .. }
        | Event::ActionExecuted { action_id, .. }
        | Event::GuardResult { action_id, .. }
        | Event::GuardRestored { action_id, .. } => Some(action_id),
        Event::TaskStarted { .. }
        | Event::IntentPending { .. }
        | Event::TaskFinished { .. }
        | Event::ObsApps { .. }
        | Event::DecisionMade { .. }
        | Event::DecisionAnswered { .. }
        | Event::Backtrack { .. }
        | Event::PlanProduced { .. }
        | Event::ObsSnapshot { .. }
        | Event::ExecHeld { .. }
        | Event::ConfirmRequested { .. }
        | Event::ConfirmResolved { .. }
        | Event::ConfirmReverified { .. } => None,
        Event::ObsDiff { action_id, .. }
        | Event::VerifyResult { action_id, .. }
        | Event::UndoRefused { action_id, .. }
        | Event::UndoDone { action_id, .. } => Some(action_id),
    }
}

/// The run's finished file batch (ADR 0117): the last `files` action that executed ok, in the first `n` events.
fn done_batch(log: &EventLog, n: usize) -> Option<(String, FsOp)> {
    let st = fold(log.events().take(n));
    log.events()
        .take(n)
        .filter_map(|e| match e {
            Event::ActionRequested { action_id, routes } => routes.iter().find_map(|r| match r {
                // Emptying the Trash (ADR 0122) has nothing to put back.
                Route::Files(op @ FsOp::Batch { ops })
                    if !ops.iter().any(|o| matches!(o, FsOp::Delete { .. })) =>
                {
                    Some((action_id.clone(), op.clone()))
                }
                _ => None,
            }),
            _ => None,
        })
        .filter(|(id, _)| st.actions.get(id) == Some(&ActionStatus::Done))
        .last()
}

fn why_name(w: UndoWhy) -> &'static str {
    match w {
        UndoWhy::Moved => "moved",
        UndoWhy::Occupied => "occupied",
    }
}

fn skipped_json(skipped: &[UndoSkip], labels: &BTreeMap<String, String>) -> Vec<Value> {
    skipped
        .iter()
        .map(|k| {
            json!({
                "key": k.file,
                "label": labels.get(&k.file).map(|l| l.to_lowercase()).unwrap_or_else(|| "a file ctrl moved".into()),
                "why": why_name(k.why),
            })
        })
        .collect()
}

fn event_node(e: &Event) -> Option<&'static str> {
    match e {
        Event::DecisionMade { node, .. } => Some(node.as_str()),
        _ => None,
    }
}

fn pct(bp: u32) -> f64 {
    f64::from(bp) / 10000.0
}

fn option_json(d: &DecisionState, i: usize, goal: Option<&Goal>) -> Value {
    let o = &d.options[i];
    json!({
        "id": o.id,
        "label": label(o, goal),
        "p": pct(d.p_bp[i]),
        "irreversible": o.irreversible,
        "refusedInBackground": o.ladder.is_some(),
    })
}

fn decision_json(d: &DecisionState, goal: Option<&Goal>) -> Value {
    let outcome = match d.outcome {
        crate::decide::Outcome::Act => "act",
        crate::decide::Outcome::Tentative => "tentative",
        crate::decide::Outcome::Ask => "ask",
    };
    let chosen = d.options.iter().find(|o| o.id == d.chosen);
    json!({
        "id": d.id,
        "node": d.node.as_str(),
        "outcome": outcome,
        "answered": d.answered,
        "chosen": d.chosen,
        "chosenLabel": chosen.map_or_else(|| d.chosen.clone(), |o| label(o, goal)),
        "p": pct(d.p_chosen_bp),
        "model": format!("{} {}", d.model.id, d.model.version),
        "options": (0..d.options.len()).map(|i| option_json(d, i, goal)).collect::<Vec<_>>(),
    })
}

fn event_kind(e: &Event) -> String {
    serde_json::to_value(e)
        .ok()
        .and_then(|v| v["kind"].as_str().map(str::to_string))
        .unwrap_or_default()
}

fn chosen_label(s: &CtrlState, node: crate::decide::Node, goal: Option<&Goal>) -> Option<String> {
    let d = s.decisions.iter().rev().find(|d| d.node == node)?;
    d.options
        .iter()
        .find(|o| o.id == d.chosen)
        .map(|o| label(o, goal))
}

fn answer_name(a: &ConfirmAnswer) -> &'static str {
    match a {
        ConfirmAnswer::Approve => "approved",
        ConfirmAnswer::Deny => "denied",
        ConfirmAnswer::ApproveRemember { .. } => "remembered",
        ConfirmAnswer::Standing { .. } => "standing",
        ConfirmAnswer::Expired { .. } => "expired",
    }
}

/// The confirm card's view: what will happen, item by item (titles from memory, else `window 2`).
fn confirm_json(s: &CtrlState, goal: Option<&Goal>, labels: &BTreeMap<String, String>) -> Value {
    let Some(g) = s.confirm.as_ref() else {
        return Value::Null;
    };
    use crate::decide::Node;
    let kind = |k: crate::gate::ItemKind| match k {
        crate::gate::ItemKind::Window => "window",
        crate::gate::ItemKind::Tab => "tab",
        crate::gate::ItemKind::File => "file",
        crate::gate::ItemKind::App => "app",
        crate::gate::ItemKind::Action => "action",
    };
    let action = chosen_label(s, Node::ChooseIntent, goal)
        .unwrap_or_else(|| "do something that can't be undone".into());
    let items: Vec<Value> = g
        .preview
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let fallback = match it.kind {
                crate::gate::ItemKind::Action => action.clone(),
                k => format!("{} {}", kind(k), i + 1),
            };
            json!({
                "key": it.key,
                "kind": kind(it.kind),
                "label": labels.get(&it.key).map_or(fallback, |t| t.to_lowercase()),
                "unsaved": it.unsaved,
            })
        })
        .collect();
    json!({
        "gateId": g.id,
        "action": action,
        "app": chosen_label(s, Node::ChooseApp, goal),
        "move": chosen_label(s, Node::ChooseMove, goal),
        "items": items,
        "complete": g.preview.complete,
        "rules": g.rules,
        "answer": g.answer.as_ref().map(answer_name),
        "reverified": g.reverified,
        "used": g.used,
    })
}

fn state_json(s: &CtrlState, goal: Option<&Goal>) -> Value {
    let ask = s
        .ask
        .as_ref()
        .and_then(|id| s.decisions.iter().find(|d| &d.id == id))
        .map(|d| {
            json!({
                "decisionId": d.id,
                "node": d.node.as_str(),
                "options": ask_options(d).into_iter().map(|i| option_json(d, i, goal)).collect::<Vec<_>>(),
            })
        });
    let plan = s.plan.as_ref().map(|p| {
        json!({
            "irreversible": p.irreversible,
            "steps": p.steps.iter().map(|st| {
                let o = s.decisions.iter().filter(|d| d.node == st.node).flat_map(|d| d.options.iter()).find(|o| o.id == st.option);
                json!({"node": st.node.as_str(), "option": st.option, "label": o.map_or_else(|| st.option.clone(), |o| label(o, goal))})
            }).collect::<Vec<_>>(),
        })
    });
    json!({
        "decisions": s.decisions.iter().map(|d| decision_json(d, goal)).collect::<Vec<_>>(),
        "plan": plan,
        "ask": ask,
        "actions": s.actions.iter().map(|(id, st)| json!({"id": id, "status": action_status(*st)})).collect::<Vec<_>>(),
        "violations": s.violations,
        "inconclusive": s.inconclusive,
        "quarantined": s.quarantined.iter().map(|(app, rung)| json!({"app": app, "rung": rung})).collect::<Vec<_>>(),
        "status": run_status(s),
        "confirm": Value::Null,
        "undo": Value::Null,
        "held": s.held.map(|h| match h {
            HoldReason::Irreversible => "irreversible",
            HoldReason::FilesNotYet => "files-not-yet",
            HoldReason::NoRoute => "no-route",
            HoldReason::UnknownApp => "unknown-app",
            HoldReason::FolderNotYet => "folder-not-yet",
        }),
        "verified": s.verified.iter().map(|(id, v)| json!({"actionId": id, "outcome": match v {
            VerifyOutcome::Passed => "passed",
            VerifyOutcome::Failed => "failed",
            VerifyOutcome::Unverifiable => "unverifiable",
        }})).collect::<Vec<_>>(),
    })
}

fn str_field<'a>(p: &'a Value, k: &str) -> Result<&'a str, CtrlError> {
    p[k].as_str()
        .ok_or_else(|| CtrlError::new(CtrlErrorCode::Validation, format!("{k} is required")))
}

impl CtrlService {
    pub fn open(dir: impl Into<std::path::PathBuf>, clock: Box<dyn Clock + Send + Sync>) -> Self {
        Self::open_with_zone(dir, clock, Box::new(SystemLocalZone))
    }

    pub fn open_with_zone(
        dir: impl Into<std::path::PathBuf>,
        clock: Box<dyn Clock + Send + Sync>,
        zone: Box<dyn LocalZone>,
    ) -> Self {
        CtrlService {
            store: Mutex::new(Store::open(dir)),
            clock,
            zone,
            apps: Box::new(ScanApps::mac_default()),
            scorer: Box::new(Heuristic),
            helper: None,
            files: crate::files::FileRoots::default_for_os(),
            labels: Mutex::new(BTreeMap::new()),
            waiting: Mutex::new(None),
            notices: Mutex::new(Vec::new()),
        }
    }

    /// Connect the ctrl helper: the app catalog comes from it and plans execute through it (slice 5).
    /// Act on these folders for file routes (slice 7).
    pub fn with_files(mut self, roots: crate::files::FileRoots) -> Self {
        self.files = Some(roots);
        self
    }

    pub fn with_helper(mut self, helper: Arc<HelperClient>) -> Self {
        self.helper = Some(helper);
        self
    }

    /// The connected helper if it is up right now (slice 5b): a restarting helper process means the run
    /// behaves as without one (stops at `planned`) instead of failing every call.
    fn live_helper(&self) -> Option<&Arc<HelperClient>> {
        self.helper.as_ref().filter(|h| h.is_up())
    }

    /// Where the decision layers get the app catalog (default: the application folders).
    pub fn with_apps(mut self, apps: Box<dyn AppSource>) -> Self {
        self.apps = apps;
        self
    }

    /// The decision layers' scorer (default: the heuristic layer; see `laya::select_scorer`).
    pub fn with_scorer(mut self, scorer: Box<dyn Scorer>) -> Self {
        self.scorer = scorer;
        self
    }

    fn last_run_at(store: &Store, t: &Task) -> Option<u64> {
        t.runs.last().and_then(|r| store.run_meta(r)).map(|m| m.1)
    }

    fn run_json(store: &Store, log: &EventLog) -> Value {
        let s = log.state();
        let (task_id, at) = store.run_meta(log.run_id()).cloned().unwrap_or_default();
        json!({
            "id": log.run_id(),
            "taskId": task_id,
            "at": at,
            "status": run_status(&s),
            "events": log.records().len(),
            "violations": s.violations,
        })
    }

    fn task_json(&self, store: &Store, t: &Task) -> Value {
        let last = Self::last_run_at(store, t);
        let next = if t.paused {
            None
        } else {
            next_due(&t.schedule, t.created_at, last, self.zone.as_ref())
        };
        json!({
            "id": t.id,
            "text": t.text,
            "schedule": t.schedule,
            "paused": t.paused,
            "createdAt": t.created_at,
            "runs": t.runs.len(),
            "lastRunAt": last,
            "nextRunAt": next,
            "approvals": t.approvals.iter().map(|a| json!({
                "id": a.id,
                "intent": a.scope.intent,
                "move": a.scope.mv,
                "app": a.scope.app,
                "items": a.scope.items,
                "unsaved": a.scope.unsaved,
                "grantedAt": a.granted_at,
            })).collect::<Vec<_>>(),
        })
    }

    /// Record a run the same way for `ctrl:run-now` and a scheduled fire: `run.recorded` + `task.started` +
    /// `intent.pending`, then `obs.apps` and the decision chain (ADR 0106).
    fn record_pending_run(&self, store: &mut Store, now: u64, task_id: &str) -> Result<EventLog, CtrlError> {
        // A newer run replaces a gate still waiting from an older run of the same task (it never acts later).
        if let Some(prev) = store.task(task_id)?.runs.last().cloned() {
            self.expire_gate(store, &prev, now, ExpiredWhy::Superseded)?;
        }
        let mono = self.clock.mono_ns();
        let id = task_id.to_string();
        let mut log = store.start_run(
            now,
            task_id,
            &[
                (mono, Event::TaskStarted { task_id: id.clone() }),
                (mono, Event::IntentPending { task_id: id.clone() }),
            ],
        )?;
        let apps = match self.live_helper() {
            Some(h) => HelperApps(h).catalog().observe(),
            None => self.apps.catalog().observe(),
        };
        store.append_run_record(&mut log, self.clock.mono_ns(), now, Event::ObsApps { apps })?;
        let text = store.task(task_id)?.text.clone();
        self.continue_chain(store, &mut log, now, task_id, &text)?;
        Ok(log)
    }

    /// Run the chain from the log's state and append what it decides.
    fn continue_chain(
        &self,
        store: &Store,
        log: &mut EventLog,
        now: u64,
        task_id: &str,
        text: &str,
    ) -> Result<(), CtrlError> {
        let goal = Goal { task_id, text };
        if let Some(h) = self.live_helper() {
            let standing = store
                .task(task_id)
                .map(|t| t.approvals.clone())
                .unwrap_or_default();
            let driver = Driver {
                helper: h,
                scorer: self.scorer.as_ref(),
                clock: self.clock.as_ref(),
                standing: &standing,
                files: self.files.as_ref(),
            };
            let run_id = log.run_id().to_string();
            let mut sink = StoreSink(store, now, self.clock.as_ref(), self, &run_id);
            return driver.drive(&goal, log, &mut sink).map_err(|e| {
                CtrlError::new(CtrlErrorCode::Internal, format!("couldn't save ctrl's run ({e})"))
            });
        }
        for e in run_chain(&goal, &log.state(), self.scorer.as_ref(), self.clock.as_ref()) {
            store.append_run_record(log, self.clock.mono_ns(), now, e)?;
        }
        Ok(())
    }

    /// End a run whose gate still waits (`confirm.resolved {expired}` + `task.finished {expired}`). Nothing acts.
    fn expire_gate(&self, store: &Store, run_id: &str, now: u64, why: ExpiredWhy) -> Result<bool, CtrlError> {
        let mut log = store.run(run_id)?;
        let s = log.state();
        let (Some(g), true) = (s.confirm.as_ref(), needs_confirm(&s)) else {
            return Ok(false);
        };
        let task_id = store.run_meta(run_id).map(|m| m.0.clone()).unwrap_or_default();
        let mono = self.clock.mono_ns();
        store.append_run_record(
            &mut log,
            mono,
            now,
            Event::ConfirmResolved {
                gate_id: g.id.clone(),
                answer: ConfirmAnswer::Expired { why },
            },
        )?;
        store.append_run_record(
            &mut log,
            mono,
            now,
            Event::TaskFinished {
                task_id,
                status: TaskStatus::Expired,
            },
        )?;
        self.unwait(run_id);
        Ok(true)
    }

    fn unwait(&self, run_id: &str) {
        if let Some(w) = self.waiting.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            w.remove(run_id);
        }
    }

    /// Gates past [`CONFIRM_TTL_MS`] expire. Returns the task ids whose run changed.
    fn expire_due(&self, store: &Store, now: u64) -> Result<Vec<String>, CtrlError> {
        let runs: Vec<String> = {
            let mut w = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
            if w.is_none() {
                // First tick: find the runs that were waiting when the app last quit.
                let mut set = BTreeSet::new();
                for t in store.tasks() {
                    if let Some(r) = t.runs.last() {
                        if store.run(r).is_ok_and(|l| needs_confirm(&l.state())) {
                            set.insert(r.clone());
                        }
                    }
                }
                *w = Some(set);
            }
            w.as_ref()
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default()
        };
        let mut changed = Vec::new();
        for r in runs {
            let Ok(log) = store.run(&r) else {
                self.unwait(&r);
                continue;
            };
            let asked_at = log
                .records()
                .iter()
                .rev()
                .find(|x| matches!(x.event, Event::ConfirmRequested { .. }))
                .map_or(0, |x| x.t_wall_ms);
            if !needs_confirm(&log.state()) {
                self.unwait(&r);
            } else if now.saturating_sub(asked_at) >= CONFIRM_TTL_MS
                && self.expire_gate(store, &r, now, ExpiredWhy::Timeout)?
            {
                changed.extend(store.run_meta(&r).map(|m| m.0.clone()));
            }
        }
        Ok(changed)
    }

    /// (task id, run id) of the gates that started waiting since the last call (the shell raises a notification
    /// that doesn't activate qrtz; the answer happens in the ctrl view).
    pub fn take_confirm_notices(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.notices.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The run's view (`ctrl:run`): events, and the state after `at` with labels resolved from the task text.
    fn run_view(&self, store: &Store, log: &EventLog, at: Option<u64>) -> Value {
        let total = log.records().len();
        let at = at.map_or(total, |a| (a as usize).min(total));
        let task_id = store
            .run_meta(log.run_id())
            .map(|m| m.0.clone())
            .unwrap_or_default();
        let text = store.task(&task_id).ok().map(|t| t.text.clone());
        let goal = text.as_deref().map(|text| Goal {
            task_id: &task_id,
            text,
        });
        let events: Vec<Value> = log
            .records()
            .iter()
            .map(|r| {
                json!({
                    "seq": r.seq,
                    "kind": event_kind(&r.event),
                    "at": r.t_wall_ms,
                    "actionId": event_action(&r.event),
                    "node": event_node(&r.event),
                })
            })
            .collect();
        let st = fold(log.events().take(at));
        let mut state = state_json(&st, goal.as_ref());
        let labels = self.labels.lock().unwrap_or_else(|e| e.into_inner());
        state["confirm"] = confirm_json(&st, goal.as_ref(), &labels);
        state["undo"] = self.undo_json(log, &st, at, &labels);
        json!({
            "run": Self::run_json(store, log),
            "events": events,
            "at": at,
            "state": state,
        })
    }

    /// The put-back of the run's finished file batch (ADR 0117): what would go back now (read from the disk, only
    /// at the end of the log), or what went back. `null`: nothing to put back.
    fn undo_json(
        &self,
        log: &EventLog,
        st: &CtrlState,
        at: usize,
        labels: &BTreeMap<String, String>,
    ) -> Value {
        let Some((action_id, op)) = done_batch(log, at) else {
            return Value::Null;
        };
        if st.task_status != Some(TaskStatus::Done) {
            return Value::Null;
        }
        if let Some(u) = st.undo.as_ref() {
            return json!({
                "actionId": u.action_id,
                "files": u.undone as usize + u.skipped.len(),
                "ready": 0,
                "cantUndo": skipped_json(&u.skipped, labels),
                "done": {"undone": u.undone},
            });
        }
        let (files, ready, cant) = match (self.files.as_ref(), at == log.records().len()) {
            (Some(roots), true) => {
                let p = plan_undo(&op, roots);
                (p.files, p.back.len(), skipped_json(&p.skipped, labels))
            }
            // Scrubbed back, or no folder to read: the batch as logged, nothing judged.
            _ => {
                let FsOp::Batch { ops } = &op else {
                    return Value::Null;
                };
                let n = ops.iter().filter(|o| !matches!(o, FsOp::MakeDir { .. })).count();
                (n, 0, Vec::new())
            }
        };
        json!({
            "actionId": action_id,
            "files": files,
            "ready": ready,
            "cantUndo": cant,
            "done": Value::Null,
        })
    }

    /// `ctrl:undo` (ADR 0117): put a finished file batch back. Everything that can go back goes back together; if
    /// anything moved since, ctrl refuses (`undo.refused`, nothing moves) unless the user said `rest`.
    fn undo(&self, store: &Store, log: &mut EventLog, now: u64, rest: bool) -> Result<(), CtrlError> {
        let refuse = |m: &str| Err(CtrlError::new(CtrlErrorCode::Validation, m.to_string()));
        let s = log.state();
        let Some((action_id, op)) = done_batch(log, log.records().len()) else {
            return refuse("this run didn't move any files");
        };
        if s.task_status != Some(TaskStatus::Done) {
            return refuse("this run didn't finish");
        }
        if s.undo.is_some() {
            return refuse("this run was already put back");
        }
        let Some(roots) = self.files.as_ref() else {
            return refuse("ctrl can't reach your files here");
        };
        let p = plan_undo(&op, roots);
        let mono = self.clock.mono_ns();
        if p.back.is_empty() || (!p.skipped.is_empty() && !rest) {
            store.append_run_record(
                log,
                mono,
                now,
                Event::UndoRefused {
                    action_id,
                    skipped: p.skipped,
                },
            )?;
            return Ok(());
        }
        let (ok, undone, err) = match run_undo(&p) {
            Ok(n) => (true, n as u32, None),
            Err(e) => (false, 0, Some(e)),
        };
        store.append_run_record(
            log,
            mono,
            now,
            Event::UndoDone {
                action_id,
                undone,
                skipped: p.skipped,
                ok,
                err,
            },
        )?;
        Ok(())
    }

    /// One scheduler pass (ADR 0105): for every non-paused task that is due, record exactly one run (catch-up
    /// once after sleep/quit; no burst). Returns the task ids that got a run (for `ctrl:changed`).
    pub fn tick(&self) -> Result<Vec<String>, CtrlError> {
        let mut guard = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let store = guard.as_mut().map_err(|e| CtrlError::from(e.clone()))?;
        let now = self.clock.wall_ms();
        let zone = self.zone.as_ref();
        // Snapshot ids first: recording a run mutates the fold.
        let due: Vec<String> = store
            .tasks()
            .into_iter()
            .filter(|t| !t.paused)
            .filter(|t| {
                let last = Self::last_run_at(store, t);
                is_due(&t.schedule, t.created_at, last, now, zone)
            })
            .map(|t| t.id.clone())
            .collect();
        let mut fired = Vec::with_capacity(due.len());
        for id in due {
            // Re-check: a previous fire in this pass shouldn't matter (different tasks), but skip if gone/paused.
            let Some(t) = store.task(&id).ok().cloned() else {
                continue;
            };
            if t.paused {
                continue;
            }
            let last = Self::last_run_at(store, &t);
            if !is_due(&t.schedule, t.created_at, last, now, zone) {
                continue;
            }
            self.record_pending_run(store, now, &id)?;
            fired.push(id);
        }
        for id in self.expire_due(store, now)? {
            if !fired.contains(&id) {
                fired.push(id);
            }
        }
        Ok(fired)
    }

    /// One `ctrl:*` call.
    pub fn call(&self, channel: &str, p: Value) -> Result<Value, CtrlError> {
        let mut guard = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let store = guard.as_mut().map_err(|e| CtrlError::from(e.clone()))?;
        let now = self.clock.wall_ms();
        match channel {
            "ctrl:tasks" => Ok(Value::Array(
                store
                    .tasks()
                    .into_iter()
                    .map(|t| self.task_json(store, t))
                    .collect(),
            )),
            "ctrl:create-task" => {
                let schedule: Schedule = serde_json::from_value(p["schedule"].clone())
                    .map_err(|_| CtrlError::new(CtrlErrorCode::Validation, "pick when the task runs"))?;
                let t = store.create_task(now, str_field(&p, "text")?, schedule)?;
                Ok(self.task_json(store, &t))
            }
            "ctrl:set-paused" => {
                let paused = p["paused"].as_bool().unwrap_or(true);
                let t = store.set_paused(now, str_field(&p, "id")?, paused)?;
                Ok(self.task_json(store, &t))
            }
            "ctrl:delete-task" => {
                store.delete_task(now, str_field(&p, "id")?)?;
                Ok(json!({"ok": true}))
            }
            "ctrl:run-now" => {
                let id = str_field(&p, "id")?.to_string();
                let log = self.record_pending_run(store, now, &id)?;
                Ok(Self::run_json(store, &log))
            }
            "ctrl:runs" => {
                let t = store.task(str_field(&p, "taskId")?)?;
                let runs = t
                    .runs
                    .iter()
                    .rev()
                    .map(|r| store.run(r).map(|log| Self::run_json(store, &log)))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::Array(runs))
            }
            "ctrl:run" => {
                let log = store.run(str_field(&p, "runId")?)?;
                Ok(self.run_view(store, &log, p["at"].as_u64()))
            }
            // Answer the decision the run is asking about (or cancel the run), then continue the chain.
            "ctrl:answer" => {
                let mut log = store.run(str_field(&p, "runId")?)?;
                let decision_id = str_field(&p, "decisionId")?.to_string();
                let s = log.state();
                if s.ask.as_deref() != Some(decision_id.as_str()) {
                    return Err(CtrlError::new(
                        CtrlErrorCode::Validation,
                        "that question was already answered",
                    ));
                }
                let task_id = store
                    .run_meta(log.run_id())
                    .map(|m| m.0.clone())
                    .unwrap_or_default();
                let text = store.task(&task_id)?.text.clone();
                let choice = match &p["optionId"] {
                    Value::Null => None,
                    v => {
                        let id = v.as_str().unwrap_or_default();
                        let d = s.decisions.iter().find(|d| d.id == decision_id);
                        let ok = d.is_some_and(|d| ask_options(d).iter().any(|&i| d.options[i].id == id));
                        if !ok {
                            return Err(CtrlError::new(
                                CtrlErrorCode::Validation,
                                "pick one of the options",
                            ));
                        }
                        Some(id.to_string())
                    }
                };
                let mono = self.clock.mono_ns();
                let cancel = choice.is_none();
                store.append_run_record(
                    &mut log,
                    mono,
                    now,
                    Event::DecisionAnswered { decision_id, choice },
                )?;
                if cancel {
                    store.append_run_record(
                        &mut log,
                        mono,
                        now,
                        Event::TaskFinished {
                            task_id,
                            status: TaskStatus::Cancelled,
                        },
                    )?;
                } else {
                    self.continue_chain(store, &mut log, now, &task_id, &text)?;
                }
                Ok(self.run_view(store, &log, None))
            }
            // The confirm gate's answer (ADR 0111): approve acts once, remember also grants a standing approval for
            // this task, deny ends the run without acting.
            "ctrl:confirm" => {
                let mut log = store.run(str_field(&p, "runId")?)?;
                let gate_id = str_field(&p, "gateId")?.to_string();
                let s = log.state();
                let Some(g) = s
                    .confirm
                    .as_ref()
                    .filter(|g| g.id == gate_id && g.answer.is_none())
                else {
                    return Err(CtrlError::new(
                        CtrlErrorCode::Validation,
                        "that request was already answered",
                    ));
                };
                let task_id = store
                    .run_meta(log.run_id())
                    .map(|m| m.0.clone())
                    .unwrap_or_default();
                let text = store.task(&task_id)?.text.clone();
                let answer = match str_field(&p, "answer")? {
                    "approve" => ConfirmAnswer::Approve,
                    "deny" => ConfirmAnswer::Deny,
                    "remember" => {
                        let scope = g.preview.scope();
                        let approval_id = store.grant_approval(now, &task_id, scope.clone())?;
                        ConfirmAnswer::ApproveRemember { approval_id, scope }
                    }
                    _ => {
                        return Err(CtrlError::new(
                            CtrlErrorCode::Validation,
                            "answer approve, remember or deny",
                        ))
                    }
                };
                let approves = answer.approves();
                // "Close all my tabs" (ADR 0114): the other tabs are already closed; deny keeps the held ones open
                // and the run is done.
                let denied_status = if g.preview.mv == crate::exec::HELD_TABS_MOVE {
                    TaskStatus::Done
                } else {
                    TaskStatus::Cancelled
                };
                let mono = self.clock.mono_ns();
                store.append_run_record(&mut log, mono, now, Event::ConfirmResolved { gate_id, answer })?;
                self.unwait(log.run_id());
                if approves {
                    self.continue_chain(store, &mut log, now, &task_id, &text)?;
                } else {
                    store.append_run_record(
                        &mut log,
                        mono,
                        now,
                        Event::TaskFinished {
                            task_id,
                            status: denied_status,
                        },
                    )?;
                }
                Ok(self.run_view(store, &log, None))
            }
            "ctrl:undo" => {
                let mut log = store.run(str_field(&p, "runId")?)?;
                self.undo(store, &mut log, now, p["rest"].as_bool().unwrap_or(false))?;
                Ok(self.run_view(store, &log, None))
            }
            "ctrl:revoke-approval" => {
                let task_id = str_field(&p, "taskId")?.to_string();
                store.revoke_approval(now, &task_id, str_field(&p, "approvalId")?)?;
                Ok(self.task_json(store, store.task(&task_id)?))
            }
            _ => Err(CtrlError::new(
                CtrlErrorCode::Unavailable,
                format!("{channel} is not a ctrl channel"),
            )),
        }
    }
}
