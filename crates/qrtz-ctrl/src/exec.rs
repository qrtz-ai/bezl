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
//! approval allows. **File routes** (`files.*`, slice 7, ADR 0113) expand into one batch of file moves on the
//! driver's [`FileRoots`] ([`crate::files`]), run by the [`FsExecutor`] between the same guard snapshots and
//! verified by re-reading the folder; trashing waits at the gate with a `file` preview. `files.empty_trash`
//! (ADR 0122) deletes the Trash's items through the same executor, never through the Finder, only after the gate,
//! and only the items the approved card listed. Without roots they still stop with `exec.held { files_not_yet }`. (Runs from before slice 6 may still hold `exec.held { irreversible }`; they replay as before.)
use crate::apps::{
    hide_one, level_of, show_one, step_of, stepped, still_hidden, still_visible, to_hide, unused_today,
    DEFAULT_VOLUME_STEP, HIDE_APP_MOVE, HIDE_OTHERS_MOVE, MUTE_MOVE, QUIT_APP_MOVE, QUIT_UNUSED_MOVE,
    SET_VOLUME_MOVE, SHOW_APP_MOVE, UNMUTE_MOVE, VOLUME_DOWN_MOVE, VOLUME_UP_MOVE,
};
use crate::ax::{diff, AxSnapshot};
use crate::decide::{next_backtrack, run_chain_obs, unobserved, BacktrackReason, Goal, Node, Scorer};
use crate::files::{
    path_ref, plan_cleanup, FilePlan, FileRoots, Planned, DESKTOP_SCREENSHOTS_MOVE, EMPTY_TRASH_MOVE,
    TRASH_SCREENSHOTS_MOVE,
};
use crate::fs_exec::{FsExecutor, MemStore};
use crate::gate::{
    counted_items, hash_items, intent_and_move, preview_what, rules, Approval, ConfirmAnswer, ItemKind,
    Preview, PreviewItem,
};
use crate::guard::{Field, UiSnapshot};
use crate::helper::PreviewWhat;
use crate::helper::{Check, Expect, HelperClient};
use crate::ladder::{AxOp, ContentRef, Env, FocusMode, Route, ScriptVerb, UserFolder, WorkspaceOp};
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
    /// ADR 0125: a folder ctrl has no routes for yet (Documents). The run stops and says so; nothing is read or
    /// moved, and no question is left open that no answer could settle.
    FolderNotYet,
    /// ADR 0158: the "qrtz Set Focus" shortcut isn't there; the view says "set up the shortcut". No fallback.
    FocusNoShortcut,
    /// ADR 0158: Automation for Shortcuts Events is denied; the view says "allow automation". No fallback.
    AutomationDenied,
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

/// ADR 0155: the only move offered for "turn on do not disturb" / "turn off focus". It plans nothing: the executor
/// holds the run with `no-route` (asked out) and sends nothing to the helper.
pub const FOCUS_HOLD_MOVE: &str = "route.system.focus_hold";
/// ADR 0158: `route.system.focus.<mode>` (a [`FocusMode`] spelling) runs the focus shortcut.
pub const FOCUS_MOVE_PREFIX: &str = "route.system.focus.";

/// ADR 0166: "open my downloads" shows one of the user's folders in a Finder window.
pub const OPEN_FOLDER_MOVE: &str = "route.workspace.open_folder";

/// ADR 0166: the folder a plan chose (`folder.<name>` of the closed [`UserFolder`] set), else `None`. The task's own
/// words never get here: only an option id the layers made.
pub fn chosen_user_folder(s: &CtrlState) -> Option<UserFolder> {
    s.chosen()
        .get(&Node::ChooseTarget)
        .and_then(|o| o.strip_prefix("folder."))
        .and_then(UserFolder::parse)
}

