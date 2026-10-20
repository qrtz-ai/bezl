//! The append-only ctrl do event log (decision-layer spec §13). State is `fold(apply, CtrlState::default(), events)`;
//! [`apply`] is pure (no clock, no I/O, no goal text: times and ids arrive inside events). Each record carries the
//! SHA-256 of the previous one, so a rewritten or dropped record breaks [`EventLog::verify`].
//!
//! Payloads hold ids, refs, enums, numbers and hashed tokens only (ADR 0083, ADR 0214): never a window title, an
//! element label, a value, the goal or the typed text. Text from the goal is a pointer into it (`goal#5..12`).
//!
//! The reducer also tracks where the chain is ([`Phase`]): which question comes next, which action is pending. A
//! decision moves the phase; a backtrack or an answer re-applies the decision's move from the phase it was asked in.
use crate::act::Action;
use crate::ax::{AxDiff, ScopeStep, ShortcutObs, TreeObs};
use crate::chain::AppsObs;
use crate::decide::{BacktrackReason, ModelId, Node, Opt, Outcome, Thresholds};
use crate::gate::{Binding, ConfirmAnswer, GateState};
use crate::guard::StopReason;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Schema 2: ctrl do (ADR 0214). Schema-1 runs (the chores) are not read.
pub const SCHEMA_V: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardPhase {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    #[serde(rename = "do.started")]
    DoStarted { do_id: String },
    /// The app catalog the chain chose from: bundle ids and hashed name tokens.
    #[serde(rename = "obs.apps")]
    ObsApps { apps: AppsObs },
    /// The pruned tree of an app's front window (`scope: None`) or one element's children; `None` + `err`: the
    /// helper couldn't read it.
    #[serde(rename = "obs.tree")]
    ObsTree {
        app: String,
        scope: Option<String>,
        tree: Option<TreeObs>,
        err: Option<String>,
    },
    #[serde(rename = "obs.shortcuts")]
    ObsShortcuts {
        app: String,
        items: Vec<ShortcutObs>,
        err: Option<String>,
    },
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
        alternatives: Vec<String>,
        model: ModelId,
        input_hash: String,
        latency_us: u64,
    },
    /// Jared's answer to an asked decision; `None` = cancel the run.
    #[serde(rename = "decision.answered_by_user")]
    DecisionAnswered {
        decision_id: String,
        choice: Option<String>,
    },
    #[serde(rename = "backtrack")]
    Backtrack {
        decision_id: String,
        to: String,
        reason: BacktrackReason,
    },
    #[serde(rename = "confirm.requested")]
    ConfirmRequested {
        gate_id: String,
        binding: Binding,
        rules: Vec<String>,
    },
    #[serde(rename = "confirm.resolved")]
    ConfirmResolved { gate_id: String, answer: ConfirmAnswer },
    /// Right before acting, the tree was read again: the same (`ok`), or it moved and the gate asks again.
    #[serde(rename = "confirm.reverified")]
    ConfirmReverified {
        gate_id: String,
        ok: bool,
        /// Review M1: the approved element's handle in the fresh tree (ids change on every full read).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handle: Option<String>,
    },
    #[serde(rename = "action.requested")]
    ActionRequested { action_id: String, action: Action },
    /// The host's policy refused the action before sending it (stale handle, combo not allowed, …).
    #[serde(rename = "action.refused")]
    ActionRefused { action_id: String, code: String },
    #[serde(rename = "action.executed")]
    ActionExecuted {
        action_id: String,
        ok: bool,
        err: Option<String>,
        elapsed_ms: u64,
    },
    /// The stop-key watcher's flags around an action (`stop: None`: clear).
    #[serde(rename = "guard.result")]
    GuardResult {
        action_id: String,
        phase: GuardPhase,
        stop: Option<StopReason>,
        frontmost: Option<String>,
    },
    /// What the action changed in the window's tree.
    #[serde(rename = "obs.diff")]
    ObsDiff { action_id: String, diff: AxDiff },
    #[serde(rename = "task.finished")]
    TaskFinished {
        status: TaskStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop: Option<StopReason>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Done,
    Failed,
    /// Jared cancelled an ask or denied a confirm.
    Cancelled,
    /// Out of alternatives: ctrl couldn't do it and says so.
    AskedOut,
    /// The stop key, Jared's input, or the stop button.
    Stopped,
    /// The step budget ran out.
    OutOfSteps,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub seq: u64,
    pub run_id: String,
    pub parent_seq: Option<u64>,
    pub t_mono_ns: u64,
    pub t_wall_ms: u64,
    pub schema_v: u32,
    pub event: Event,
    pub prev_hash: String,
    pub hash: String,
}

