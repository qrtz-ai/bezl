//! The append-only ctrl event log (decision-layer spec §13). State is `fold(apply, CtrlState::default(), events)`;
//! [`apply`] is pure (no clock, no I/O: times and ids arrive inside events). Each record carries the SHA-256 of
//! the previous one, so a rewritten or dropped record breaks [`EventLog::verify`]. Payloads hold ids, refs, enums
//! and numbers only, never paths, labels or text (ADR 0083).
//!
//! Slice 1 covers the execution events: what was decided for an action, the guard snapshots around it, the result
//! and any violation. Observation and model-decision events (`obs.*`, `decision.made`) arrive with slices 4–5.
use crate::guard::{Field, GuardOutcome, UiSnapshot};
use crate::ladder::{Decision, Route, Rung, Verdict};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA_V: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    #[serde(rename = "task.started")]
    TaskStarted { task_id: String },
    #[serde(rename = "action.requested")]
    ActionRequested { action_id: String, routes: Vec<Route> },
    #[serde(rename = "action.decided")]
    ActionDecided {
        action_id: String,
        decision: Decision,
        verdicts: Vec<Verdict>,
    },
    #[serde(rename = "guard.snapshot")]
    GuardSnapshot {
        action_id: String,
        phase: Phase,
        snapshot: UiSnapshot,
    },
    #[serde(rename = "action.executed")]
    ActionExecuted {
        action_id: String,
        ok: bool,
        err: Option<String>,
        elapsed_ms: u64,
    },
    #[serde(rename = "guard.result")]
    GuardResult {
        action_id: String,
        app: Option<String>,
        rung: Rung,
        outcome: GuardOutcome,
    },
    /// ctrl tried to put back what it changed (re-activate the previous app). Recorded, not hidden.
    #[serde(rename = "guard.restored")]
    GuardRestored {
        action_id: String,
        fields: Vec<Field>,
        ok: bool,
    },
    #[serde(rename = "task.finished")]
    TaskFinished { task_id: String, status: TaskStatus },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Done,
    Failed,
    Cancelled,
    AskedOut,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub seq: u64,
    pub run_id: String,
    /// Set on the first record of a fork: the parent run's seq it continues from.
    pub parent_seq: Option<u64>,
    pub t_mono_ns: u64,
    pub t_wall_ms: u64,
    pub schema_v: u32,
    pub event: Event,
    pub prev_hash: String,
    pub hash: String,
}

fn digest(r: &Record) -> String {
    #[derive(Serialize)]
    struct Body<'a> {
        seq: u64,
        run_id: &'a str,
        parent_seq: Option<u64>,
        t_mono_ns: u64,
        t_wall_ms: u64,
        schema_v: u32,
        event: &'a Event,
        prev_hash: &'a str,
    }
    let body = Body {
        seq: r.seq,
        run_id: &r.run_id,
        parent_seq: r.parent_seq,
        t_mono_ns: r.t_mono_ns,
        t_wall_ms: r.t_wall_ms,
        schema_v: r.schema_v,
        event: &r.event,
        prev_hash: &r.prev_hash,
    };
    let bytes = serde_json::to_vec(&body).expect("records serialize");
    hex::encode(Sha256::digest(bytes))
}

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq)]
pub enum LogError {
    BrokenChain { seq: u64 },
    BadSeq { expected: u64, got: u64 },
}

/// The run's log. Append-only: there is no API that edits or removes a record.
#[derive(Debug, Clone, Default)]
pub struct EventLog {
    run_id: String,
    records: Vec<Record>,
}

impl EventLog {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            records: Vec::new(),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn events(&self) -> impl Iterator<Item = &Event> {
        self.records.iter().map(|r| &r.event)
    }

    pub fn append(&mut self, t_mono_ns: u64, t_wall_ms: u64, event: Event) -> &Record {
        let seq = self.records.len() as u64;
        let prev_hash = self
            .records
            .last()
            .map_or_else(|| GENESIS.to_string(), |r| r.hash.clone());
        let mut r = Record {
            seq,
            run_id: self.run_id.clone(),
            parent_seq: None,
            t_mono_ns,
            t_wall_ms,
            schema_v: SCHEMA_V,
            event,
            prev_hash,
            hash: String::new(),
        };
        r.hash = digest(&r);
        self.records.push(r);
        self.records.last().expect("just pushed")
    }

