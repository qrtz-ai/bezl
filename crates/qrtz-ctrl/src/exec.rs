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
//! **Irreversible** plans (quit, empty trash, a system setting, send) go through the confirm gate first (slice 6,
//! ADR 0111, [`crate::gate`]): `confirm.requested` with a preview from the helper, then the user's answer (or a
//! standing approval of the task that covers it), then `confirm.reverified` right before the one action the
//! approval allows. **File routes** (`files.*`, slice 7, ADR 0112) expand into one batch of file moves on the
//! driver's [`FileRoots`] ([`crate::files`]), run by the [`FsExecutor`] between the same guard snapshots and
//! verified by re-reading the folder; trashing waits at the gate with a `file` preview. Without roots (and for
//! `files.empty_trash`) they still stop with `exec.held { files_not_yet }`. (Runs from before slice 6 may still hold `exec.held { irreversible }`; they replay as before.)
use crate::ax::{diff, AxSnapshot};
use crate::decide::{next_backtrack, run_chain_obs, unobserved, BacktrackReason, Goal, Node, Scorer};
use crate::files::{path_ref, plan_cleanup, FilePlan, FileRoots};
use crate::fs_exec::{FsExecutor, MemStore};
use crate::gate::{
    counted_items, hash_items, intent_and_move, preview_what, rules, Approval, ConfirmAnswer, ItemKind,
    Preview, PreviewItem,
};
use crate::guard::{Field, UiSnapshot};
use crate::helper::PreviewWhat;
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
    /// The plan can't be undone (slice 5 logs; slice 6 asks through the confirm gate instead).
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
    s.plan.as_ref()?;
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
        // Quit, posted to the app's pid (irreversible: only after the confirm gate). The app keeps its own save
        // sheet for unsaved work; nothing here answers it.
        "shortcut.cmd+q" => act(
            vec![key("cmd+q")],
            Check::AppRunning { app: app.clone() },
            Expect::Zero,
        ),
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
    /// Readable titles of a gate preview's item keys (kept in memory for the view, never logged).
    fn labels(&mut self, _labels: Vec<(String, String)>) {}
    /// A gate now waits for the user (raise a non-activating notification).
    fn confirm_waiting(&mut self, _gate_id: &str) {}
}

/// The confirm gate's preview of a step, from the helper: the windows or tabs it touches, or a count when the
/// helper can't enumerate them, or the action itself when there's nothing to enumerate.
pub fn preview_of(
    helper: &HelperClient,
    s: &CtrlState,
    routes: &[Route],
) -> (Preview, Vec<(String, String)>) {
    let (intent, mv) = intent_and_move(s);
    let app = routes.iter().find_map(|r| r.app().map(str::to_string));
    let (items, labels, complete) = match (preview_what(&intent), app.as_deref()) {
        (Some((what, kind)), Some(a)) => match helper.preview(a, what) {
            Ok(wire) => {
                let (items, labels) = hash_items(kind, &wire);
                (items, labels, true)
            }
            Err(_) => {
                let check = match what {
                    PreviewWhat::Windows => Check::WindowCount { app: a.into() },
                    PreviewWhat::Tabs => Check::TabCount { app: a.into() },
                };
                (
                    counted_items(kind, helper.measure(&check).unwrap_or(0)),
                    Vec::new(),
                    false,
                )
            }
        },
        _ => (
            vec![PreviewItem {
                kind: ItemKind::Action,
                key: "action".into(),
                unsaved: Some(false),
            }],
            Vec::new(),
            true,
        ),
    };
    (
        Preview {
            intent,
            mv,
            app,
            items,
            complete,
        },
        labels,
    )
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
    /// The task's standing approvals (slice 6): one that covers a gate's preview answers it without asking.
    pub standing: &'a [Approval],
    /// Where file routes act (slice 7). `None`: they hold `files_not_yet`.
    pub files: Option<&'a FileRoots>,
}

/// The `file` preview of a batch: the files it trashes, or the batch itself when it only moves.
pub fn files_preview(s: &CtrlState, plan: &FilePlan) -> (Preview, Vec<(String, String)>) {
    let (intent, mv) = intent_and_move(s);
    let mut labels = Vec::new();
    let items = plan
        .trashed()
        .into_iter()
        .map(|p| {
            let key = path_ref(p);
            labels.push((
                key.clone(),
                p.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            ));
            PreviewItem {
                kind: ItemKind::File,
                key,
                unsaved: Some(false),
            }
        })
        .collect();
    (
        Preview {
            intent,
            mv,
            app: None,
            items,
            complete: true,
        },
        labels,
    )
}