#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "serde_json on a plain derived type with string keys cannot fail"
)]
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
    hex::encode(Sha256::digest(
        serde_json::to_vec(&body).expect("records serialize"),
    ))
}

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq)]
pub enum LogError {
    BrokenChain { seq: u64 },
    BadSeq { expected: u64, got: u64 },
}

/// A run's log. Append-only: no API edits or removes a record.
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
        let at = self.records.len();
        self.records.push(r);
        &self.records[at]
    }

    /// A new run that starts with this run's records `[0..=at]` (rehashed under the new run id; the first record
    /// carries `parent_seq = at`).
    pub fn fork(&self, at: u64, run_id: impl Into<String>) -> EventLog {
        let mut f = EventLog::new(run_id);
        for r in self.records.iter().take(at as usize + 1) {
            f.append(r.t_mono_ns, r.t_wall_ms, r.event.clone());
        }
        if let Some(first) = f.records.first_mut() {
            first.parent_seq = Some(at);
        }
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

    #[allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "serde_json on a plain derived type with string keys cannot fail"
    )]
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

/// Where the chain is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// Waiting for the app catalog.
    #[default]
    Catalog,
    ChooseApp,
    /// A decision chose an action; it runs next (through the gate when a rule matches).
    Act {
        action: Action,
    },
    /// The action ran; the window's tree is read next.
    AfterAction,
    /// After that read: verify (or choose the text when a text field just got focus).
    Verify,
    ChooseMove,
    Descend {
        path: Vec<ScopeStep>,
    },
    /// Verify said done.
    Done,
    /// Verify said it didn't work, or the action failed or was refused, or the path is gone: backtrack or give up.
    Failed {
        reason: BacktrackReason,
    },
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Requested,
    Refused,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionState {
    pub id: String,
    pub action: Action,
    pub status: ActionStatus,
    pub err: Option<String>,
    pub diff: Option<AxDiff>,
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
    /// The phase it was asked in (a backtrack or an answer re-applies the move from here).
    pub phase_before: Phase,
    /// Actions run (done or failed) when it was made. ADR 0214 review (b): once an action ran after a decision,
    /// that decision is never backtracked: the run ends and hands back.
    #[serde(default)]
    pub actions_before: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CtrlState {
    pub started: bool,
    pub apps: Option<AppsObs>,
    /// The app ctrl opened (the one the chain works in).
    pub app: Option<String>,
    pub tree: Option<TreeObs>,
    /// The tree hash right before the last action (for verify).
    pub tree_before: Option<String>,
    pub shortcuts: BTreeMap<String, Vec<ShortcutObs>>,
    pub phase: Phase,
    pub decisions: Vec<DecisionState>,
    /// Every `decision.made` ever applied (ids stay unique across backtracks).
    pub decisions_made: u64,
    pub ask: Option<String>,
    pub backtracks: u32,
    pub actions: Vec<ActionState>,
    /// Text fields ctrl already typed into in this run.
    pub typed: BTreeSet<String>,
    /// The hashed label tokens of the element (or menu item) the latest action acted on.
    pub acted_tokens: Vec<String>,
    pub acted_role: Option<String>,
    pub confirm: Option<GateState>,
    pub gates: u64,
    pub status: Option<TaskStatus>,
    pub stop: Option<StopReason>,
}

impl CtrlState {
    pub fn last_action(&self) -> Option<&ActionState> {
        self.actions.last()
    }
    pub fn executed(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| a.status == ActionStatus::Done || a.status == ActionStatus::Failed)
            .count()
    }
}

