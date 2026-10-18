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

/// Whether this build can run Laya at all (macOS with the `laya` feature); elsewhere the heuristic always decides.
pub const RUNTIME_BUILD: bool = cfg!(all(target_os = "macos", feature = "laya"));

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
    /// The consented, installed model the host hands the helper with every question (ADR 0116); `None` sends no
    /// path, and a helper started without `QRTZ_CTRL_LAYA_MODEL` then answers `laya.no_model`.
    pub path: Option<PathBuf>,
}

impl LayaRuntime for HelperLaya {
    fn model(&self) -> ModelId {
        self.model.clone()
    }
    fn choose(&self, r: &LayaRequest) -> Result<Vec<f32>, String> {
        self.helper
            .laya(self.path.as_deref(), &r.state, &r.question, &r.options)
    }
}

/// Run 19: the scorer re-selected per question, so a model downloaded (or switched off) while the app runs takes
/// effect at the next question instead of the next app start. Each question reads the consent (a file read, no
/// network): with consent, an installed model on disk and a runtime build, the Laya model answers through the helper
/// with that path; otherwise the heuristic layer does, exactly as [`select_scorer`] would have chosen at start.
pub struct LiveScorer {
    pub consent: Box<dyn Fn() -> ModelConsent + Send + Sync>,
    pub helper: std::sync::Arc<crate::helper::HelperClient>,
    pub model: ModelId,
    /// [`RUNTIME_BUILD`] in the app; tests set it to exercise the switch on Linux.
    pub runtime_build: bool,
}

impl LiveScorer {
    fn active(&self) -> Option<PathBuf> {
        if !self.runtime_build {
            return None;
        }
        let c = (self.consent)();
        c.installed.filter(|p| c.consented && p.exists())
    }
}

impl Scorer for LiveScorer {
    fn model(&self) -> ModelId {
        match self.active() {
            Some(_) => self.model.clone(),
            None => Heuristic.model(),
        }
    }
    fn logits(&self, q: &Question, goal: &Goal) -> Result<Vec<i64>, String> {
        match self.active() {
            Some(path) => LayaScorer {
                runtime: HelperLaya {
                    helper: self.helper.clone(),
                    model: self.model.clone(),
                    path: Some(path),
                },
            }
            .logits(q, goal),
            None => Heuristic.logits(q, goal),
        }
    }
}

/// ADR 0133: warm the Laya model on the helper as soon as it can be used, so the first question of a run doesn't
/// pay the tokenizer read and the Core ML load (and, once, the compile). The app calls [`LayaWarmer::tick`] from
/// a low-priority thread every few seconds. A tick sends `ctrl.laya.warm` only when the helper is up, this build
/// has the runtime, the user consented and the installed model is on disk, and only once per helper start and
/// model path: a helper restart, or a download that finishes while the app runs, warms again; nothing else does.
/// The helper answers at once and does the work on its own background queue, so neither the host nor echo/nano
/// wait for it; nothing is downloaded and there is no network.
pub struct LayaWarmer {
    pub consent: Box<dyn Fn() -> ModelConsent + Send + Sync>,
    pub helper: std::sync::Arc<crate::helper::HelperClient>,
    /// [`RUNTIME_BUILD`] in the app; tests set it to exercise the warm-up on Linux.
    pub runtime_build: bool,
    /// Ticks the helper must have been up before the warm-up is sent (0: at the first tick), so echo's and nano's
    /// own start (their model loads) has the machine first.
    pub settle_ticks: u32,
    warmed: std::sync::Mutex<(u32, Option<PathBuf>)>,
}

impl LayaWarmer {
    pub fn new(
        consent: Box<dyn Fn() -> ModelConsent + Send + Sync>,
        helper: std::sync::Arc<crate::helper::HelperClient>,
        runtime_build: bool,
    ) -> Self {
        LayaWarmer {
            consent,
            helper,
            runtime_build,
            settle_ticks: 0,
            warmed: std::sync::Mutex::new((0, None)),
        }
    }

    pub fn with_settle(mut self, ticks: u32) -> Self {
        self.settle_ticks = ticks;
        self
    }

    /// One check. `None`: nothing sent (helper down, no runtime, no consent, no model on disk, or already warmed
    /// for this helper start). `Some(reply)`: the helper's answer to the one `ctrl.laya.warm` sent (a state, or an
    /// error code such as `laya.no_model` or `helper.timeout`; either way it isn't sent again until the helper
    /// restarts or the model path changes, so an older helper is asked once, not every tick).
    pub fn tick(&self) -> Option<Result<String, String>> {
        let mut g = self.warmed.lock().unwrap_or_else(|e| e.into_inner());
        let (up_ticks, warmed) = &mut *g;
        if !self.helper.is_up() {
            *up_ticks = 0;
            *warmed = None;
            return None;
        }
        if !self.runtime_build {
            return None;
        }
        if *up_ticks < self.settle_ticks {
            *up_ticks += 1;
            return None;
        }
        let c = (self.consent)();
        let Some(path) = c.installed.filter(|p| c.consented && p.exists()) else {
            // An off (or a deleted model) forgets the warm-up, so a later yes + download warms again.
            *warmed = None;
            return None;
        };
        if warmed.as_ref() == Some(&path) {
            return None;
        }
        *warmed = Some(path.clone());
        Some(self.helper.laya_warm(&path))
    }
}
