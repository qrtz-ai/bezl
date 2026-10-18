//! The append-only ctrl event log (decision-layer spec §13). State is `fold(apply, CtrlState::default(), events)`;
//! [`apply`] is pure (no clock, no I/O: times and ids arrive inside events). Each record carries the SHA-256 of
//! the previous one, so a rewritten or dropped record breaks [`EventLog::verify`]. Payloads hold ids, refs, enums
//! and numbers only, never paths, labels or text (ADR 0083).
//!
//! Slice 2 persists each run as its own file (`store.rs`, ADR 0103). Slice 1 covers the execution events: what was decided for an action, the guard snapshots around it, the result
//! and any violation. Slice 4 (ADR 0106) adds the decision layers' events: `obs.apps` (the app catalog the chain
//! chose from), `decision.made` (options, probabilities, chosen option, outcome), `decision.answered_by_user`,
//! `backtrack` and `plan.produced`. Slice 5 (ADR 0108) adds the executor's: `obs.snapshot` (an AX scope of a
//! background app, hashed), `obs.diff` (what an action changed in it), `verify.result` and `exec.held` (the plan
//! stops before acting: irreversible, not yet supported, or no background route).
use crate::ax::{AxDiff, AxSnapshot};
use crate::decide::{BacktrackReason, ModelId, Node, Opt, Outcome, Thresholds};
use crate::exec::{HoldReason, VerifyOutcome};
use crate::guard::{Field, GuardOutcome, UiSnapshot};
use crate::helper::{Check, Expect};
use crate::ladder::{Decision, Route, Rung, Verdict};
use crate::layers::AppsObs;
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
    /// The run is waiting for an intent: the decision layers (slice 4) haven't produced a plan yet. Slice 2
    /// records runs this far ("run now" in the ctrl view) so history, scrub and replay work end to end.
    #[serde(rename = "intent.pending")]
    IntentPending { task_id: String },
    /// The app catalog the decision layers chose from: bundle ids and hashed name tokens, never names.
    #[serde(rename = "obs.apps")]
    ObsApps { apps: AppsObs },
    /// One small decision (spec §13.2): the options, their logits and calibrated probabilities (basis points),
    /// the chosen option and what the chain does with it.
    #[serde(rename = "decision.made")]
    DecisionMade {
        decision_id: String,
        node: Node,
        options: Vec<Opt>,
        logits_milli: Vec<i64>,
        p_bp: Vec<u32>,
        temperature_milli: u32,
        chosen: String,
        p_chosen_bp: u32,
        thresholds: Thresholds,
        outcome: Outcome,
        /// Tentative only: the next-best options, the backtrack stack.
        alternatives: Vec<String>,
        model: ModelId,
        seed: u64,
        input_hash: String,
        latency_us: u64,
    },
    /// The user's answer to an asked decision; `None` = cancel.
    #[serde(rename = "decision.answered_by_user")]
    DecisionAnswered {
        decision_id: String,
        choice: Option<String>,
    },
    /// A tentative choice didn't verify: take the next alternative; later decisions are dropped.
    #[serde(rename = "backtrack")]
    Backtrack {
        decision_id: String,
        to: String,
        reason: BacktrackReason,
    },
    /// Every node has a choice: the plan the executor (slice 5) runs.
    #[serde(rename = "plan.produced")]
    PlanProduced {
        steps: Vec<PlanStep>,
        irreversible: bool,
        model: ModelId,
    },
    /// An AX scope of a background app as the helper saw it (`None` + `err`: it couldn't, e.g. not running).
    #[serde(rename = "obs.snapshot")]
    ObsSnapshot {
        app: String,
        scope: Option<String>,
        snapshot: Option<AxSnapshot>,
        err: Option<String>,
    },
    /// What an action changed in an observed scope (the new snapshot follows as `obs.snapshot`).
    #[serde(rename = "obs.diff")]
    ObsDiff {
        action_id: String,
        app: String,
        scope: Option<String>,
        diff: AxDiff,
    },
    /// Did the action do what the move meant? A failure drives `backtrack` (or asks when out of alternatives).
    #[serde(rename = "verify.result")]
    VerifyResult {
        action_id: String,
        check: Check,
        expect: Expect,
        before: Option<i64>,
        after: Option<i64>,
        outcome: VerifyOutcome,
    },
    /// The executor stopped before acting (slice 6's confirm gate resumes an irreversible plan).
    #[serde(rename = "exec.held")]
    ExecHeld { reason: HoldReason },
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub node: Node,
    pub option: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanState {
    pub steps: Vec<PlanStep>,
    pub irreversible: bool,
}

