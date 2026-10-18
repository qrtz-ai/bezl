//! The decision layers' chain (slice 4, ADR 0106; decision-layer spec §1–§5, §13). A run's intent is decided by a
//! chain of small questions, each with at most [`MAX_OPTIONS`] enumerated options, narrowing the way a person does:
//! what kind of task → which app → which target (folder, tabs, text) → which move (a shortcut, a script verb, a
//! workspace or file route, later an AX branch). Every question is answered by a [`Scorer`] (the deterministic
//! heuristic layer today, a consented Laya model later) with a calibrated probability per option, and the outcome
//! is one of: act (p ≥ τ_act), tentative (τ_back ≤ p < τ_act, alternatives kept for backtracking) or ask the user
//! (p < τ_back, or "none of these" won).
//!
//! The chain is an effect over the log: it reads the folded state, asks the next question, and appends one
//! `decision.made` per question, then `plan.produced`. Replaying the log re-derives the same questions
//! ([`replay_check`]); scrubbing never calls a scorer. Events hold ids, numbers and pointers only (ADR 0083):
//! probabilities are basis points (integers), so a record hashes the same after a round trip through JSON.
use crate::layers::{self, Next, Question};
use crate::log::{apply, CtrlState, Event, EventLog};
use crate::runner::Clock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Target option count per question (spec §1), "none of these" included.
pub const MAX_OPTIONS: usize = 20;
/// Hard cap (spec §1).
pub const HARD_CAP: usize = 64;
/// At most this many backtracks per decision and per task (spec §5).
pub const BACKTRACKS_PER_DECISION: u32 = 2;
pub const BACKTRACKS_PER_TASK: u32 = 6;
/// The option every question ends with; choosing it asks the user.
pub const NONE: &str = "none";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Node {
    ChooseIntent,
    ChooseApp,
    ChooseTarget,
    ChooseText,
    ChooseMove,
}

impl Node {
    pub fn as_str(self) -> &'static str {
        match self {
            Node::ChooseIntent => "choose_intent",
            Node::ChooseApp => "choose_app",
            Node::ChooseTarget => "choose_target",
            Node::ChooseText => "choose_text",
            Node::ChooseMove => "choose_move",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    Intent,
    App,
    Folder,
    Scope,
    TextCandidate,
    Shortcut,
    /// A route the ladder knows (files, workspace, scripting verb).
    Route,
    /// A top-level AX branch of the front window (slice 5 observes them).
    Branch,
    None,
}

/// What the ladder (ADR 0100) will say about an option when it runs in the background. The layer still offers
/// the option (Jared wants shortcuts like cmd-l next to the tree) but scores it down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LadderHint {
    /// A focus-intent shortcut (cmd-l, cmd-tab…): refused in the background.
    FocusIntent,
}

/// One enumerated option. `id` is stable and text-free (`intent.close_tabs`, `app.com.apple.Safari`,
/// `shortcut.cmd+w`); text derived from the task is a pointer into it (`label_ref`), never inline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opt {
    pub id: String,
    pub kind: OptionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_ref: Option<String>,
    /// Can't be undone (send, empty trash, quit with unsaved work, a system setting): slice 6's confirm gate asks.
    pub irreversible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ladder: Option<LadderHint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Act,
    Tentative,
    Ask,
}

/// Basis points (0…10000), per node type (spec §5; to be fitted on the replay set from the Mac benchmark).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thresholds {
    pub act_bp: u32,
    pub back_bp: u32,
}

