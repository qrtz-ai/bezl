//! The executor (slice 5, ADR 0108): a produced plan becomes ladder routes (ADR 0100), each route runs through the
//! helper protocol between two guard snapshots ([`crate::runner::Runner`]), a verify step checks the move did what
//! it meant, and a failed verify (or a failed / refused action) pops the backtrack stack ([`next_backtrack`]) and
//! re-decides, or asks the user when there's nothing left to try.
//!
//! Mapping a move (`route.*`, `shortcut.*`, `ax.*`) to routes is pure ([`step_for`]): the same state gives the same
//! routes, so a replay sees why each action was requested. Nothing here can activate an app, move the cursor or
//! type into the user's focus: the routes are the ladder's, the ladder judges them against the helper's `Env`, and
//! the helper judges them again before acting.
//!
//! Not executed in this slice: **irreversible** plans (quit, empty trash, a system setting, send) stop with
//! `exec.held { irreversible }` before any action, for slice 6's confirm gate; **file routes** (`files.*`) stop with
//! `exec.held { files_not_yet }` until slice 7 expands a folder into moves.
use crate::ax::{diff, AxSnapshot};
use crate::decide::{next_backtrack, run_chain_obs, unobserved, BacktrackReason, Goal, Node, Scorer};
use crate::guard::{Field, UiSnapshot};
use crate::helper::{Check, Expect, HelperClient};
use crate::ladder::{AxOp, ContentRef, Env, Route, ScriptVerb, WorkspaceOp};
use crate::layers::{branch_element, chosen_app, find_snapshot, label};
use crate::log::{CtrlState, Event, EventLog, TaskStatus};
use crate::runner::{Clock, Executor, RunOutcome, Runner, UiProbe};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldReason {
    /// The plan can't be undone: slice 6's confirm gate asks first.
    Irreversible,
    /// File routes run in slice 7 (folder → moves).
    FilesNotYet,
    /// The move has no background route (it would need focus or typing): ask the user.
    NoRoute,
    /// The plan's app isn't known (no default browser reported, an element that's gone).
    UnknownApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyOutcome {
    Passed,
    Failed,
    /// The helper couldn't measure (no Automation grant to count tabs, …): the action stands, unverified.
    Unverifiable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Verify {
    pub check: Check,
    pub expect: Expect,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Run these routes (the ladder picks one), then verify. `content` resolves the routes' refs for the helper
    /// call only (the URL to open); the log keeps the refs.
    Act {
        routes: Vec<Route>,
        verify: Verify,
        content: BTreeMap<String, String>,
    },
    Hold(HoldReason),
}

fn chosen_opt(s: &CtrlState, node: Node) -> Option<&crate::decide::Opt> {
    let d = s.decisions.iter().rev().find(|d| d.node == node)?;
    d.options.iter().find(|o| o.id == d.chosen)
}

/// The step a plan's move maps to. Pure.
pub fn step_for(s: &CtrlState, goal: &Goal) -> Option<Step> {
    let plan = s.plan.as_ref()?;
    if plan.irreversible {
        return Some(Step::Hold(HoldReason::Irreversible));
    }
    let apps = s.apps.clone().unwrap_or_default();
    let chosen = s.chosen();
    let mv = chosen.get(&Node::ChooseMove)?.as_str();
    if mv.starts_with("route.files.") {
        return Some(Step::Hold(HoldReason::FilesNotYet));
    }
    let Some(app) = chosen_app(&apps, &chosen) else {
        return Some(Step::Hold(HoldReason::UnknownApp));
    };
    let tabs = Check::TabCount { app: app.clone() };
    let windows = Check::WindowCount { app: app.clone() };
    let key = |combo: &str| Route::PidKey {
        app: app.clone(),
        combo: combo.into(),
    };
    let script = |verb| Route::Scripting {
        app: app.clone(),
        verb,
    };
    let browser =
        apps.apps.iter().any(|a| a.id == app && a.browser) || apps.default_browser.as_deref() == Some(&app);
    let act = |routes: Vec<Route>, check: Check, expect: Expect| Step::Act {
        routes,
        verify: Verify { check, expect },
        content: BTreeMap::new(),
    };
    let step = match mv {
        "route.scripting.close_all_tabs" => act(vec![script(ScriptVerb::CloseAllTabs)], tabs, Expect::Zero),
        "route.scripting.close_tab" => act(
            vec![script(ScriptVerb::CloseTab), key("cmd+w")],
            tabs,
            Expect::Decreased,
        ),
        "route.scripting.close_window" => act(
            vec![script(ScriptVerb::CloseWindow), key("cmd+w")],
            windows,
            Expect::Decreased,
        ),
        "shortcut.cmd+w" if browser => act(vec![key("cmd+w")], tabs, Expect::Decreased),
        "shortcut.cmd+w" => act(vec![key("cmd+w")], windows, Expect::Decreased),
        "shortcut.cmd+option+w" => act(vec![key("cmd+option+w")], tabs, Expect::Decreased),
        // A focus-intent shortcut: the ladder refuses it (ADR 0100), which backtracks or asks.
        "shortcut.cmd+l" => act(vec![key("cmd+l")], tabs, Expect::Increased),
        "route.workspace.open_url" => {
            let Some(text) = chosen_opt(s, Node::ChooseText) else {
                return Some(Step::Hold(HoldReason::NoRoute));
            };
            // A search text has no address to open in the background.
            let (true, Some(r)) = (
                matches!(text.id.as_str(), "text.literal" | "text.url_guess"),
                text.label_ref.clone(),
            ) else {
                return Some(Step::Hold(HoldReason::NoRoute));
            };
            let addr = label(text, Some(goal));
            let url = if addr.contains("://") {
                addr
            } else {
                format!("https://{addr}")
            };
            Step::Act {
                routes: vec![Route::Workspace(WorkspaceOp::OpenUrl {
                    app: app.clone(),
                    url: ContentRef(r.clone()),
                })],
                verify: Verify {
                    check: tabs,
                    expect: Expect::Increased,
                },
                content: BTreeMap::from([(r, url)]),
            }
        }
        "route.workspace.launch_hidden" => act(
            vec![Route::Workspace(WorkspaceOp::LaunchHidden { app: app.clone() })],
            Check::AppRunning { app: app.clone() },
            Expect::One,
        ),
        m if m.starts_with("ax.") => {
            // The element: the control inside a chosen pane, or the chosen top-level element itself.
            let (el, scope) = match chosen.get(&Node::ChooseControl).and_then(|c| branch_element(c)) {
                Some(c) => (c.to_string(), branch_element(m).map(str::to_string)),
                None => (branch_element(m)?.to_string(), None),
            };
            let Some(node) = find_snapshot(&s.snapshots, &app, scope.as_deref()).and_then(|sn| sn.node(&el))
            else {
                return Some(Step::Hold(HoldReason::UnknownApp));
            };
            if !node.actions.iter().any(|a| a == "AXPress") {
                return Some(Step::Hold(HoldReason::NoRoute));
            }
            act(
                vec![Route::Ax {
                    app: app.clone(),
                    role: node.role.clone(),
                    element: ContentRef(el),
                    op: AxOp::Perform {
                        action: "AXPress".into(),
                    },
                }],
                Check::AxScope {
                    app: app.clone(),
                    scope,
                },
                Expect::Changed,
            )
        }
        // cmd-t opens an empty tab, find / new message need typing into the user's focus: no background route.
        _ => Step::Hold(HoldReason::NoRoute),
    };
    Some(step)
}

pub fn judge_verify(expect: Expect, before: Option<i64>, after: Option<i64>) -> VerifyOutcome {
    let ok = match (expect, before, after) {
        (Expect::Zero, _, Some(a)) => a == 0,
        (Expect::One, _, Some(a)) => a == 1,
        (Expect::Decreased, Some(b), Some(a)) => a < b,
        (Expect::Increased, Some(b), Some(a)) => a > b,
        _ => return VerifyOutcome::Unverifiable,
    };
    if ok {
        VerifyOutcome::Passed
    } else {
        VerifyOutcome::Failed
    }
}

struct HelperProbe<'a>(&'a HelperClient);
impl UiProbe for HelperProbe<'_> {
    fn snapshot(&self) -> UiSnapshot {
        // An unreachable helper reads as "unknown" on every field: inconclusive, never "unchanged".
        self.0.probe().map(|(_, ui)| ui).unwrap_or_default()
    }
}

