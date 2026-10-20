//! Scoring one question of the chain (decision-layer spec §1, §5, §13; ADR 0106, kept by ADR 0214). A question has
//! at most [`MAX_OPTIONS`] enumerated options, the last one always "none of these". A [`Scorer`] (the deterministic
//! [`Heuristic`], or a consented Laya model) gives a milli-logit per option; a softmax at the node's temperature
//! gives calibrated probabilities in basis points (integers, so a record hashes the same after a JSON round trip).
//!
//! - p ≥ τ_act (0.80): act.
//! - τ_back (0.50) ≤ p < τ_act: act tentatively; the next two options are the backtrack stack (≤ 2 per decision,
//!   ≤ 6 per run).
//! - otherwise, or "none of these" won: ask Jared, showing the top three real options.
//!
//! Events hold ids, numbers and pointers only (ADR 0083); replay re-derives every question ([`crate::driver`]).
use crate::log::{DecisionState, Event};
use crate::runner::Clock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Target option count per question (spec §1), "none of these" included.
pub const MAX_OPTIONS: usize = 20;
/// Hard cap (spec §1).
pub const HARD_CAP: usize = 64;
/// At most this many backtracks per decision and per run (spec §5).
pub const BACKTRACKS_PER_DECISION: u32 = 2;
pub const BACKTRACKS_PER_TASK: u32 = 6;
/// The option every question ends with; choosing it backtracks or asks.
pub const NONE: &str = "none";
/// The fixed prior of "none of these".
pub const NONE_PRIOR: i64 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Node {
    /// Which app (installed or running) does it.
    ChooseApp,
    /// A menu-bar shortcut, a shortcut from the built-in table, or a top-level branch of the front window.
    ChooseMove,
    /// Inside a branch: descend into a child branch or group, or an (element, action) pair.
    ChooseElement,
    /// What to type into the focused field.
    ChooseText,
    /// After an action: done, keep going, or it didn't work.
    Verify,
}

impl Node {
    pub fn as_str(self) -> &'static str {
        match self {
            Node::ChooseApp => "choose_app",
            Node::ChooseMove => "choose_move",
            Node::ChooseElement => "choose_element",
            Node::ChooseText => "choose_text",
            Node::Verify => "verify",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    App,
    /// A menu-bar item with a key equivalent (pressed through its handle).
    MenuItem,
    /// A shortcut from the built-in table (sent as a key combo).
    Shortcut,
    /// Open an address in the chosen browser directly.
    OpenUrl,
    /// An element with children: descend.
    Branch,
    /// A cluster of same-role children ("list: 140 rows"): descend into it.
    Group,
    /// One (element, action) pair.
    ElementAction,
    TextCandidate,
    Verdict,
    None,
}

/// One enumerated option. `id` is stable and text-free (`app.com.apple.Safari`, `key.cmd+l`, `ax.e:4:12.press`);
/// text derived from the goal is a pointer into it (`label_ref`), never inline. `role` is the element's AX role
/// (the UI's label fallback when the readable title isn't in memory any more).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opt {
    pub id: String,
    pub kind: OptionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The host's confirm rules match this option (shown as "asks you first"); the gate decides at act time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gated: bool,
}

impl Opt {
    pub fn new(id: impl Into<String>, kind: OptionKind) -> Opt {
        Opt {
            id: id.into(),
            kind,
            label_ref: None,
            role: None,
            gated: false,
        }
    }
    pub fn none() -> Opt {
        Opt::new(NONE, OptionKind::None)
    }
}

/// One option with its heuristic prior (milli-logits) and its readable label (memory only: never logged; the Laya
/// prompt and the UI read it).
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub opt: Opt,
    pub prior: i64,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub node: Node,
    pub choices: Vec<Choice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Act,
    Tentative,
    Ask,
}

/// Basis points (0…10000), per node type (spec §5; to be fitted on the Mac replay set).
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

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.id, self.version)
    }
}

/// The goal: what Jared typed (from the store, in memory only).
#[derive(Debug, Clone, Copy)]
pub struct Goal<'a> {
    pub text: &'a str,
}

