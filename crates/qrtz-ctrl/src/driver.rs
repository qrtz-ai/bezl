//! The loop (ADR 0214): ask [`chain::next`] what the run needs, do it through the helper, append the result, and
//! repeat until Jared must answer (an ask or a confirm) or the run ends. The log is the only state: the driver
//! can be dropped and rebuilt from the log and the goal, which is how `ctrl:answer` and `ctrl:confirm` resume a run.
//!
//! Confirm gates: a rule from [`crate::gate::rules_for`] holds the action. Jared approves a [`Binding`] (element,
//! action, tree hash); right before acting the tree is read again and the approval stands only if it hashes the
//! same, otherwise the gate asks again. The scorer never lowers a rule.
use crate::act::Action;
use crate::ax::{diff, find, prune, shortcuts_obs, wire_labels};
use crate::chain::{next, position, Next};
use crate::decide::{decide, next_backtrack, BacktrackReason, Goal, Heuristic, Scorer};
use crate::gate::{rules_for, Binding, ConfirmAnswer};
use crate::guard::StopReason;
use crate::helper::HelperClient;
use crate::log::{CtrlState, Event, EventLog, Phase, TaskStatus};
use crate::runner::{Clock, Ran, Runner};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Hard cap on loop turns, whatever the chain says (a stuck helper can't spin the run forever).
const MAX_TURNS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The run ended.
    Finished(TaskStatus),
    /// Waiting for Jared to pick (the decision is `state.ask`).
    Asking,
    /// Waiting for Jared to approve or deny (the gate is `state.confirm`).
    Confirming,
}

pub struct Driver<'a> {
    pub log: &'a mut EventLog,
    pub helper: &'a HelperClient,
    pub scorer: &'a dyn Scorer,
    pub clock: &'a dyn Clock,
    pub goal: &'a str,
    /// Readable labels (handle or bundle id → text): memory only, for the UI and the Laya prompt.
    pub labels: &'a mut BTreeMap<String, String>,
    /// The stop button: checked between steps.
    pub stop: &'a AtomicBool,
}