/// The step a plan's move maps to. Pure.
pub fn step_for(s: &CtrlState, goal: &Goal) -> Option<Step> {
    s.plan.as_ref()?;
    let apps = s.apps.clone().unwrap_or_default();
    let chosen = s.chosen();
    let mv = chosen.get(&Node::ChooseMove)?.as_str();
    // ADR 0155: Focus / Do Not Disturb has no background route yet; asked out before anything is read or acted.
    if mv == FOCUS_HOLD_MOVE {
        return Some(Step::Hold(HoldReason::NoRoute));
    }
    // ADR 0158: the user's "qrtz Set Focus" shortcut through Shortcuts Events. The mode is parsed strictly from the
    // move id (an option the layers made, never the task's text); anything else holds before the helper is asked.
    if let Some(m) = mv.strip_prefix(FOCUS_MOVE_PREFIX) {
        let Some(mode) = FocusMode::parse(m) else {
            return Some(Step::Hold(HoldReason::NoRoute));
        };
        return Some(Step::Act {
            routes: vec![Route::Workspace(WorkspaceOp::SetFocus { mode })],
            verify: Verify {
                check: Check::FocusOn,
                expect: if mode == FocusMode::Off {
                    Expect::Zero
                } else {
                    Expect::One
                },
            },
            content: BTreeMap::new(),
        });
    }
    // ADR 0166: the folder from the closed set (a plan without one, or with screenshots, holds before the helper is
    // asked); the helper resolves it to the system's own location and opens it in the running Finder.
    if mv == OPEN_FOLDER_MOVE {
        let Some(folder) = chosen_user_folder(s) else {
            return Some(Step::Hold(HoldReason::NoRoute));
        };
        return Some(Step::Act {
            routes: vec![Route::Workspace(WorkspaceOp::OpenFolder { folder })],
            verify: Verify {
                check: Check::FolderOpen { folder },
                expect: Expect::One,
            },
            content: BTreeMap::new(),
        });
    }
    if mv == crate::files::DOCUMENTS_HOLD_MOVE {
        return Some(Step::Hold(HoldReason::FolderNotYet));
    }
    if mv.starts_with("route.files.") {
        return Some(Step::Hold(HoldReason::FilesNotYet));
    }
    // ADR 0130 / 0134: the driver plans these from the helper's `ctrl.apps`; without that list there is nothing to
    // quit or hide.
    // ADR 0143: a relative volume step is planned from the level the driver reads just before; not pure.
    // ADR 0145: so is a toggle (it's planned from the appearance read just before).
    // ADR 0149: "hide <app>" too (the driver reads whether the app is frontmost, hidden or running just before).
    // ADR 0152: and "show <app>", its pair.
    if mv == QUIT_UNUSED_MOVE
        || mv == HIDE_OTHERS_MOVE
        || mv == HIDE_APP_MOVE
        || mv == SHOW_APP_MOVE
        || mv == VOLUME_UP_MOVE
        || mv == VOLUME_DOWN_MOVE
        || mv == DARK_MODE_TOGGLE_MOVE
    {
        return Some(Step::Hold(HoldReason::NoRoute));
    }
    // ADR 0135: the system output, no app; verified by reading the mute back.
    if mv == MUTE_MOVE {
        return Some(Step::Act {
            routes: vec![Route::Workspace(WorkspaceOp::MuteOutput)],
            verify: Verify {
                check: Check::OutputMuted,
                expect: Expect::One,
            },
            content: BTreeMap::new(),
        });
    }
    // ADR 0137: the appearance, through System Events (a faceless agent) only; verified by reading the global
    // appearance back in the helper.
    if mv == DARK_MODE_ON_MOVE || mv == DARK_MODE_OFF_MOVE {
        return Some(appearance_step(mv == DARK_MODE_ON_MOVE));
    }
    // ADR 0140: the level from the plan's `level.N`, set and unmuted in the helper's own process; verified by reading
    // the volume back (exactly N, give or take 1). No level, nothing to set: the layers asked, so this can't happen
    // on a plan they made; an older or hand-made plan holds.
    if mv == SET_VOLUME_MOVE {
        let Some(level) = chosen.get(&Node::ChooseTarget).and_then(|o| level_of(o)) else {
            return Some(Step::Hold(HoldReason::NoRoute));
        };
        return Some(Step::Act {
            routes: vec![Route::Workspace(WorkspaceOp::SetVolume { level })],
            verify: Verify {
                check: Check::OutputVolume,
                expect: Expect::Level(i64::from(level)),
            },
            content: BTreeMap::new(),
        });
    }
    // ADR 0139: the reverse, verified the same way (expect 0).
    if mv == UNMUTE_MOVE {
        return Some(Step::Act {
            routes: vec![Route::Workspace(WorkspaceOp::UnmuteOutput)],
            verify: Verify {
                check: Check::OutputMuted,
                expect: Expect::Zero,
            },
            content: BTreeMap::new(),
        });
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
        // Slice 7b (ADR 0114): close the tabs that are neither pinned nor hold unsaved input; the driver then holds
        // the unsaved ones behind the confirm gate ([`HELD_TABS_MOVE`]).
        "route.scripting.close_all_tabs" => act(
            vec![script(ScriptVerb::CloseSavedTabs)],
            Check::SavedTabs { app: app.clone() },
            Expect::Zero,
        ),
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
        // ADR 0148: quit one named app through the workspace (`NSRunningApplication.terminate()`, like the Dock's
        // Quit): no activation, no key event, only after the confirm gate. The ladder refuses it for the app the user
        // is in. The app keeps its own save sheet for unsaved work; nothing here answers it.
        "route.workspace.quit_app" => act(
            vec![Route::Workspace(WorkspaceOp::QuitApps {
                apps: vec![app.clone()],
            })],
            Check::AppRunning { app: app.clone() },
            Expect::Zero,
        ),
        // Quit, posted to the app's pid (an older plan's move; ADR 0148 plans the workspace route instead).
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
        (Expect::Level(n), _, Some(a)) => (a - n).abs() <= 1,
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
    // ADR 0148: a one-app workspace quit previews that app's windows, like the key route it replaced.
    let app = routes.iter().find_map(|r| match r {
        Route::Workspace(WorkspaceOp::QuitApps { apps }) if apps.len() == 1 => Some(apps[0].clone()),
        r => r.app().map(str::to_string),
    });
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

/// The `route.files.*` moves a batch can expand (emptying the Trash since ADR 0122: gated, no undo).
fn file_move(mv: &str) -> bool {
    matches!(
        mv,
        "route.files.tidy"
            | "route.files.sort_by_kind"
            | "route.files.trash_old"
            | EMPTY_TRASH_MOVE
            | DESKTOP_SCREENSHOTS_MOVE
            | TRASH_SCREENSHOTS_MOVE
    )
}

/// ADR 0122: once the gate is approved and re-verified, a delete plan keeps only the items the card listed. Whatever
/// landed in the Trash after the re-check waits for the next run instead of being deleted unasked.
fn only_approved(s: &CtrlState, mut plan: FilePlan) -> FilePlan {
    if !plan.deletes() {
        return plan;
    }
    let Some(g) = s
        .confirm
        .as_ref()
        .filter(|g| !g.used && g.reverified == Some(true) && g.answer.as_ref().is_some_and(|a| a.approves()))
    else {
        return plan;
    };
    plan.ops.retain(|o| match o {
        Planned::Delete { src } => g.preview.items.iter().any(|i| i.key == path_ref(src)),
        _ => false,
    });
    plan
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

/// The gate's move for the tabs "close all my tabs" held back because they hold unsaved input (or the helper
/// couldn't tell): approving it closes every unpinned tab ([`ScriptVerb::CloseUnpinnedTabs`]).
pub const HELD_TABS_MOVE: &str = "route.scripting.close_unsaved_tabs";
/// ADR 0137: the only app ctrl scripts for the appearance (faceless: it never comes forward).
pub const SYSTEM_EVENTS: &str = "com.apple.systemevents";
/// ADR 0137: "turn on dark mode" / "turn off dark mode" (reversible, no gate).
/// ADR 0137: one System Events script sets the appearance; the helper's read-back verifies it.
fn appearance_step(on: bool) -> Step {
    Step::Act {
        routes: vec![Route::Scripting {
            app: SYSTEM_EVENTS.into(),
            verb: if on {
                ScriptVerb::SetDarkMode
            } else {
                ScriptVerb::SetLightMode
            },
        }],
        verify: Verify {
            check: Check::DarkMode,
            expect: if on { Expect::One } else { Expect::Zero },
        },
        content: BTreeMap::new(),
    }
}

pub const DARK_MODE_ON_MOVE: &str = "route.scripting.dark_mode_on";
pub const DARK_MODE_OFF_MOVE: &str = "route.scripting.dark_mode_off";
/// ADR 0145: "toggle dark mode" reads the appearance just before and takes the other one.
pub const DARK_MODE_TOGGLE_MOVE: &str = "route.scripting.dark_mode_toggle";

pub const CLOSE_ALL_TABS_MOVE: &str = "route.scripting.close_all_tabs";

/// "Close all my tabs" (slice 7b): the browser's tabs, split into the ones closed without asking (not pinned, no
/// unsaved input) and the ones held for the gate (not pinned, unsaved input or unknown). Pinned tabs are never
/// touched. A helper that can't enumerate gives a count, all held.
pub fn tabs_split(
    helper: &HelperClient,
    app: &str,
) -> (usize, Vec<PreviewItem>, Vec<(String, String)>, bool) {
    match helper.preview(app, PreviewWhat::Tabs) {
        Ok(wire) => {
            let (items, labels) = hash_items(ItemKind::Tab, &wire);
            let mut safe = 0;
            let mut held = Vec::new();
            for (it, w) in items.into_iter().zip(wire.iter()) {
                match (w.pinned, w.unsaved) {
                    (Some(true), _) => {}
                    (Some(false), Some(false)) => safe += 1,
                    // Unsaved, or the helper can't tell whether it's pinned: ask.
                    _ => held.push(it),
                }
            }
            let keep: std::collections::BTreeSet<&String> = held.iter().map(|i| &i.key).collect();
            let labels = labels.into_iter().filter(|(k, _)| keep.contains(k)).collect();
            (safe, held, labels, true)
        }
        Err(_) => {
            let n = helper.measure(&Check::TabCount { app: app.into() }).unwrap_or(0);
            (0, counted_items(ItemKind::Tab, n), Vec::new(), false)
        }
    }
}

fn held_tabs_preview(s: &CtrlState, app: &str, items: Vec<PreviewItem>, complete: bool) -> Preview {
    Preview {
        intent: intent_and_move(s).0,
        mv: HELD_TABS_MOVE.into(),
        app: Some(app.into()),
        items,
        complete,
    }
}

/// ADR 0130: the apps "quit unused apps" would quit now, from the helper's last-used report.
#[derive(Debug, Clone)]
struct QuitPlan {
    /// (bundle id, name, unsaved)
    apps: Vec<(String, String, Option<bool>)>,
}

impl QuitPlan {
    fn ids(&self) -> Vec<String> {
        self.apps.iter().map(|(id, ..)| id.clone()).collect()
    }
}

fn quit_plan(helper: &HelperClient) -> Result<QuitPlan, String> {
    let r = helper.apps()?;
    Ok(QuitPlan {
        apps: unused_today(&r.apps, r.since_ms, r.day_start_ms)
            .into_iter()
            .map(|a| (a.bundle_id.clone(), a.name.clone(), a.unsaved))
            .collect(),
    })
}

/// The gate's `app` preview: one item per app, keyed by its hashed name (the name stays in memory for the card).
fn quit_preview(s: &CtrlState, q: &QuitPlan) -> (Preview, Vec<(String, String)>) {
    let (intent, mv) = intent_and_move(s);
    let wire: Vec<crate::helper::PreviewItemWire> = q
        .apps
        .iter()
        .map(|(_, name, unsaved)| crate::helper::PreviewItemWire {
            title: name.clone(),
            unsaved: *unsaved,
            pinned: None,
        })
        .collect();
    let (items, labels) = hash_items(ItemKind::App, &wire);
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
                        Ok(p) => match only_approved(&s, p) {
                            // Everything the card listed is gone already (the user emptied it meanwhile).
                            p if p.files() == 0 => return sink.push(log, finish(TaskStatus::Done)),
                            p => Some((roots, p)),
                        },
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
            // ADR 0148: "quit <app>" never quits the app the user is in. Checked before the gate (no card for a quit
            // that can't happen) and before the ladder's own refusal, which would backtrack into another intent.
            if fplan.is_none() && mv == QUIT_APP_MOVE {
                let target = s
                    .chosen()
                    .get(&Node::ChooseApp)
                    .and_then(|o| o.strip_prefix("app."))
                    .map(str::to_string);
                // The same frontmost read the ladder refuses on (the helper's probe).
                let front = self.helper.probe().ok().and_then(|(env, _)| env.frontmost_app);
                if target.is_some() && front == target {
                    sink.push(
                        log,
                        Event::ExecHeld {
                            reason: HoldReason::NoRoute,
                        },
                    )?;
                    return sink.push(log, finish(TaskStatus::AskedOut));
                }
            }
            // ADR 0166: a folder a Finder window shows already is done with nothing acted (opening it again would
            // only bring that window up). A helper that can't tell (`null`) gets the open; the verify decides.
            if fplan.is_none() && mv == OPEN_FOLDER_MOVE {
                if let Some(folder) = chosen_user_folder(&s) {
                    if self.helper.measure(&Check::FolderOpen { folder }) == Some(1) {
                        return sink.push(log, finish(TaskStatus::Done));
                    }
                }
            }
            // ADR 0130: "quit unused apps" is planned from the helper's last-used report each round, so the gate's
            // re-check and the act both see the apps as they are now.
            let qplan: Option<QuitPlan> = if fplan.is_none() && mv == QUIT_UNUSED_MOVE {
                match quit_plan(self.helper) {
                    Ok(q) if q.apps.is_empty() => return sink.push(log, finish(TaskStatus::Done)),
                    Ok(q) => Some(q),
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
            } else {
                None
            };
            // ADR 0134: "hide other apps" is planned from `ctrl.apps` too: every visible app but the frontmost and
            // qrtz. Reversible, so no gate; already all hidden means done.
            // ADR 0149: "hide <app>" is the same act for the one named app, read from `ctrl.apps` just before: the app
            // the user is in is held (hiding it would bring another app forward), not running or hidden already is
            // done with nothing acted.
            let one = (mv == HIDE_APP_MOVE || mv == SHOW_APP_MOVE)
                .then(|| {
                    s.chosen()
                        .get(&Node::ChooseApp)
                        .and_then(|o| o.strip_prefix("app."))
                        .map(str::to_string)
                })
                .flatten();
            let hplan: Option<Vec<String>> = if fplan.is_none()
                && (mv == HIDE_OTHERS_MOVE || mv == HIDE_APP_MOVE)
            {
                match self.helper.apps() {
                    Ok(r) if mv == HIDE_APP_MOVE => match one.as_deref().map(|id| hide_one(&r.apps, id)) {
                        Some(Ok(Some(id))) => Some(vec![id]),
                        Some(Ok(None)) => return sink.push(log, finish(TaskStatus::Done)),
                        _ => {
                            sink.push(
                                log,
                                Event::ExecHeld {
                                    reason: HoldReason::NoRoute,
                                },
                            )?;
                            return sink.push(log, finish(TaskStatus::AskedOut));
                        }
                    },
                    Ok(r) => match to_hide(&r.apps) {
                        v if v.is_empty() => return sink.push(log, finish(TaskStatus::Done)),
                        v => Some(v.into_iter().map(|a| a.bundle_id.clone()).collect()),
                    },
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
            } else {
                None
            };
            // ADR 0152: "show <app>" unhides the one named app, read from `ctrl.apps` just before: not running is held
            // (showing it would mean starting it; that's "open <app>"), on screen already or the app the user is in is
            // done with nothing acted. Nothing is activated, so the app the user is in stays frontmost.
            let uplan: Option<Vec<String>> = if fplan.is_none() && mv == SHOW_APP_MOVE {
                match (self.helper.apps(), one.as_deref()) {
                    (Ok(r), Some(id)) => match show_one(&r.apps, id) {
                        Ok(Some(id)) => Some(vec![id]),
                        Ok(None) => return sink.push(log, finish(TaskStatus::Done)),
                        Err(()) => {
                            sink.push(
                                log,
                                Event::ExecHeld {
                                    reason: HoldReason::NoRoute,
                                },
                            )?;
                            return sink.push(log, finish(TaskStatus::AskedOut));
                        }
                    },
                    _ => {
                        sink.push(
                            log,
                            Event::ExecHeld {
                                reason: HoldReason::NoRoute,
                            },
                        )?;
                        return sink.push(log, finish(TaskStatus::AskedOut));
                    }
                }
            } else {
                None
            };
            // ADR 0143: "turn it up / down" is read-modify-write: read the level now, step from it (the plan's
            // `step.N`, else the default), set that level, and verify the read-back against it. Muted or no output
            // reads no level, so it asks out with nothing changed; already at the end of the range is done.
            let vplan: Option<(i64, u8)> =
                if fplan.is_none() && (mv == VOLUME_UP_MOVE || mv == VOLUME_DOWN_MOVE) {
                    let step = s
                        .chosen()
                        .get(&Node::ChooseTarget)
                        .and_then(|o| step_of(o))
                        .unwrap_or(DEFAULT_VOLUME_STEP);
                    match self.helper.measure(&Check::OutputVolume) {
                        Some(b) => match stepped(b, step, mv == VOLUME_UP_MOVE) {
                            t if i64::from(t) == b => return sink.push(log, finish(TaskStatus::Done)),
                            t => Some((b, t)),
                        },
                        None => {
                            sink.push(
                                log,
                                Event::ExecHeld {
                                    reason: HoldReason::NoRoute,
                                },
                            )?;
                            return sink.push(log, finish(TaskStatus::AskedOut));
                        }
                    }
                } else {
                    None
                };
            // ADR 0145: "toggle dark mode" reads the appearance now and sets the other one, verified like on/off
            // (ADR 0137). No reading (no helper answer) asks out with nothing changed.
            let dplan: Option<(i64, bool)> = if fplan.is_none() && mv == DARK_MODE_TOGGLE_MOVE {
                match self.helper.measure(&Check::DarkMode) {
                    Some(b) => Some((b, b == 0)),
                    None => {
                        sink.push(
                            log,
                            Event::ExecHeld {
                                reason: HoldReason::NoRoute,
                            },
                        )?;
                        return sink.push(log, finish(TaskStatus::AskedOut));
                    }
                }
            } else {
                None
            };
            let step = match (&fplan, &qplan) {
                (None, None) if dplan.is_some() => appearance_step(dplan.is_some_and(|(_, on)| on)),
                (None, None) if vplan.is_some() => {
                    let level = vplan.map(|(_, t)| t).unwrap_or_default();
                    Step::Act {
                        routes: vec![Route::Workspace(WorkspaceOp::SetVolume { level })],
                        verify: Verify {
                            check: Check::OutputVolume,
                            expect: Expect::Level(i64::from(level)),
                        },
                        content: BTreeMap::new(),
                    }
                }
                (None, None) if uplan.is_some() => {
                    let apps = uplan.clone().unwrap_or_default();
                    Step::Act {
                        routes: vec![Route::Workspace(WorkspaceOp::UnhideApps { apps: apps.clone() })],
                        verify: Verify {
                            check: Check::AppsHidden { apps },
                            expect: Expect::Zero,
                        },
                        content: BTreeMap::new(),
                    }
                }
                (None, None) if hplan.is_some() => {
                    let apps = hplan.clone().unwrap_or_default();
                    Step::Act {
                        routes: vec![Route::Workspace(WorkspaceOp::HideApps { apps: apps.clone() })],
                        verify: Verify {
                            check: Check::AppsVisible { apps },
                            expect: Expect::Zero,
                        },
                        content: BTreeMap::new(),
                    }
                }
                (None, Some(q)) => Step::Act {
                    routes: vec![Route::Workspace(WorkspaceOp::QuitApps { apps: q.ids() })],
                    verify: Verify {
                        check: Check::AppsRemaining { apps: q.ids() },
                        expect: Expect::Zero,
                    },
                    content: BTreeMap::new(),
                },
                (Some((roots, p)), _) => Step::Act {
                    routes: vec![Route::Files(p.to_op(roots).0)],
                    verify: Verify {
                        check: Check::FilesRemaining,
                        expect: Expect::Zero,
                    },
                    content: BTreeMap::new(),
                },
                (None, None) => match step_for(&s, goal) {
                    Some(step) => step,
                    None => return Ok(()),
                },
            };
            let preview = |s: &CtrlState, routes: &[Route]| match (&fplan, &qplan) {
                (Some((_, p)), _) => files_preview(s, p),
                (None, Some(q)) => quit_preview(s, q),
                (None, None) => preview_of(self.helper, s, routes),
            };
            let measure = |c: &Check| match (c, &fplan) {
                (Check::FilesRemaining, Some((roots, p))) => Some(p.remaining(roots)),
                (Check::AppsRemaining { apps }, _) => self
                    .helper
                    .apps()
                    .ok()
                    .map(|r| r.apps.iter().filter(|a| apps.contains(&a.bundle_id)).count() as i64),
                (Check::AppsVisible { apps }, _) => {
                    self.helper.apps().ok().and_then(|r| still_visible(&r.apps, apps))
                }
                (Check::AppsHidden { apps }, _) => {
                    self.helper.apps().ok().and_then(|r| still_hidden(&r.apps, apps))
                }
                _ => self.helper.measure(c),
            };
            let (routes, verify, content) = match step {
                Step::Hold(reason) => {
                    sink.push(log, Event::ExecHeld { reason })?;
                    if matches!(
                        reason,
                        HoldReason::NoRoute
                            | HoldReason::UnknownApp
                            | HoldReason::FolderNotYet
                            | HoldReason::FocusNoShortcut
                            | HoldReason::AutomationDenied
                    ) {
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
            // Slice 7b (ADR 0114): "close all my tabs" closes the plain tabs unattended, never touches pinned ones,
            // and holds tabs with unsaved input behind the gate; approving closes them, deny leaves them open.
            let tabs_app = (fplan.is_none() && mv == CLOSE_ALL_TABS_MOVE)
                .then(|| routes.iter().find_map(|r| r.app().map(str::to_string)))
                .flatten();
            let mut tabs_more = false;
            let (routes, verify) = match &tabs_app {
                None => (routes, verify),
                Some(app) => {
                    let held_gate = s
                        .confirm
                        .as_ref()
                        .filter(|g| g.preview.mv == HELD_TABS_MOVE && !g.used);
                    match held_gate.map(|g| (g, g.answer.as_ref())) {
                        // Waiting, or denied / expired (the service finishes the run).
                        Some((_, None)) => return Ok(()),
                        Some((_, Some(a))) if !a.approves() => return Ok(()),
                        Some((g, Some(_))) if g.reverified.is_none() => {
                            let (_, held, labels, complete) = tabs_split(self.helper, app);
                            sink.labels(labels);
                            let ok = held_tabs_preview(&s, app, held, complete).within(&g.preview);
                            sink.push(
                                log,
                                Event::ConfirmReverified {
                                    gate_id: g.id.clone(),
                                    ok,
                                },
                            )?;
                            continue;
                        }
                        Some(_) => (
                            vec![Route::Scripting {
                                app: app.clone(),
                                verb: ScriptVerb::CloseUnpinnedTabs,
                            }],
                            Verify {
                                check: Check::UnpinnedTabs { app: app.clone() },
                                expect: Expect::Zero,
                            },
                        ),
                        None => {
                            let (safe, held, labels, complete) = tabs_split(self.helper, app);
                            // The unattended close runs once per run (a tab opened after it isn't swept up).
                            let swept = log.records().iter().any(|r| {
                                matches!(&r.event, Event::ActionRequested { routes, .. } if routes.iter().any(|r| matches!(r, Route::Scripting { verb: ScriptVerb::CloseSavedTabs, .. })))
                            });
                            if safe > 0 && !swept {
                                tabs_more = !held.is_empty();
                                (routes, verify)
                            } else if held.is_empty() {
                                // Only pinned tabs (or nothing) left: done, nothing to close.
                                return sink.push(log, finish(TaskStatus::Done));
                            } else {
                                let preview = held_tabs_preview(&s, app, held, complete);
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
                        }
                    }
                }
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
            // ADR 0143: the level the step was taken from is the "before" (one read, logged with the verify).
            // ADR 0145: likewise the appearance a toggle was planned from.
            let before = match (vplan, dplan) {
                (Some((b, _)), _) | (None, Some((b, _))) => Some(b),
                (None, None) => measure(&verify.check),
            };
            let env: Env = self.helper.probe().map(|(e, _)| e).unwrap_or_default();
            let from = log.records().len();
            let outcome = {
                let probe = HelperProbe(self.helper);
                let clock = DynClock(self.clock);
                match &fplan {
                    Some((roots, p)) => {
                        // Names stay in memory: they label what can't be put back later (ADR 0117).
                        sink.labels(
                            p.ops
                                .iter()
                                .filter_map(|o| match o {
                                    Planned::Move { src, .. }
                                    | Planned::Trash { src }
                                    | Planned::Delete { src } => Some(src),
                                    Planned::MakeDir { .. } => None,
                                })
                                .map(|src| {
                                    (
                                        path_ref(src),
                                        src.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                                    )
                                })
                                .collect(),
                        );
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
                        if tabs_more {
                            // The held tabs' gate comes next round.
                            continue;
                        }
                        sink.push(log, finish(TaskStatus::Done))?;
                        return Ok(());
                    }
                    BacktrackReason::VerifyFailed
                }
                // ADR 0158: a missing focus shortcut or denied Automation stops the run with what to do; nothing
                // else is tried (no CLI, no osascript, no other route).
                RunOutcome::Failed { ref err, .. }
                    if routes
                        .iter()
                        .any(|r| matches!(r, Route::Workspace(WorkspaceOp::SetFocus { .. })))
                        && (err == "focus.no_shortcut" || err == "automation.denied") =>
                {
                    let reason = if err == "focus.no_shortcut" {
                        HoldReason::FocusNoShortcut
                    } else {
                        HoldReason::AutomationDenied
                    };
                    sink.push(log, Event::ExecHeld { reason })?;
                    sink.push(log, finish(TaskStatus::AskedOut))?;
                    return Ok(());
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
            // Key routes (cmd-w, …) can't skip pinned or unsaved tabs: a selective close that failed asks. A delete
            // that failed (ADR 0122) asks too: the other way to empty the Trash is the Finder, which would take focus.
            if tabs_app.is_some()
                || qplan.is_some()
                || hplan.is_some()
                || fplan.as_ref().is_some_and(|(_, p)| p.deletes())
            {
                sink.push(log, finish(TaskStatus::AskedOut))?;
                return Ok(());
            }
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