    /// A new run that starts with this run's records `[0..=at]` (rehashed under the new run id; the first record
    /// carries `parent_seq = at`). Continue it live with a different option, model or threshold.
    pub fn fork(&self, at: u64, run_id: impl Into<String>) -> EventLog {
        let mut f = EventLog::new(run_id);
        for r in self.records.iter().take(at as usize + 1) {
            f.append(r.t_mono_ns, r.t_wall_ms, r.event.clone());
        }
        if let Some(first) = f.records.first_mut() {
            first.parent_seq = Some(at);
        }
        // parent_seq changed the first record's body: rechain.
        let mut prev = GENESIS.to_string();
        for r in &mut f.records {
            r.prev_hash = prev;
            r.hash = digest(r);
            prev = r.hash.clone();
        }
        f
    }

    pub fn verify(&self) -> Result<(), LogError> {
        let mut prev = GENESIS.to_string();
        for (i, r) in self.records.iter().enumerate() {
            if r.seq != i as u64 {
                return Err(LogError::BadSeq {
                    expected: i as u64,
                    got: r.seq,
                });
            }
            if r.prev_hash != prev || digest(r) != r.hash {
                return Err(LogError::BrokenChain { seq: r.seq });
            }
            prev = r.hash.clone();
        }
        Ok(())
    }

    /// JSON lines (one record per line): the on-disk form for now, and what replay tests round-trip.
    pub fn to_jsonl(&self) -> String {
        self.records
            .iter()
            .map(|r| serde_json::to_string(r).expect("serialize") + "\n")
            .collect()
    }

    pub fn from_jsonl(run_id: &str, s: &str) -> Result<EventLog, serde_json::Error> {
        let records = s
            .lines()
            .filter(|l| !l.is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        Ok(EventLog {
            run_id: run_id.to_string(),
            records,
        })
    }

    pub fn state(&self) -> CtrlState {
        fold(self.events())
    }

    /// State after the first `n` events (scrubbing). Nothing executes.
    pub fn state_at(&self, n: usize) -> CtrlState {
        fold(self.events().take(n))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Requested,
    Executing,
    Deferred,
    Asked,
    Refused,
    Done,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CtrlState {
    pub actions: BTreeMap<String, ActionStatus>,
    /// Total guard violations in this run.
    pub violations: u64,
    /// Guard results that couldn't judge every field.
    pub inconclusive: u64,
    /// (app, rung) pairs that violated the guard: the ladder refuses them for the rest of the run.
    pub quarantined: BTreeSet<(String, Rung)>,
    pub task_status: Option<TaskStatus>,
}

/// The reducer. Pure.
pub fn apply(mut s: CtrlState, e: &Event) -> CtrlState {
    match e {
        Event::TaskStarted { .. } => {}
        Event::ActionRequested { action_id, .. } => {
            s.actions.insert(action_id.clone(), ActionStatus::Requested);
        }
        Event::ActionDecided {
            action_id, decision, ..
        } => {
            let st = match decision {
                Decision::Execute { .. } => ActionStatus::Executing,
                Decision::Defer { .. } => ActionStatus::Deferred,
                Decision::Ask { .. } => ActionStatus::Asked,
                Decision::Refuse => ActionStatus::Refused,
            };
            s.actions.insert(action_id.clone(), st);
        }
        Event::GuardSnapshot { .. } | Event::GuardRestored { .. } => {}
        Event::ActionExecuted { action_id, ok, .. } => {
            s.actions.insert(
                action_id.clone(),
                if *ok {
                    ActionStatus::Done
                } else {
                    ActionStatus::Failed
                },
            );
        }
        Event::GuardResult {
            app, rung, outcome, ..
        } => {
            if !outcome.violations.is_empty() {
                s.violations += outcome.violations.len() as u64;
                if let Some(app) = app {
                    s.quarantined.insert((app.clone(), *rung));
                }
            }
            if !outcome.inconclusive.is_empty() {
                s.inconclusive += 1;
            }
        }
        Event::TaskFinished { status, .. } => s.task_status = Some(*status),
    }
    s
}

pub fn fold<'a>(events: impl IntoIterator<Item = &'a Event>) -> CtrlState {
    events.into_iter().fold(CtrlState::default(), apply)
}