/// The `route.files.*` moves a batch can expand (emptying the Trash can't be undone: still held).
fn file_move(mv: &str) -> bool {
    matches!(
        mv,
        "route.files.tidy" | "route.files.sort_by_kind" | "route.files.trash_old"
    )
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
            // Slice 7: a file move on known roots becomes one batch, planned from the folder as it is now.
            let mv = intent_and_move(&s).1;
            let fplan: Option<(&FileRoots, FilePlan)> = match self.files.filter(|_| file_move(&mv)) {
                Some(roots) => {
                    let now = std::time::UNIX_EPOCH + std::time::Duration::from_millis(self.clock.wall_ms());
                    match plan_cleanup(roots, &mv, now) {
                        Ok(p) if p.files() == 0 => return sink.push(log, finish(TaskStatus::Done)),
                        Ok(p) => Some((roots, p)),
                        // An unreadable folder (no Files and Folders grant yet): ask, nothing moved.
                        Err(_) => {
                            sink.push(
                                log,
                                Event::ExecHeld {
                                    reason: HoldReason::NoRoute,
                                },
                            )?;
                            return sink.push(log, finish(TaskStatus::AskedOut));
                        }
                    }
                }
                None => None,
            };
            let step = match &fplan {
                Some((roots, p)) => Step::Act {
                    routes: vec![Route::Files(p.to_op(roots).0)],
                    verify: Verify {
                        check: Check::FilesRemaining,
                        expect: Expect::Zero,
                    },
                    content: BTreeMap::new(),
                },
                None => match step_for(&s, goal) {
                    Some(step) => step,
                    None => return Ok(()),
                },
            };
            let preview = |s: &CtrlState, routes: &[Route]| match &fplan {
                Some((_, p)) => files_preview(s, p),
                None => preview_of(self.helper, s, routes),
            };
            let measure = |c: &Check| match (c, &fplan) {
                (Check::FilesRemaining, Some((roots, p))) => Some(p.remaining(roots)),
                _ => self.helper.measure(c),
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
            // The confirm gate (ADR 0111): an irreversible plan acts only on an unspent approval of this plan whose
            // preview still holds right now.
            if s.plan.as_ref().is_some_and(|p| p.irreversible) {
                let gate = s.confirm.as_ref().filter(|g| !g.used);
                match gate.map(|g| (g, g.answer.as_ref())) {
                    None => {
                        let (preview, labels) = preview(&s, &routes);
                        sink.labels(labels);
                        let gate_id = format!("g{}", s.gates);
                        let scope = preview.scope();
                        sink.push(
                            log,
                            Event::ConfirmRequested {
                                gate_id: gate_id.clone(),
                                preview,
                                rules: rules(&s),
                            },
                        )?;
                        match self.standing.iter().find(|a| a.scope.covers(&scope)) {
                            Some(a) => sink.push(
                                log,
                                Event::ConfirmResolved {
                                    gate_id,
                                    answer: ConfirmAnswer::Standing {
                                        approval_id: a.id.clone(),
                                        scope: a.scope.clone(),
                                    },
                                },
                            )?,
                            None => {
                                sink.confirm_waiting(&gate_id);
                                return Ok(());
                            }
                        }
                        continue;
                    }
                    // Waiting for the user, or denied / expired (the service finishes the run).
                    Some((_, None)) => return Ok(()),
                    Some((_, Some(a))) if !a.approves() => return Ok(()),
                    Some((g, Some(_))) if g.reverified.is_none() => {
                        let (fresh, labels) = preview(&s, &routes);
                        sink.labels(labels);
                        let ok = fresh.within(&g.preview);
                        sink.push(
                            log,
                            Event::ConfirmReverified {
                                gate_id: g.id.clone(),
                                ok,
                            },
                        )?;
                        continue;
                    }
                    Some(_) => {}
                }
            }
            let action_id = format!("a{}", s.actions.len());
            let before_scope = match &verify.check {
                Check::AxScope { app, scope } => find_snapshot(&s.snapshots, app, scope.as_deref()).cloned(),
                _ => None,
            };
            let before = measure(&verify.check);
            let env: Env = self.helper.probe().map(|(e, _)| e).unwrap_or_default();
            let from = log.records().len();
            let outcome = {
                let probe = HelperProbe(self.helper);
                let clock = DynClock(self.clock);
                match &fplan {
                    Some((roots, p)) => {
                        let store: MemStore = p.to_op(roots).1;
                        let mut exec = FsExecutor {
                            store,
                            trash_dir: roots.trash.clone(),
                        };
                        Runner {
                            log,
                            probe: &probe,
                            exec: &mut exec,
                            clock: &clock,
                        }
                        .run(&action_id, &routes, &env)
                    }
                    None => {
                        let mut exec = HelperExec {
                            helper: self.helper,
                            action_id: action_id.clone(),
                            content,
                        };
                        Runner {
                            log,
                            probe: &probe,
                            exec: &mut exec,
                            clock: &clock,
                        }
                        .run(&action_id, &routes, &env)
                    }
                }
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
                            let after = measure(c);
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