/// A decision as the state holds it (from `decision.made`, then answers and backtracks).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionState {
    pub id: String,
    pub node: Node,
    pub options: Vec<Opt>,
    pub p_bp: Vec<u32>,
    pub chosen: String,
    pub p_chosen_bp: u32,
    pub outcome: Outcome,
    pub alternatives: Vec<String>,
    pub answered: bool,
    pub backtracks: u32,
    pub model: ModelId,
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
    /// `task.started` was seen.
    pub started: bool,
    /// Waiting for the decision layers; cleared by `plan.produced`, the first `action.requested` or
    /// `task.finished`.
    pub intent_pending: bool,
    /// The app catalog of `obs.apps`.
    pub apps: Option<AppsObs>,
    /// The chain so far, in order (a backtrack drops the decisions after the one it changes).
    pub decisions: Vec<DecisionState>,
    /// Every `decision.made` ever applied (decision ids stay unique across backtracks).
    pub decisions_made: u64,
    pub plan: Option<PlanState>,
    /// The decision waiting for the user's answer.
    pub ask: Option<String>,
    pub backtracks: u32,
    /// AX observations (`obs.snapshot`), in order; questions read the latest per (app, scope).
    pub snapshots: Vec<AxSnapshot>,
    /// Verify outcomes per action.
    pub verified: BTreeMap<String, VerifyOutcome>,
    /// Why the executor stopped before acting, until a new plan (or the confirm gate) moves on.
    pub held: Option<HoldReason>,
}

impl CtrlState {
    /// The option chosen at each node: acted, tentative, or answered by the user (an unanswered ask isn't).
    pub fn chosen(&self) -> BTreeMap<Node, String> {
        self.decisions
            .iter()
            .filter(|d| d.outcome != Outcome::Ask || d.answered)
            .map(|d| (d.node, d.chosen.clone()))
            .collect()
    }
}

/// The reducer. Pure.
pub fn apply(mut s: CtrlState, e: &Event) -> CtrlState {
    match e {
        Event::TaskStarted { .. } => s.started = true,
        Event::IntentPending { .. } => s.intent_pending = true,
        Event::ObsApps { apps } => s.apps = Some(apps.clone()),
        Event::DecisionMade {
            decision_id,
            node,
            options,
            p_bp,
            chosen,
            p_chosen_bp,
            outcome,
            alternatives,
            model,
            ..
        } => {
            s.decisions_made += 1;
            s.plan = None;
            s.held = None;
            if *outcome == Outcome::Ask {
                s.ask = Some(decision_id.clone());
            }
            s.decisions.push(DecisionState {
                id: decision_id.clone(),
                node: *node,
                options: options.clone(),
                p_bp: p_bp.clone(),
                chosen: chosen.clone(),
                p_chosen_bp: *p_chosen_bp,
                outcome: *outcome,
                alternatives: alternatives.clone(),
                answered: false,
                backtracks: 0,
                model: model.clone(),
            });
        }
        Event::DecisionAnswered { decision_id, choice } => {
            if s.ask.as_deref() == Some(decision_id.as_str()) {
                s.ask = None;
            }
            if let (Some(c), Some(d)) = (choice, s.decisions.iter_mut().find(|d| &d.id == decision_id)) {
                d.chosen = c.clone();
                d.answered = true;
            }
        }
        Event::Backtrack { decision_id, to, .. } => {
            if let Some(i) = s.decisions.iter().position(|d| &d.id == decision_id) {
                s.decisions.truncate(i + 1);
                let d = &mut s.decisions[i];
                d.chosen = to.clone();
                d.alternatives.retain(|a| a != to);
                d.backtracks += 1;
                s.backtracks += 1;
                s.plan = None;
                s.held = None;
                s.ask = None;
            }
        }
        Event::PlanProduced {
            steps, irreversible, ..
        } => {
            s.intent_pending = false;
            s.plan = Some(PlanState {
                steps: steps.clone(),
                irreversible: *irreversible,
            });
        }
        Event::ActionRequested { action_id, .. } => {
            s.intent_pending = false;
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
        Event::GuardSnapshot { .. } | Event::GuardRestored { .. } | Event::ObsDiff { .. } => {}
        Event::ObsSnapshot {
            app, scope, snapshot, ..
        } => s.snapshots.push(snapshot.clone().unwrap_or(AxSnapshot {
            app: app.clone(),
            scope: scope.clone(),
            nodes: Vec::new(),
            truncated: 0,
        })),
        Event::VerifyResult {
            action_id, outcome, ..
        } => {
            s.verified.insert(action_id.clone(), *outcome);
        }
        Event::ExecHeld { reason } => s.held = Some(*reason),
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
        Event::TaskFinished { status, .. } => {
            s.intent_pending = false;
            s.task_status = Some(*status)
        }
    }
    s
}

pub fn fold<'a>(events: impl IntoIterator<Item = &'a Event>) -> CtrlState {
    events.into_iter().fold(CtrlState::default(), apply)
}