pub fn thresholds(_node: Node) -> Thresholds {
    Thresholds {
        act_bp: 8000,
        back_bp: 5000,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModelId {
    pub id: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
}

/// The goal as the layers read it: the task text (from `tasks.jsonl`, in memory only) and its id.
#[derive(Debug, Clone, Copy)]
pub struct Goal<'a> {
    pub task_id: &'a str,
    pub text: &'a str,
}

/// Scores one question: a logit (milli-units) per option, same order. Implementations: [`Heuristic`] (no model,
/// always available) and [`crate::laya::LayaScorer`] (a consented on-device model).
pub trait Scorer: Send + Sync {
    fn model(&self) -> ModelId;
    fn logits(&self, q: &Question, goal: &Goal) -> Result<Vec<i64>, String>;
    /// Temperature (milli) per node type; 1000 until fitted.
    fn temperature_milli(&self, _node: Node) -> u32 {
        1000
    }
}

pub const HEURISTIC_ID: &str = "qrtz-heuristic";
pub const HEURISTIC_VERSION: &str = "1";

/// The deterministic layer: the option sources' priors (keyword overlap with the goal, default-handler and
/// running-app priors, ladder hints) are the logits. Runs with no model and no download.
pub struct Heuristic;

impl Scorer for Heuristic {
    fn model(&self) -> ModelId {
        ModelId {
            id: HEURISTIC_ID.into(),
            version: HEURISTIC_VERSION.into(),
            quant: None,
        }
    }
    fn logits(&self, q: &Question, _goal: &Goal) -> Result<Vec<i64>, String> {
        Ok(q.choices.iter().map(|c| c.prior).collect())
    }
}

/// Softmax over milli-logits at temperature `t_milli`, in basis points.
pub fn softmax_bp(logits: &[i64], t_milli: u32) -> Vec<u32> {
    if logits.is_empty() {
        return Vec::new();
    }
    let t = f64::from(t_milli.max(1)) / 1000.0;
    let xs: Vec<f64> = logits.iter().map(|l| *l as f64 / 1000.0 / t).collect();
    let max = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let es: Vec<f64> = xs.iter().map(|x| (x - max).exp()).collect();
    let sum: f64 = es.iter().sum();
    es.iter().map(|e| (e / sum * 10000.0).round() as u32).collect()
}

/// Index of the highest probability (the first one on ties, so option order breaks ties deterministically).
fn argmax(p: &[u32]) -> usize {
    let mut best = 0;
    for (i, v) in p.iter().enumerate() {
        if *v > p[best] {
            best = i;
        }
    }
    best
}

/// Option indexes by probability, highest first (stable).
fn ranked(p: &[u32]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..p.len()).collect();
    idx.sort_by(|a, b| p[*b].cmp(&p[*a]));
    idx
}

/// Hash of what a question was asked over: node, option ids, the choices so far and the goal's hash. Replay
/// recomputes it; a difference is a divergence.
pub fn input_hash(goal: &Goal, node: Node, options: &[Opt], chosen: &BTreeMap<Node, String>) -> String {
    #[derive(Serialize)]
    struct In<'a> {
        node: Node,
        options: Vec<&'a str>,
        refs: Vec<Option<&'a str>>,
        chosen: &'a BTreeMap<Node, String>,
        goal: String,
    }
    let body = In {
        node,
        options: options.iter().map(|o| o.id.as_str()).collect(),
        refs: options.iter().map(|o| o.label_ref.as_deref()).collect(),
        chosen,
        goal: hex::encode(Sha256::digest(goal.text.as_bytes())),
    };
    let h = Sha256::digest(serde_json::to_vec(&body).expect("serializes"));
    hex::encode(&h[..8])
}

/// Answer one question: score, calibrate, pick, and classify the outcome. Pure except for the scorer and clock.
pub fn decide(
    q: &Question,
    goal: &Goal,
    chosen: &BTreeMap<Node, String>,
    scorer: &dyn Scorer,
    clock: &dyn Clock,
    decision_id: String,
) -> Event {
    let options: Vec<Opt> = q.choices.iter().map(|c| c.opt.clone()).collect();
    assert!(
        options.len() <= HARD_CAP,
        "a question never has more than {HARD_CAP} options"
    );
    let t0 = clock.mono_ns();
    let (logits, model, t_milli) = match scorer.logits(q, goal) {
        Ok(l) if l.len() == options.len() => (l, scorer.model(), scorer.temperature_milli(q.node)),
        // A failing or malformed model never blocks a run: the heuristic answers and the log says so.
        _ => (
            Heuristic.logits(q, goal).expect("heuristic"),
            Heuristic.model(),
            1000,
        ),
    };
    let p_bp = softmax_bp(&logits, t_milli);
    let best = argmax(&p_bp);
    let th = thresholds(q.node);
    let p_chosen = p_bp[best];
    let outcome = if options[best].kind == OptionKind::None || p_chosen < th.back_bp {
        Outcome::Ask
    } else if p_chosen >= th.act_bp {
        Outcome::Act
    } else {
        Outcome::Tentative
    };
    let alternatives = if outcome == Outcome::Tentative {
        ranked(&p_bp)
            .into_iter()
            .filter(|&i| i != best && options[i].kind != OptionKind::None && p_bp[i] > 0)
            .take(BACKTRACKS_PER_DECISION as usize)
            .map(|i| options[i].id.clone())
            .collect()
    } else {
        Vec::new()
    };
    let latency_us = clock.mono_ns().saturating_sub(t0) / 1000;
    Event::DecisionMade {
        decision_id,
        node: q.node,
        input_hash: input_hash(goal, q.node, &options, chosen),
        chosen: options[best].id.clone(),
        options,
        logits_milli: logits,
        p_bp,
        temperature_milli: t_milli,
        p_chosen_bp: p_chosen,
        thresholds: th,
        outcome,
        alternatives,
        model,
        seed: 0,
        latency_us,
    }
}

