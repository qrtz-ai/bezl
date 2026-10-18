//! The Laya backend interface (slice 4, ADR 0106). Laya (Convai Innovations, Apache-2.0) is a small encoder that
//! answers typed questions (choice / score / yes-no) with calibrated probabilities in one forward pass; its Core ML
//! port runs on the Mac. ctrl uses it only when the user has downloaded it from onboarding or settings (consent,
//! ADR 0076): there is no download here and no network ever.
//!
//! This module is pure and builds everywhere: the request format ([`laya_request`]) and the adapter from a runtime's
//! probabilities to the chain's logits ([`LayaScorer`]). The runtime itself (Core ML, in the Swift helper) is
//! behind [`LayaRuntime`] and arrives with the helper protocol in slice 5. [`select_scorer`] only ever picks Laya on
//! macOS with the `laya` feature, a consented and installed model, and a runtime; otherwise the heuristic layer.
use crate::decide::{Goal, Heuristic, ModelId, Node, Scorer};
use crate::layers::{label, Question};
use std::path::PathBuf;

/// What Laya reads for a `choice` question. In memory only: never logged, never written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayaRequest {
    pub state: String,
    pub question: String,
    pub options: Vec<String>,
}

fn question_text(node: Node) -> &'static str {
    match node {
        Node::ChooseIntent => "what kind of task is this?",
        Node::ChooseApp => "which app should do it?",
        Node::ChooseTarget => "what should it act on?",
        Node::ChooseText => "what should be typed?",
        Node::ChooseMove => "which move does it?",
        Node::ChooseControl => "which control in it?",
    }
}

/// The typed `choice` request for one question: the goal as the state, the node's question, the options' labels.
pub fn laya_request(q: &Question, goal: &Goal) -> LayaRequest {
    LayaRequest {
        state: format!("task: {}", goal.text),
        question: question_text(q.node).into(),
        options: q.choices.iter().map(|c| label(&c.opt, Some(goal))).collect(),
    }
}

/// Runs the model. Returns one probability per option (same order).
pub trait LayaRuntime: Send + Sync {
    fn model(&self) -> ModelId;
    fn choose(&self, r: &LayaRequest) -> Result<Vec<f32>, String>;
}

/// The chain's scorer over a Laya runtime: probabilities become milli-logits (ln p), so the chain's softmax at
/// T = 1 gives the model's distribution back.
pub struct LayaScorer<R: LayaRuntime> {
    pub runtime: R,
}

impl<R: LayaRuntime> Scorer for LayaScorer<R> {
    fn model(&self) -> ModelId {
        self.runtime.model()
    }
    fn logits(&self, q: &Question, goal: &Goal) -> Result<Vec<i64>, String> {
        let p = self.runtime.choose(&laya_request(q, goal))?;
        if p.len() != q.choices.len() || p.iter().any(|x| !x.is_finite() || *x < 0.0) {
            return Err("bad_output".into());
        }
        Ok(p.iter()
            .map(|x| (f64::from(x.max(1e-6)).ln() * 1000.0).round() as i64)
            .collect())
    }
}

/// Whether the user downloaded the model (onboarding / settings) and where it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelConsent {
    pub consented: bool,
    pub installed: Option<PathBuf>,
}

/// Why the heuristic layer answers instead of Laya.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// Not macOS, or built without the `laya` feature.
    NotThisBuild,
    NoConsent,
    NotInstalled,
    NoRuntime,
}

/// Pick the scorer: Laya only with consent, an installed model and a runtime, on a macOS `laya` build.
pub fn select_scorer(
    consent: &ModelConsent,
    runtime: Option<Box<dyn LayaRuntime>>,
) -> (Box<dyn Scorer>, Option<Fallback>) {
    #[cfg(all(target_os = "macos", feature = "laya"))]
    {
        if !consent.consented {
            return (Box::new(Heuristic), Some(Fallback::NoConsent));
        }
        if !consent.installed.as_ref().is_some_and(|p| p.exists()) {
            return (Box::new(Heuristic), Some(Fallback::NotInstalled));
        }
        match runtime {
            Some(rt) => (Box::new(LayaScorer { runtime: rt }), None),
            None => (Box::new(Heuristic), Some(Fallback::NoRuntime)),
        }
    }
    #[cfg(not(all(target_os = "macos", feature = "laya")))]
    {
        let _ = (consent, runtime);
        (Box::new(Heuristic), Some(Fallback::NotThisBuild))
    }
}

impl LayaRuntime for Box<dyn LayaRuntime> {
    fn model(&self) -> ModelId {
        (**self).model()
    }
    fn choose(&self, r: &LayaRequest) -> Result<Vec<f32>, String> {
        (**self).choose(r)
    }
}

/// The Laya runtime behind the ctrl helper protocol (slice 5, ADR 0108): one `ctrl.laya` request per question, run
/// by the helper's Core ML stack on the consented, installed model. The helper answers `laya.no_model` until the
/// model is downloaded from onboarding / settings; [`decide::decide`](crate::decide::decide) then falls back to the
/// heuristic layer for that question and the log says so. Selecting it still goes through [`select_scorer`].
pub struct HelperLaya {
    pub helper: std::sync::Arc<crate::helper::HelperClient>,
    pub model: ModelId,
}

impl LayaRuntime for HelperLaya {
    fn model(&self) -> ModelId {
        self.model.clone()
    }
    fn choose(&self, r: &LayaRequest) -> Result<Vec<f32>, String> {
        self.helper.laya(&r.state, &r.question, &r.options)
    }
}