impl Driver<'_> {
    fn push(&mut self, e: Event) {
        let (m, w) = (self.clock.mono_ns(), self.clock.wall_ms());
        self.log.append(m, w, e);
    }

    fn finish(&mut self, status: TaskStatus, stop: Option<StopReason>) -> Step {
        self.push(Event::TaskFinished { status, stop });
        let run = self.log.run_id().to_string();
        let _ = self.helper.observe(&run, Some(false));
        Step::Finished(status)
    }

    fn pause(&mut self, step: Step) -> Step {
        let run = self.log.run_id().to_string();
        let _ = self.helper.observe(&run, Some(false));
        step
    }

    /// Run until Jared must answer or the run ends.
    pub fn drive(&mut self) -> Step {
        for _ in 0..MAX_TURNS {
            let st = self.log.state();
            if let Some(s) = st.status {
                return Step::Finished(s);
            }
            if self.stop.load(Ordering::SeqCst) {
                return self.finish(TaskStatus::Stopped, Some(StopReason::StopButton));
            }
            let goal = Goal { text: self.goal };
            match next(&goal, &st, &*self.labels) {
                Next::Wait => {
                    if st.ask.is_some() {
                        return self.pause(Step::Asking);
                    }
                    if st.confirm.is_some() {
                        return self.pause(Step::Confirming);
                    }
                    return self.finish(TaskStatus::Failed, None);
                }
                Next::Catalog => match self.helper.catalog() {
                    Ok(c) => {
                        for a in &c.apps {
                            self.labels.insert(a.id.clone(), a.name.clone());
                        }
                        self.push(Event::ObsApps { apps: c.observe() });
                    }
                    Err(_) => return self.finish(TaskStatus::Failed, None),
                },
                Next::Tree { app, scope } => self.read_tree(&st, app, scope),
                Next::Shortcuts { app } => match self.helper.menu_shortcuts(&app) {
                    Ok(items) => {
                        for m in &items {
                            if let Some(t) = m.path.last() {
                                self.labels.insert(m.id.clone(), t.clone());
                            }
                        }
                        self.push(Event::ObsShortcuts {
                            app,
                            items: shortcuts_obs(&items),
                            err: None,
                        });
                    }
                    Err(e) => self.push(Event::ObsShortcuts {
                        app,
                        items: vec![],
                        err: Some(e),
                    }),
                },
                Next::Ask(q) => {
                    let id = format!("d{}", st.decisions_made);
                    let e = decide(&q, &goal, &position(&st), self.scorer, self.clock, id);
                    self.push(e);
                    if self.log.state().ask.is_some() {
                        return self.pause(Step::Asking);
                    }
                }
                Next::Act(a) => {
                    if let Some(step) = self.act_step(&st, &a) {
                        return step;
                    }
                }
                Next::Backtrack(reason) => match next_backtrack(&st, reason) {
                    Some(e) => self.push(e),
                    None => {
                        let status = if reason == BacktrackReason::NoneOfThese {
                            TaskStatus::AskedOut
                        } else {
                            TaskStatus::Failed
                        };
                        return self.finish(status, None);
                    }
                },
                Next::Finish(status) => return self.finish(status, None),
            }
        }
        self.finish(TaskStatus::OutOfSteps, None)
    }

    fn read_tree(&mut self, st: &CtrlState, app: String, scope: Option<String>) {
        match self.helper.tree(&app, scope.as_deref()) {
            Ok(t) => {
                wire_labels(&t.root, self.labels);
                let obs = prune(&app, scope.as_deref(), &t.root, t.focused.as_deref());
                let after_action = st.phase == Phase::AfterAction && scope.is_none();
                let diff = after_action
                    .then(|| st.tree.as_ref().map(|b| diff(b, &obs)))
                    .flatten();
                let action_id = st.last_action().map(|a| a.id.clone());
                self.push(Event::ObsTree {
                    app,
                    scope,
                    tree: Some(obs),
                    err: None,
                });
                if let (Some(d), Some(id)) = (diff, action_id) {
                    self.push(Event::ObsDiff {
                        action_id: id,
                        diff: d,
                    });
                }
            }
            Err(e) => self.push(Event::ObsTree {
                app,
                scope,
                tree: None,
                err: Some(e),
            }),
        }
    }

    fn binding(st: &CtrlState, a: &Action) -> Binding {
        Binding {
            app: st.app.clone().unwrap_or_default(),
            handle: a.handle().map(str::to_string),
            action: a.binding_key(),
            tree_hash: st.tree.as_ref().map_or_else(|| "-".into(), |t| t.hash.clone()),
        }
    }

    /// `Some`: the run stops here (confirming or finished). `None`: carry on.
    fn act_step(&mut self, st: &CtrlState, a: &Action) -> Option<Step> {
        let app = st.app.clone().unwrap_or_default();
        let node = a
            .handle()
            .and_then(|h| st.tree.as_ref().and_then(|t| find(&t.root, h)));
        // A menu item's label rules (it isn't in the window's tree) count the same as an element's.
        let menu_risk: Vec<String> = match (a, node) {
            (Action::Press { handle }, None) => st
                .shortcuts
                .get(&app)
                .and_then(|m| m.iter().find(|x| &x.id == handle))
                .map(|x| x.risk.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let rules = rules_for(a, node, &app, &menu_risk);
        if !rules.is_empty() {
            let binding = Self::binding(st, a);
            match &st.confirm {
                None => {
                    self.push(Event::ConfirmRequested {
                        gate_id: format!("g{}", st.gates),
                        binding,
                        rules,
                    });
                    return Some(self.pause(Step::Confirming));
                }
                Some(g) if g.answer == Some(ConfirmAnswer::Deny) => {
                    return Some(self.finish(TaskStatus::Cancelled, None));
                }
                Some(g) if g.answer.is_none() => return Some(self.pause(Step::Confirming)),
                Some(g) if g.reverified != Some(true) => {
                    // Approved: read the tree again; the approval stands only if nothing moved.
                    let gate_id = g.id.clone();
                    self.read_tree(st, app.clone(), None);
                    let now = self.log.state();
                    let ok = Self::binding(&now, a) == binding && now.tree.as_ref().is_some();
                    self.push(Event::ConfirmReverified { gate_id, ok });
                    return None;
                }
                Some(g) if !g.allows(&binding) => {
                    let gate_id = g.id.clone();
                    self.push(Event::ConfirmReverified { gate_id, ok: false });
                    return None;
                }
                Some(_) => {}
            }
        }
        let action_id = format!("a{}", st.actions.len());
        let ran = Runner {
            log: self.log,
            helper: self.helper,
            clock: self.clock,
            goal: self.goal,
        }
        .run(&action_id, a);
        match ran {
            Ran::Stopped(r) => Some(self.finish(TaskStatus::Stopped, Some(r))),
            Ran::Refused | Ran::Acted => None,
        }
    }
}

/// Replay (spec §13): every decision's question, rebuilt from the log up to it, has the same options and input
/// hash, and a heuristic decision comes out the same. A changed prior or option source shows up here.
pub fn replay_check(log: &EventLog, goal: &str) -> Result<(), String> {
    struct Zero;
    impl Clock for Zero {
        fn mono_ns(&self) -> u64 {
            0
        }
        fn wall_ms(&self) -> u64 {
            0
        }
    }
    let labels: BTreeMap<String, String> = BTreeMap::new();
    let g = Goal { text: goal };
    for (i, r) in log.records().iter().enumerate() {
        let Event::DecisionMade {
            decision_id,
            options,
            input_hash,
            chosen,
            p_bp,
            model,
            node,
            ..
        } = &r.event
        else {
            continue;
        };
        let st = log.state_at(i);
        let Next::Ask(q) = next(&g, &st, &labels) else {
            return Err(format!("{decision_id}: the chain asks nothing here"));
        };
        let ids: Vec<&String> = q.choices.iter().map(|c| &c.opt.id).collect();
        if ids != options.iter().map(|o| &o.id).collect::<Vec<_>>() || q.node != *node {
            return Err(format!("{decision_id}: options differ"));
        }
        let e = decide(&q, &g, &position(&st), &Heuristic, &Zero, decision_id.clone());
        if let Event::DecisionMade {
            input_hash: h,
            chosen: c,
            p_bp: p,
            ..
        } = e
        {
            if &h != input_hash {
                return Err(format!("{decision_id}: input hash differs"));
            }
            if model.id == crate::decide::HEURISTIC_ID && (&c != chosen || &p != p_bp) {
                return Err(format!("{decision_id}: heuristic decision differs"));
            }
        }
    }
    Ok(())
}
