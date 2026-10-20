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
use crate::decide::{decide, next_backtrack, BacktrackReason, Goal, ModelStatus, Scorer};
use crate::gate::{rules_for, Binding, ConfirmAnswer};
use crate::guard::StopReason;
use crate::helper::HelperClient;
use crate::log::{CtrlState, Event, EventLog, Phase, TaskStatus};
use crate::runner::{Clock, Ran, Runner};
use sha2::Digest;
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
        // QRT-293: no model, no run. Nothing is read or acted on until the decision model is ready; the run ends
        // as failed and `ctrl:model` says why (the "download to start" screen).
        if self.log.state().status.is_none() && self.scorer.status() != ModelStatus::Ready {
            return self.finish(TaskStatus::Failed, None);
        }
        if self.log.state().status.is_none() {
            // Review H-A: watch from the start of every drive (a new run, or a resume after an answer), not only at
            // act time, so Esc right after the ctrl+fn release, or while the first tree is read, stops the run.
            // The disarm first ends any run the watcher still holds (a host that crashed mid-run), so its latched
            // stop can't block this one; the arm then clears the flags once, for this run.
            let run = self.log.run_id().to_string();
            let _ = self.helper.observe(&run, Some(false));
            let o = self.helper.observe(&run, Some(true)).unwrap_or_default();
            if let Some(stop) = crate::guard::verdict(&o) {
                return self.finish(TaskStatus::Stopped, Some(stop));
            }
        }
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
                    // QRT-293: the model only proposes; a failed or unready model ends the run with no action.
                    let Ok(e) = decide(&q, &goal, &position(&st), self.scorer, self.clock, id) else {
                        return self.finish(TaskStatus::Failed, None);
                    };
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
        let t = st.tree.as_ref();
        // The element acted on, or for a key combo the focused one; a menu item (not in the tree) by its id.
        let target = a
            .handle()
            .map(str::to_string)
            .or_else(|| t.and_then(|t| t.focused.clone()));
        // A menu item (not in the tree) or a key combo binds to the item and to the window it acts on: the
        // window's own role and label and the focused element's fingerprint.
        let window = |t: &crate::ax::TreeObs| {
            let f = t
                .focused
                .as_deref()
                .and_then(|f| crate::ax::target_fingerprint(&t.root, f))
                .unwrap_or_default();
            format!("{}|{}|{f}", t.root.role, t.root.tokens.join(","))
        };
        let fp = match (t, target.as_deref()) {
            (Some(t), Some(h)) => crate::ax::target_fingerprint(&t.root, h).unwrap_or_else(|| {
                let d = sha2::Sha256::digest(format!("{h}|{}", window(t)).as_bytes());
                format!("menu:{}", hex::encode(&d[..8]))
            }),
            (Some(t), None) => {
                let d = sha2::Sha256::digest(window(t).as_bytes());
                format!("win:{}", hex::encode(&d[..8]))
            }
            _ => "-".into(),
        };
        Binding {
            app: st.app.clone().unwrap_or_default(),
            handle: a.handle().map(str::to_string),
            action: a.binding_key(),
            tree_hash: fp,
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
            // Review M3: a key combo that is a menu item's shortcut inherits that item's label rules.
            (Action::KeyCombo { combo, .. }, _) => {
                let c = crate::act::normalize_combo(combo);
                st.shortcuts
                    .get(&app)
                    .into_iter()
                    .flatten()
                    .filter(|x| crate::act::normalize_combo(&x.combo) == c)
                    .flat_map(|x| x.risk.iter().cloned())
                    .collect()
            }
            _ => Vec::new(),
        };
        // Review H2: the target of a submit is the field typed into, or the focused element for a bare Return.
        let target = node.or_else(|| {
            st.tree
                .as_ref()
                .and_then(|t| t.focused.as_deref().and_then(|f| find(&t.root, f)))
        });
        let last = st
            .actions
            .iter()
            .rev()
            .find(|x| x.status == crate::log::ActionStatus::Done)
            .map(|x| &x.action);
        let browser = st
            .apps
            .as_ref()
            .is_some_and(|x| x.apps.iter().any(|x| x.id == app && x.browser));
        let exempt = crate::gate::submit_exempt(target, last, browser);
        let mut extra = menu_risk;
        if let (Some(h), Some(t)) = (a.handle(), st.tree.as_ref()) {
            extra.extend(crate::gate::context_rules(&t.root, h));
        }
        let rules = rules_for(a, node, &app, &extra, exempt);
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
                    // Review M1: find the approved element again by its fingerprint (exactly one), not its old id.
                    let fresh = match (a.handle(), now.tree.as_ref()) {
                        (Some(_), Some(t)) if !binding.tree_hash.starts_with("menu:") => {
                            let hits = crate::ax::find_by_fingerprint(&t.root, &binding.tree_hash);
                            (hits.len() == 1).then(|| hits[0].clone())
                        }
                        _ => None,
                    };
                    let rebound = fresh.as_deref().map_or_else(|| a.clone(), |h| a.with_handle(h));
                    let ok = Self::binding(&now, &rebound) == binding && now.tree.as_ref().is_some();
                    self.push(Event::ConfirmReverified {
                        gate_id,
                        ok,
                        handle: fresh.filter(|_| ok),
                    });
                    return None;
                }
                Some(g) if !g.allows(&binding) => {
                    let gate_id = g.id.clone();
                    self.push(Event::ConfirmReverified {
                        gate_id,
                        ok: false,
                        handle: None,
                    });
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
/// hash. A changed option source shows up here. (QRT-293: there is no rules scorer to re-decide with; the model's
/// own choice is checked only against the options it was offered.)
pub fn replay_check(log: &EventLog, goal: &str) -> Result<(), String> {
    let labels: BTreeMap<String, String> = BTreeMap::new();
    let g = Goal { text: goal };
    for (i, r) in log.records().iter().enumerate() {
        let Event::DecisionMade {
            decision_id,
            options,
            input_hash,
            chosen,
            p_bp,
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
        if &crate::decide::input_hash(
            &g,
            q.node,
            &q.choices.iter().map(|c| c.opt.clone()).collect::<Vec<_>>(),
            &position(&st),
        ) != input_hash
        {
            return Err(format!("{decision_id}: input hash differs"));
        }
        if !options.iter().any(|o| &o.id == chosen) || p_bp.len() != options.len() {
            return Err(format!("{decision_id}: chose outside its options"));
        }
    }
    Ok(())
}