struct HelperExec<'a> {
    helper: &'a HelperClient,
    action_id: String,
    content: BTreeMap<String, String>,
}
impl Executor for HelperExec<'_> {
    fn execute(&mut self, route: &Route) -> Result<(), String> {
        self.helper.act(&self.action_id, route, &self.content)
    }
    fn restore(&mut self, before: &UiSnapshot, fields: &[Field]) -> bool {
        fields.contains(&Field::FrontmostApp) && self.helper.restore(before)
    }
}

/// Where the driver's events go: appended to the run's log and persisted (the service writes each record).
pub trait Sink {
    fn push(&mut self, log: &mut EventLog, e: Event) -> Result<(), String>;
    /// Persist records the runner appended directly (from index `from`).
    fn flush(&mut self, log: &EventLog, from: usize) -> Result<(), String>;
}

/// An observer over the helper: snapshot `app`'s scope, or record why it couldn't.
pub fn observe(helper: &HelperClient, app: &str, scope: Option<&str>) -> Event {
    match helper.snapshot(app, scope) {
        Ok(nodes) => Event::ObsSnapshot {
            app: app.into(),
            scope: scope.map(str::to_string),
            snapshot: Some(AxSnapshot::from_wire(app, scope, &nodes)),
            err: None,
        },
        Err(code) => unobserved(app, scope, &code),
    }
}

pub struct Driver<'a> {
    pub helper: &'a HelperClient,
    pub scorer: &'a dyn Scorer,
    pub clock: &'a dyn Clock,
}

struct DynClock<'a>(&'a dyn Clock);
impl Clock for DynClock<'_> {
    fn mono_ns(&self) -> u64 {
        self.0.mono_ns()
    }
    fn wall_ms(&self) -> u64 {
        self.0.wall_ms()
    }
}