/// The top three real options of an asked decision (what the user picks from).
pub fn ask_options(d: &crate::log::DecisionState) -> Vec<usize> {
    ranked(&d.p_bp)
        .into_iter()
        .filter(|&i| d.options[i].kind != OptionKind::None)
        .take(3)
        .collect()
}

/// Run the chain from the current state until it asks or produces a plan. Needs `obs.apps` first (the caller
/// observes); does nothing on a run that is asking, planned or finished.
pub fn run_chain(goal: &Goal, state: &CtrlState, scorer: &dyn Scorer, clock: &dyn Clock) -> Vec<Event> {
    let mut s = state.clone();
    let mut out = Vec::new();
    let Some(apps) = s.apps.clone() else {
        return out;
    };
    if s.ask.is_some() || s.plan.is_some() || s.task_status.is_some() {
        return out;
    }
    // Five node types; a bound so a bug can't loop forever.
    for _ in 0..8 {
        let chosen = s.chosen();
        let e = match layers::next_question(goal, &apps, &chosen) {
            Next::Ask(q) => decide(&q, goal, &chosen, scorer, clock, format!("d{}", s.decisions_made)),
            Next::Plan(steps) => Event::PlanProduced {
                irreversible: plan_irreversible(&s, &steps),
                steps,
                model: scorer.model(),
            },
        };
        s = apply(s, &e);
        let stop = matches!(e, Event::PlanProduced { .. }) || s.ask.is_some();
        out.push(e);
        if stop {
            break;
        }
    }
    out
}

/// Any chosen option marked irreversible makes the plan irreversible (slice 6's confirm gate asks first).
fn plan_irreversible(s: &CtrlState, steps: &[crate::log::PlanStep]) -> bool {
    steps.iter().any(|st| {
        s.decisions
            .iter()
            .filter(|d| d.node == st.node)
            .any(|d| d.options.iter().any(|o| o.id == st.option && o.irreversible))
    })
}

/// The first record whose decision can't be re-derived from the log before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub seq: u64,
}

/// Exact replay check (spec §13.4): fold the log and, at every `decision.made` / `plan.produced`, rebuild the
/// question from the state before it. The node, option ids, refs and input hash must match. No scorer runs.
pub fn replay_check(log: &EventLog, goal: &Goal) -> Result<(), Divergence> {
    let mut s = CtrlState::default();
    for r in log.records() {
        let div = Divergence { seq: r.seq };
        match &r.event {
            Event::DecisionMade {
                node,
                options,
                input_hash: h,
                ..
            } => {
                let apps = s.apps.clone().ok_or(div.clone())?;
                let chosen = s.chosen();
                match layers::next_question(goal, &apps, &chosen) {
                    Next::Ask(q)
                        if q.node == *node
                            && q.choices.iter().map(|c| &c.opt).eq(options.iter())
                            && input_hash(goal, *node, options, &chosen) == *h => {}
                    _ => return Err(div),
                }
            }
            Event::PlanProduced { steps, .. } => {
                let apps = s.apps.clone().ok_or(div.clone())?;
                match layers::next_question(goal, &apps, &s.chosen()) {
                    Next::Plan(p) if p == *steps => {}
                    _ => return Err(div),
                }
            }
            _ => {}
        }
        s = apply(s, &r.event);
    }
    Ok(())
}

/// Why a tentative choice is being undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BacktrackReason {
    VerifyFailed,
    ActionFailed,
}

/// After a failed verify (slice 5): pop the latest tentative decision's next alternative, within the budgets
/// (2 per decision, 6 per task). `None` = out of alternatives or budget: ask the user.
pub fn next_backtrack(s: &CtrlState, reason: BacktrackReason) -> Option<Event> {
    if s.backtracks >= BACKTRACKS_PER_TASK {
        return None;
    }
    let d = s
        .decisions
        .iter()
        .rev()
        .find(|d| !d.alternatives.is_empty() && d.backtracks < BACKTRACKS_PER_DECISION)?;
    Some(Event::Backtrack {
        decision_id: d.id.clone(),
        to: d.alternatives[0].clone(),
        reason,
    })
}
