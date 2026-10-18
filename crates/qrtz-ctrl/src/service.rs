//! The `ctrl:*` channels (slice 2): what the shell's ctrl view calls, over the [`Store`]. Payloads arrive already
//! validated against the IPC contract (`packages/core/src/ipc/contract.ts`); answers are the contract's shapes
//! (camelCase). Nothing here executes a task yet: "run now" records a run that waits for an intent
//! (`intent.pending`); the scheduler (slice 3) and the decision layers (slice 4) take it from there.
use crate::log::{fold, ActionStatus, CtrlState, Event, EventLog, TaskStatus};
use crate::runner::Clock;
use crate::store::{Store, StoreError};
use crate::task::{Schedule, Task};
use serde_json::{json, Value};
use std::sync::Mutex;

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
}

/// The status a run shows: its `task.finished`, else waiting for an intent, else running.
pub fn run_status(s: &CtrlState) -> &'static str {
    match s.task_status {
        Some(TaskStatus::Done) => "done",
        Some(TaskStatus::Failed) => "failed",
        Some(TaskStatus::Cancelled) => "cancelled",
        Some(TaskStatus::AskedOut) => "asked-out",
        Some(TaskStatus::Deferred) => "deferred",
        None if s.intent_pending => "pending-intent",
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
        Event::TaskStarted { .. } | Event::IntentPending { .. } | Event::TaskFinished { .. } => None,
    }
}

fn event_kind(e: &Event) -> String {
    serde_json::to_value(e)
        .ok()
        .and_then(|v| v["kind"].as_str().map(str::to_string))
        .unwrap_or_default()
}

fn state_json(s: &CtrlState) -> Value {
    json!({
        "actions": s.actions.iter().map(|(id, st)| json!({"id": id, "status": action_status(*st)})).collect::<Vec<_>>(),
        "violations": s.violations,
        "inconclusive": s.inconclusive,
        "quarantined": s.quarantined.iter().map(|(app, rung)| json!({"app": app, "rung": rung})).collect::<Vec<_>>(),
        "status": run_status(s),
    })
}

fn str_field<'a>(p: &'a Value, k: &str) -> Result<&'a str, CtrlError> {
    p[k].as_str()
        .ok_or_else(|| CtrlError::new(CtrlErrorCode::Validation, format!("{k} is required")))
}

impl CtrlService {
    pub fn open(dir: impl Into<std::path::PathBuf>, clock: Box<dyn Clock + Send + Sync>) -> Self {
        CtrlService {
            store: Mutex::new(Store::open(dir)),
            clock,
        }
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

    fn task_json(store: &Store, t: &Task) -> Value {
        let last = t.runs.last().and_then(|r| store.run_meta(r)).map(|m| m.1);
        json!({
            "id": t.id,
            "text": t.text,
            "schedule": t.schedule,
            "paused": t.paused,
            "createdAt": t.created_at,
            "runs": t.runs.len(),
            "lastRunAt": last,
        })
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
                    .map(|t| Self::task_json(store, t))
                    .collect(),
            )),
            "ctrl:create-task" => {
                let schedule: Schedule = serde_json::from_value(p["schedule"].clone())
                    .map_err(|_| CtrlError::new(CtrlErrorCode::Validation, "pick when the task runs"))?;
                let t = store.create_task(now, str_field(&p, "text")?, schedule)?;
                Ok(Self::task_json(store, &t))
            }
            "ctrl:set-paused" => {
                let paused = p["paused"].as_bool().unwrap_or(true);
                let t = store.set_paused(now, str_field(&p, "id")?, paused)?;
                Ok(Self::task_json(store, &t))
            }
            "ctrl:delete-task" => {
                store.delete_task(now, str_field(&p, "id")?)?;
                Ok(json!({"ok": true}))
            }
            "ctrl:run-now" => {
                let id = str_field(&p, "id")?.to_string();
                let mono = self.clock.mono_ns();
                let log = store.start_run(
                    now,
                    &id,
                    &[
                        (mono, Event::TaskStarted { task_id: id.clone() }),
                        (mono, Event::IntentPending { task_id: id.clone() }),
                    ],
                )?;
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
                let total = log.records().len();
                let at = p["at"].as_u64().map_or(total, |a| (a as usize).min(total));
                let events: Vec<Value> = log
                    .records()
                    .iter()
                    .map(|r| {
                        json!({
                            "seq": r.seq,
                            "kind": event_kind(&r.event),
                            "at": r.t_wall_ms,
                            "actionId": event_action(&r.event),
                        })
                    })
                    .collect();
                Ok(json!({
                    "run": Self::run_json(store, &log),
                    "events": events,
                    "at": at,
                    "state": state_json(&fold(log.events().take(at))),
                }))
            }
            _ => Err(CtrlError::new(
                CtrlErrorCode::Unavailable,
                format!("{channel} is not a ctrl channel"),
            )),
        }
    }
}