/// The move a chosen option makes from `phase` (pure). `None`: the option doesn't lead anywhere from here.
pub fn advance(phase: &Phase, s: &CtrlState, node: Node, opt: &Opt) -> Phase {
    let app = s.app.clone().unwrap_or_default();
    let id = opt.id.as_str();
    let fail = Phase::Failed {
        reason: BacktrackReason::NoneOfThese,
    };
    let ax_action = |rest: &str| -> Option<Action> {
        let (h, a) = rest.rsplit_once('.')?;
        let handle = h.to_string();
        Some(match a {
            "press" => Action::Press { handle },
            "increment" => Action::Increment { handle },
            "decrement" => Action::Decrement { handle },
            "show_menu" => Action::ShowMenu { handle },
            "focus" => Action::Focus { handle },
            "scroll_down" => Action::Scroll {
                handle,
                dir: crate::act::ScrollDir::Down,
            },
            "scroll_up" => Action::Scroll {
                handle,
                dir: crate::act::ScrollDir::Up,
            },
            _ => return None,
        })
    };
    let path = match phase {
        Phase::Descend { path } => path.clone(),
        _ => Vec::new(),
    };
    let descend = |step: ScopeStep| {
        let mut p = path.clone();
        p.push(step);
        Phase::Descend { path: p }
    };
    match node {
        Node::ChooseApp => match id.strip_prefix("app.") {
            Some(a) => Phase::Act {
                action: Action::OpenApp { app: a.into() },
            },
            None => fail,
        },
        Node::ChooseMove | Node::ChooseElement => {
            if let Some(h) = id.strip_prefix("menu.") {
                Phase::Act {
                    action: Action::Press { handle: h.into() },
                }
            } else if let Some(c) = id.strip_prefix("key.") {
                Phase::Act {
                    action: Action::KeyCombo { app, combo: c.into() },
                }
            } else if id.starts_with("url.") {
                match &opt.label_ref {
                    Some(r) => Phase::Act {
                        action: Action::OpenUrl {
                            app,
                            url_ref: r.clone(),
                        },
                    },
                    None => fail,
                }
            } else if let Some(h) = id.strip_prefix("branch.") {
                descend(ScopeStep::Node { id: h.into() })
            } else if let Some(g) = id.strip_prefix("group.") {
                match g.rsplit_once('~').and_then(|(r, p)| Some((r, p.parse().ok()?))) {
                    Some((role, page)) => descend(ScopeStep::Group {
                        role: role.into(),
                        page,
                    }),
                    None => fail,
                }
            } else if let Some(rest) = id.strip_prefix("ax.") {
                match ax_action(rest) {
                    Some(action) => Phase::Act { action },
                    None => fail,
                }
            } else {
                fail
            }
        }
        Node::ChooseText => {
            let focused = s.tree.as_ref().and_then(|t| t.focused.clone());
            match (focused, &opt.label_ref) {
                (Some(handle), Some(r)) => Phase::Act {
                    action: Action::SetValue {
                        handle,
                        text_ref: r.clone(),
                        submit: id.ends_with(".go"),
                    },
                },
                _ => fail,
            }
        }
        Node::Verify => match id {
            "verify.done" => Phase::Done,
            "verify.continue" => Phase::ChooseMove,
            _ => Phase::Failed {
                reason: BacktrackReason::VerifyFailed,
            },
        },
    }
}

fn reapply(s: &mut CtrlState, idx: usize) {
    let d = &s.decisions[idx];
    let Some(opt) = d.options.iter().find(|o| o.id == d.chosen).cloned() else {
        return;
    };
    let (phase, node) = (d.phase_before.clone(), d.node);
    s.phase = advance(&phase, s, node, &opt);
}