/// Scores one question: a logit (milli-units) per option, same order.
pub trait Scorer: Send + Sync {
    fn model(&self) -> ModelId;
    fn logits(&self, q: &Question, goal: &Goal) -> Result<Vec<i64>, String>;
    /// Temperature (milli) per node type; 1000 until fitted.
    fn temperature_milli(&self, _node: Node) -> u32 {
        1000
    }
}

pub const HEURISTIC_ID: &str = "qrtz-heuristic";
/// Version 2: the ctrl-do chain (ADR 0214). Changing a prior changes replay, which the replay check reports.
pub const HEURISTIC_VERSION: &str = "2";

/// The deterministic layer: the option sources' priors are the logits. No model, no download.
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

/// Index of the highest probability (the first on ties, so option order breaks ties deterministically).
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
pub fn ranked(p: &[u32]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..p.len()).collect();
    idx.sort_by(|a, b| p[*b].cmp(&p[*a]));
    idx
}

/// Hash of what a question was asked over: node, option ids and refs, the run's position and the goal's hash.
/// Replay recomputes it; a difference is a divergence.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "serde_json on a plain derived type with string keys cannot fail"
)]
pub fn input_hash(goal: &Goal, node: Node, options: &[Opt], position: &str) -> String {
    #[derive(Serialize)]
    struct In<'a> {
        node: Node,
        options: Vec<&'a str>,
        refs: Vec<Option<&'a str>>,
        position: &'a str,
        goal: String,
    }
    let body = In {
        node,
        options: options.iter().map(|o| o.id.as_str()).collect(),
        refs: options.iter().map(|o| o.label_ref.as_deref()).collect(),
        position,
        goal: hex::encode(Sha256::digest(goal.text.as_bytes())),
    };
    let h = Sha256::digest(serde_json::to_vec(&body).expect("serializes"));
    hex::encode(&h[..8])
}

/// Answer one question: score, calibrate, pick, classify. Pure except for the scorer and the clock.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "the built-in heuristic scorer is infallible; it is the fallback when the model fails"
)]
pub fn decide(
    q: &Question,
    goal: &Goal,
    position: &str,
    scorer: &dyn Scorer,
    clock: &dyn Clock,
    decision_id: String,
) -> Event {
    let options: Vec<Opt> = q.choices.iter().map(|c| c.opt.clone()).collect();
    assert!(
        !options.is_empty() && options.len() <= HARD_CAP,
        "a question has 1..={HARD_CAP} options"
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
        input_hash: input_hash(goal, q.node, &options, position),
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
        latency_us,
    }
}

/// The top three real options of an asked decision (what Jared picks from).
pub fn ask_options(d: &DecisionState) -> Vec<usize> {
    ranked(&d.p_bp)
        .into_iter()
        .filter(|&i| d.options[i].kind != OptionKind::None)
        .take(3)
        .collect()
}

/// Why a tentative choice is being undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BacktrackReason {
    VerifyFailed,
    ActionFailed,
    NoneOfThese,
}

/// Pop the latest tentative decision's next alternative, within the budgets (2 per decision, 6 per run). `None`:
/// out of alternatives or budget.
pub fn next_backtrack(s: &crate::log::CtrlState, reason: BacktrackReason) -> Option<Event> {
    if s.backtracks >= BACKTRACKS_PER_TASK {
        return None;
    }
    let d = s
        .decisions
        .iter()
        .rev()
        .find(|d| !d.alternatives.is_empty() && d.backtracks < BACKTRACKS_PER_DECISION)?;
    // ADR 0214 review (b): an action already ran since this decision (a step whose done-check failed, at any
    // confidence, p >= 0.8 included): undoing the choice can't undo the action, so no backtrack; the run ends
    // and hands control back.
    if s.executed() > d.actions_before {
        return None;
    }
    Some(Event::Backtrack {
        decision_id: d.id.clone(),
        to: d.alternatives[0].clone(),
        reason,
    })
}