/// Most actions one run takes (a first try plus backtracks).
pub const MAX_ACTIONS: usize = 1 + crate::decide::BACKTRACKS_PER_TASK as usize;

impl Driver<'_> {
    /// Drive a run from its current state: finish the chain (observing AX scopes through the helper), execute the
    /// plan's step, verify, backtrack on failure, until the run is done, asks, is held, or out of budget.
    pub fn drive(&self, goal: &Goal, log: &mut EventLog, sink: &mut dyn Sink) -> Result<(), String> {
        let task_id = goal.task_id.to_string();
        let finish = |status| Event::TaskFinished {
            task_id: task_id.clone(),
            status,
        };
        // Each round is a chain continuation or one action; a bound so a bug can't loop forever.
        for _ in 0..3 * MAX_ACTIONS + 3 {
            let s = log.state();
            if s.ask.is_some() || s.task_status.is_some() || s.held.is_some() {
                return Ok(());
            }
            if s.actions.len() >= MAX_ACTIONS {
                return sink.push(log, finish(TaskStatus::AskedOut));
            }
            if s.plan.is_none() {
                let events = run_chain_obs(goal, &s, self.scorer, self.clock, &mut |app, scope| {
                    observe(self.helper, app, scope)
                });
                if events.is_empty() {
                    return Ok(());
                }
                for e in events {
                    sink.push(log, e)?;
                }
                continue;
            }
            let Some(step) = step_for(&s, goal) else {
                return Ok(());
            };
            let (routes, verify, content) = match step {
                Step::Hold(reason) => {
                    sink.push(log, Event::ExecHeld { reason })?;
                    if matches!(reason, HoldReason::NoRoute | HoldReason::UnknownApp) {
                        sink.push(log, finish(TaskStatus::AskedOut))?;
                    }
                    return Ok(());
                }
                Step::Act {
                    routes,
                    verify,
                    content,
                } => (routes, verify, content),
            };
            let action_id = format!("a{}", s.actions.len());
            let before_scope = match &verify.check {
                Check::AxScope { app, scope } => find_snapshot(&s.snapshots, app, scope.as_deref()).cloned(),
                _ => None,
            };
            let before = self.helper.measure(&verify.check);
            let env: Env = self.helper.probe().map(|(e, _)| e).unwrap_or_default();
            let from = log.records().len();
            let outcome = {
                let probe = HelperProbe(self.helper);
                let mut exec = HelperExec {
                    helper: self.helper,
                    action_id: action_id.clone(),
                    content,
                };
                let clock = DynClock(self.clock);
                Runner {
                    log,
                    probe: &probe,
                    exec: &mut exec,
                    clock: &clock,
                }
                .run(&action_id, &routes, &env)
            };
            sink.flush(log, from)?;
            let reason = match outcome {
                RunOutcome::Done { .. } => {
                    let (after, verdict) = match &verify.check {
                        Check::AxScope { app, scope } => {
                            let e = observe(self.helper, app, scope.as_deref());
                            let changed = match (&e, &before_scope) {
                                (
                                    Event::ObsSnapshot {
                                        snapshot: Some(new), ..
                                    },
                                    Some(old),
                                ) => {
                                    let d = diff(old, new);
                                    let changed = !d.is_empty();
                                    sink.push(
                                        log,
                                        Event::ObsDiff {
                                            action_id: action_id.clone(),
                                            app: app.clone(),
                                            scope: scope.clone(),
                                            diff: d,
                                        },
                                    )?;
                                    Some(changed)
                                }
                                _ => None,
                            };
                            sink.push(log, e)?;
                            (
                                changed.map(i64::from),
                                match changed {
                                    Some(true) => VerifyOutcome::Passed,
                                    Some(false) => VerifyOutcome::Failed,
                                    None => VerifyOutcome::Unverifiable,
                                },
                            )
                        }
                        c => {
                            let after = self.helper.measure(c);
                            (after, judge_verify(verify.expect, before, after))
                        }
                    };
                    sink.push(
                        log,
                        Event::VerifyResult {
                            action_id: action_id.clone(),
                            check: verify.check.clone(),
                            expect: verify.expect,
                            before,
                            after,
                            outcome: verdict,
                        },
                    )?;
                    if verdict != VerifyOutcome::Failed {
                        sink.push(log, finish(TaskStatus::Done))?;
                        return Ok(());
                    }
                    BacktrackReason::VerifyFailed
                }
                RunOutcome::Failed { .. } | RunOutcome::Refused => BacktrackReason::ActionFailed,
                // A grant only the user can give, in the foreground: the run asks and ends here.
                RunOutcome::Asked => {
                    sink.push(log, finish(TaskStatus::AskedOut))?;
                    return Ok(());
                }
                RunOutcome::Deferred { .. } => {
                    sink.push(log, finish(TaskStatus::Deferred))?;
                    return Ok(());
                }
            };
            match next_backtrack(&log.state(), reason) {
                Some(b) => sink.push(log, b)?,
                None => {
                    sink.push(log, finish(TaskStatus::AskedOut))?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}