/// The reducer. Pure.
pub fn apply(mut s: CtrlState, e: &Event) -> CtrlState {
    match e {
        Event::DoStarted { .. } => s.started = true,
        Event::ObsApps { apps } => {
            s.apps = Some(apps.clone());
            if s.phase == Phase::Catalog {
                s.phase = Phase::ChooseApp;
            }
        }
        Event::ObsTree { scope, tree, app, .. } => match (scope, tree) {
            (None, Some(t)) => {
                s.tree = Some(t.clone());
                if s.phase == Phase::AfterAction {
                    s.phase = Phase::Verify;
                }
            }
            (None, None) => {
                // Unreadable window: an empty tree, so the chain carries on with shortcuts only.
                s.tree = Some(crate::ax::prune(
                    app,
                    None,
                    &crate::ax::AxNodeWire {
                        id: "root".into(),
                        role: "AXWindow".into(),
                        enabled: true,
                        ..Default::default()
                    },
                    None,
                ));
                if s.phase == Phase::AfterAction {
                    s.phase = Phase::Verify;
                }
            }
            (Some(sc), Some(sub)) => {
                if let Some(t) = s.tree.as_mut() {
                    if crate::ax::merge(&mut t.root, sc, &sub.root) {
                        t.hash = crate::ax::tree_hash(&t.root);
                    }
                }
            }
            (Some(sc), None) => {
                // The element's children can't be read: it has none, as far as the chain is concerned.
                if let Some(t) = s.tree.as_mut() {
                    let empty = crate::ax::AxNode {
                        id: sc.clone(),
                        role: String::new(),
                        subrole: None,
                        tokens: vec![],
                        value_tokens: vec![],
                        actions: vec![],
                        text_input: false,
                        focused: false,
                        risk: vec![],
                        children: vec![],
                        more: 0,
                        sig: String::new(),
                    };
                    crate::ax::merge(&mut t.root, sc, &empty);
                }
            }
        },
        Event::ObsShortcuts { app, items, .. } => {
            s.shortcuts.insert(app.clone(), items.clone());
        }
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
                phase_before: s.phase.clone(),
                actions_before: s.executed(),
            });
            if *outcome == Outcome::Ask {
                s.ask = Some(decision_id.clone());
            } else {
                let i = s.decisions.len() - 1;
                reapply(&mut s, i);
            }
        }
        Event::DecisionAnswered { decision_id, choice } => {
            if s.ask.as_deref() == Some(decision_id.as_str()) {
                s.ask = None;
                if let (Some(c), Some(i)) = (choice, s.decisions.iter().position(|d| &d.id == decision_id)) {
                    s.decisions[i].chosen = c.clone();
                    s.decisions[i].answered = true;
                    reapply(&mut s, i);
                }
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
                s.ask = None;
                s.confirm = None;
                reapply(&mut s, i);
            }
        }
        Event::ConfirmRequested {
            gate_id,
            binding,
            rules,
        } => {
            s.gates += 1;
            s.confirm = Some(GateState {
                id: gate_id.clone(),
                binding: binding.clone(),
                rules: rules.clone(),
                answer: None,
                reverified: None,
                used: false,
            });
        }
        Event::ConfirmResolved { gate_id, answer } => {
            if let Some(g) = s
                .confirm
                .as_mut()
                .filter(|g| &g.id == gate_id && g.answer.is_none())
            {
                g.answer = Some(*answer);
            }
        }
        Event::ConfirmReverified { gate_id, ok, handle } => {
            if s.confirm.as_ref().is_some_and(|g| &g.id == gate_id) {
                if *ok {
                    if let Some(g) = s.confirm.as_mut() {
                        g.reverified = Some(true);
                        if let Some(h) = handle {
                            g.binding.handle = Some(h.clone());
                        }
                    }
                    if let (Some(h), Phase::Act { action }) = (handle, &mut s.phase) {
                        *action = action.with_handle(h);
                    }
                } else {
                    s.confirm = None;
                }
            }
        }
        Event::ActionRequested { action_id, action } => {
            if let Some(g) = s
                .confirm
                .as_mut()
                .filter(|g| g.answer == Some(ConfirmAnswer::Approve))
            {
                g.used = true;
            }
            s.tree_before = s.tree.as_ref().map(|t| t.hash.clone());
            s.acted_role = action
                .handle()
                .and_then(|h| s.tree.as_ref().and_then(|t| crate::ax::find(&t.root, h)))
                .map(|n| n.role.clone());
            s.acted_tokens = action
                .handle()
                .and_then(|h| {
                    let in_tree = s
                        .tree
                        .as_ref()
                        .and_then(|t| crate::ax::find(&t.root, h))
                        .map(|n| n.tokens.clone());
                    in_tree.or_else(|| {
                        s.shortcuts
                            .values()
                            .flatten()
                            .find(|m| m.id == h)
                            .map(|m| m.tokens.clone())
                    })
                })
                .unwrap_or_default();
            s.actions.push(ActionState {
                id: action_id.clone(),
                action: action.clone(),
                status: ActionStatus::Requested,
                err: None,
                diff: None,
            });
        }
        Event::ActionRefused { action_id, code } => {
            if let Some(a) = s.actions.iter_mut().find(|a| &a.id == action_id) {
                a.status = ActionStatus::Refused;
                a.err = Some(code.clone());
            }
            s.phase = Phase::Failed {
                reason: BacktrackReason::ActionFailed,
            };
        }
        Event::ActionExecuted {
            action_id, ok, err, ..
        } => {
            if let Some(a) = s.actions.iter_mut().find(|a| &a.id == action_id) {
                a.status = if *ok {
                    ActionStatus::Done
                } else {
                    ActionStatus::Failed
                };
                a.err = err.clone();
                if *ok {
                    match &a.action {
                        Action::OpenApp { app } => s.app = Some(app.clone()),
                        Action::SetValue { handle, .. } => {
                            s.typed.insert(handle.clone());
                        }
                        _ => {}
                    }
                }
            }
            s.confirm = None;
            s.phase = if *ok {
                Phase::AfterAction
            } else {
                Phase::Failed {
                    reason: BacktrackReason::ActionFailed,
                }
            };
        }
        Event::GuardResult { stop, .. } => {
            if stop.is_some() {
                s.stop = *stop;
            }
        }
        Event::ObsDiff { action_id, diff } => {
            if let Some(a) = s.actions.iter_mut().find(|a| &a.id == action_id) {
                a.diff = Some(diff.clone());
            }
        }
        Event::TaskFinished { status, stop } => {
            s.status = Some(*status);
            if stop.is_some() {
                s.stop = *stop;
            }
            s.phase = Phase::Finished;
            s.ask = None;
        }
    }
    s
}

pub fn fold<'a>(events: impl IntoIterator<Item = &'a Event>) -> CtrlState {
    events.into_iter().fold(CtrlState::default(), apply)
}
