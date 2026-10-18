//! The execution ladder (ADR 0100). An action comes with every route that could perform it; the ladder takes the
//! lowest rung that is allowed right now. Lower rungs touch less of the user's session:
//!
//! 1. [`Rung::Files`]: FileManager / `std::fs`. No UI at all.
//! 2. [`Rung::Workspace`]: `NSWorkspace` open with `activates = false`, only for apps that are already running (a cold
//!    launch activates anyway, so it needs the user's OK: [`Grant::Foreground`]).
//! 3. [`Rung::Scripting`]: an app's scripting dictionary over Apple Events, from a fixed set of verbs that never
//!    `activate`, `launch`, `open` or `reopen`.
//! 4. [`Rung::AxAction`]: an AX action or attribute write on a background app's element (no raise, no menus, no
//!    focus or frontmost writes).
//! 5. [`Rung::PidKey`]: `CGEventPostToPid` with a private event source, to an app that is not frontmost, never a
//!    "focus here" shortcut, never while secure input is on.
//!
//! Routes that need the cursor, global events or activation ([`Route::Click`], [`Route::GlobalKey`],
//! [`Route::Activate`]) exist so a planner can name them, and the ladder always refuses them.
//!
//! Consent is never requested from a background run: a missing grant turns into [`Decision::Ask`] so the user can
//! grant it in the foreground (onboarding / settings), because every TCC prompt is a modal that takes focus.
use crate::policy::{
    is_focus_intent, normalize_combo, FORBIDDEN_AX_ACTIONS, FORBIDDEN_AX_SET_ATTRIBUTES, MENU_OPENING_ROLES,
    SECURE_ROLES, USER_ACTIVE_WINDOW_MS,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Opaque pointer into the content store (paths, URLs, labels never appear in the log: ADR 0083, spec §13.6).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentRef(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rung {
    Files = 1,
    Workspace = 2,
    Scripting = 3,
    AxAction = 4,
    PidKey = 5,
    /// Never allowed: cursor, global events, activation.
    TakesOver = 99,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FsOp {
    /// Move `src` into the directory `dst_dir` under the same name; never overwrites.
    MoveInto { src: ContentRef, dst_dir: ContentRef },
    /// Move to the user's Trash (reversible).
    Trash { src: ContentRef },
    /// Delete one top-level item of the user's Trash for good (ADR 0122, "empty the trash"): the only unlink ctrl
    /// has, irreversible, planned only by `route.files.empty_trash` behind an approved confirm gate.
    Delete { src: ContentRef },
    /// Create one directory (slice 7: a kind folder); undone by removing it while empty.
    MakeDir { dir: ContentRef },
    /// Ops run in order as one action (slice 7, ADR 0113); a failure rolls the done ones back.
    Batch { ops: Vec<FsOp> },
}

impl FsOp {
    /// This op (or one inside a batch) deletes for good (ADR 0122).
    pub fn deletes(&self) -> bool {
        match self {
            FsOp::Delete { .. } => true,
            FsOp::Batch { ops } => ops.iter().any(FsOp::deletes),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WorkspaceOp {
    /// Open a URL in an app without activating it.
    OpenUrl { app: String, url: ContentRef },
    /// Start an app hidden, without activating it.
    LaunchHidden { app: String },
    /// ADR 0130: `NSRunningApplication.terminate()` each app (no activation, no key event). Only behind an approved
    /// confirm gate; never the frontmost app.
    QuitApps { apps: Vec<String> },
    /// ADR 0134: `NSRunningApplication.hide()` each app (no activation, no key event, no window raised). Reversible
    /// (clicking the app in the Dock brings it back), so no gate; never the frontmost app.
    HideApps { apps: Vec<String> },
    /// ADR 0135: mute the Mac's sound output (Standard Additions `set volume with output muted`, in the helper's
    /// own process: no app is told anything, nothing is shown, no key event). Reversible, no gate.
    MuteOutput,
    /// ADR 0139: the reverse of [`WorkspaceOp::MuteOutput`] (`set volume without output muted`, same process, same
    /// rules).
    UnmuteOutput,
}

/// The only Apple Event verbs ctrl sends. Templates, not free text: the type can't express `activate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptVerb {
    CountTabs,
    CloseAllTabs,
    CloseTab,
    CountWindows,
    CloseWindow,
    /// Slice 7b (ADR 0114): close every tab that is not pinned and holds no unsaved form input; the helper leaves a
    /// tab alone whenever it can't tell (pinned / unsaved unknown counts as "keep").
    CloseSavedTabs,
    /// Close every tab that is not pinned, unsaved input included. Only after an approved confirm gate on the held
    /// tabs (`replay_check` refuses it otherwise).
    CloseUnpinnedTabs,
    /// ADR 0137: System Events `tell appearance preferences to set dark mode to true` (only to
    /// `com.apple.systemevents`, a faceless agent: nothing comes forward, no key event). Reversible.
    SetDarkMode,
    /// ADR 0137: the same with `false` (light mode).
    SetLightMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AxOp {
    Perform { action: String },
    SetAttribute { attribute: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "route", rename_all = "snake_case")]
pub enum Route {
    Files(FsOp),
    Workspace(WorkspaceOp),
    Scripting {
        app: String,
        verb: ScriptVerb,
    },
    Ax {
        app: String,
        role: String,
        element: ContentRef,
        op: AxOp,
    },
    PidKey {
        app: String,
        combo: String,
    },
    /// A coordinate click: moves (or warps) the cursor and may activate on hit.
    Click {
        app: String,
    },
    /// `CGEventPost` to the HID / session tap: goes wherever the user's focus is.
    GlobalKey {
        combo: String,
    },
    /// `NSRunningApplication.activate`, `open -a`, `tell app to activate`, `AXFrontmost = true`.
    Activate {
        app: String,
    },
}

impl Route {
    pub fn rung(&self) -> Rung {
        match self {
            Route::Files(_) => Rung::Files,
            Route::Workspace(_) => Rung::Workspace,
            Route::Scripting { .. } => Rung::Scripting,
            Route::Ax { .. } => Rung::AxAction,
            Route::PidKey { .. } => Rung::PidKey,
            Route::Click { .. } | Route::GlobalKey { .. } | Route::Activate { .. } => Rung::TakesOver,
        }
    }

    pub fn app(&self) -> Option<&str> {
        match self {
            Route::Files(_)
            | Route::GlobalKey { .. }
            | Route::Workspace(
                WorkspaceOp::QuitApps { .. }
                | WorkspaceOp::HideApps { .. }
                | WorkspaceOp::MuteOutput
                | WorkspaceOp::UnmuteOutput,
            ) => None,
            Route::Workspace(WorkspaceOp::OpenUrl { app, .. } | WorkspaceOp::LaunchHidden { app }) => {
                Some(app)
            }
            Route::Scripting { app, .. }
            | Route::Ax { app, .. }
            | Route::PidKey { app, .. }
            | Route::Click { app }
            | Route::Activate { app } => Some(app),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Consent {
    Granted,
    /// Never asked, or unknown: asking would show a modal, so a background run doesn't.
    NotAsked,
    Denied,
}

/// A TCC grant the user can give in the foreground.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "grant", rename_all = "snake_case")]
pub enum Grant {
    /// Files and Folders (e.g. Downloads) or Full Disk Access.
    Files,
    /// Apple Events to one app (`kTCCServiceAppleEvents`).
    Automation {
        app: String,
    },
    Accessibility,
    /// `kTCCServicePostEvent` (`CGPreflightPostEventAccess`).
    PostEvent,
    /// Not a TCC grant: the user's OK to bring `app` forward once. Starting an app that isn't running activates it
    /// even with `activates = false` (measured on macOS 26 CI, ADR 0100), so a cold launch is never silent.
    Foreground {
        app: String,
    },
}

/// What the session looks like right now, from the helper's probes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Env {
    /// Bundle id of the frontmost app.
    pub frontmost_app: Option<String>,
    /// Physical input idle time (HID state).
    pub user_idle_ms: Option<u64>,
    /// `IsSecureEventInputEnabled()`.
    pub secure_input: bool,
    pub files: Option<Consent>,
    pub accessibility: Option<Consent>,
    pub post_event: Option<Consent>,
    pub automation: BTreeMap<String, Consent>,
    /// (app, rung) pairs that already caused a guard violation in this run (derived from the log).
    pub quarantined: BTreeSet<(String, Rung)>,
    /// Bundle ids of running apps. Workspace routes to an app that isn't running would cold-launch it.
    pub running_apps: BTreeSet<String>,
}

impl Env {
    fn user_in(&self, app: &str) -> bool {
        self.frontmost_app.as_deref() == Some(app)
    }
    fn user_active(&self) -> bool {
        self.user_idle_ms.is_some_and(|ms| ms < USER_ACTIVE_WINDOW_MS)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Refusal {
    /// The route needs the cursor, global events or activation.
    TakesOverUi,
    ForbiddenAxAction {
        action: String,
    },
    ForbiddenAxAttribute {
        attribute: String,
    },
    /// `AXPress` on a menu-opening role or the menu bar.
    OpensMenu {
        role: String,
    },
    SecureField,
    /// A shortcut that means "focus here" / switches what the user sees.
    FocusIntentShortcut {
        combo: String,
    },
    /// Key events to the app the user is in would interleave with their typing.
    TargetIsFrontmost,
    /// This (app, rung) already violated the guard in this run.
    Quarantined,
    /// The user denied the grant; only they can change it in System Settings.
    Denied {
        grant: Grant,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Allowed,
    /// Allowed later: the user is active in the target app, or secure input is on.
    Defer {
        why: DeferReason,
    },
    /// Needs a grant the user gives in the foreground.
    NeedsGrant {
        grant: Grant,
    },
    Refused {
        why: Refusal,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeferReason {
    UserActiveInTarget,
    SecureInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Run `routes[index]`.
    Execute { index: usize, rung: Rung },
    /// Nothing allowed now, but something will be: retry after the delay.
    Defer { retry_after_ms: u64 },
    /// Ask the user for these grants (in the foreground) or to do it themselves.
    Ask { grants: Vec<Grant> },
    /// Nothing can run without taking over the UI.
    Refuse,
}

fn consent(c: Option<Consent>, grant: Grant) -> Option<Verdict> {
    match c.unwrap_or(Consent::NotAsked) {
        Consent::Granted => None,
        Consent::NotAsked => Some(Verdict::NeedsGrant { grant }),
        Consent::Denied => Some(Verdict::Refused {
            why: Refusal::Denied { grant },
        }),
    }
}

/// Judge one route against the policy and the session.
pub fn judge(route: &Route, env: &Env) -> Verdict {
    if route.rung() == Rung::TakesOver {
        return Verdict::Refused {
            why: Refusal::TakesOverUi,
        };
    }
    if let Some(app) = route.app() {
        if env.quarantined.contains(&(app.to_string(), route.rung())) {
            return Verdict::Refused {
                why: Refusal::Quarantined,
            };
        }
    }
    match route {
        Route::Files(_) => consent(env.files, Grant::Files).unwrap_or(Verdict::Allowed),
        Route::Workspace(op) => {
            let app = match op {
                WorkspaceOp::OpenUrl { app, .. } | WorkspaceOp::LaunchHidden { app } => app,
                WorkspaceOp::QuitApps { apps } | WorkspaceOp::HideApps { apps } => {
                    // The app the user is in is never quit or hidden under them.
                    if apps.iter().any(|a| env.user_in(a)) {
                        return Verdict::Refused {
                            why: Refusal::TargetIsFrontmost,
                        };
                    }
                    return Verdict::Allowed;
                }
                // No app, no window, no input: nothing on screen to take over.
                WorkspaceOp::MuteOutput | WorkspaceOp::UnmuteOutput => return Verdict::Allowed,
            };
            if !env.running_apps.contains(app) {
                return Verdict::NeedsGrant {
                    grant: Grant::Foreground { app: app.clone() },
                };
            }
            if env.user_in(app) && env.user_active() {
                return Verdict::Defer {
                    why: DeferReason::UserActiveInTarget,
                };
            }
            Verdict::Allowed
        }
        Route::Scripting { app, .. } => {
            if let Some(v) = consent(
                env.automation.get(app).copied(),
                Grant::Automation { app: app.clone() },
            ) {
                return v;
            }
            if env.user_in(app) && env.user_active() {
                return Verdict::Defer {
                    why: DeferReason::UserActiveInTarget,
                };
            }
            Verdict::Allowed
        }
        Route::Ax { app, role, op, .. } => {
            if SECURE_ROLES.contains(&role.as_str()) {
                return Verdict::Refused {
                    why: Refusal::SecureField,
                };
            }
            match op {
                AxOp::Perform { action } => {
                    if FORBIDDEN_AX_ACTIONS.contains(&action.as_str()) {
                        return Verdict::Refused {
                            why: Refusal::ForbiddenAxAction {
                                action: action.clone(),
                            },
                        };
                    }
                    if action == "AXPress" && MENU_OPENING_ROLES.contains(&role.as_str()) {
                        return Verdict::Refused {
                            why: Refusal::OpensMenu { role: role.clone() },
                        };
                    }
                }
                AxOp::SetAttribute { attribute } => {
                    if FORBIDDEN_AX_SET_ATTRIBUTES.contains(&attribute.as_str()) {
                        return Verdict::Refused {
                            why: Refusal::ForbiddenAxAttribute {
                                attribute: attribute.clone(),
                            },
                        };
                    }
                }
            }
            if let Some(v) = consent(env.accessibility, Grant::Accessibility) {
                return v;
            }
            if env.user_in(app) && env.user_active() {
                return Verdict::Defer {
                    why: DeferReason::UserActiveInTarget,
                };
            }
            Verdict::Allowed
        }
        Route::PidKey { app, combo } => {
            if is_focus_intent(combo) {
                return Verdict::Refused {
                    why: Refusal::FocusIntentShortcut {
                        combo: normalize_combo(combo),
                    },
                };
            }
            if env.user_in(app) {
                return Verdict::Refused {
                    why: Refusal::TargetIsFrontmost,
                };
            }
            if let Some(v) = consent(env.post_event, Grant::PostEvent) {
                return v;
            }
            if env.secure_input {
                return Verdict::Defer {
                    why: DeferReason::SecureInput,
                };
            }
            Verdict::Allowed
        }
        Route::Click { .. } | Route::GlobalKey { .. } | Route::Activate { .. } => Verdict::Refused {
            why: Refusal::TakesOverUi,
        },
    }
}

/// Retry delay for a deferred action.
pub const DEFER_MS: u64 = 5000;

/// Choose a route: the lowest allowed rung wins (ties keep the planner's order). If none is allowed: defer if any
/// route only waits on the user being busy, else ask for the missing grants, else refuse.
pub fn choose(routes: &[Route], env: &Env) -> (Decision, Vec<Verdict>) {
    let verdicts: Vec<Verdict> = routes.iter().map(|r| judge(r, env)).collect();
    let mut order: Vec<usize> = (0..routes.len()).collect();
    order.sort_by_key(|&i| (routes[i].rung(), i));
    if let Some(&i) = order.iter().find(|&&i| verdicts[i] == Verdict::Allowed) {
        return (
            Decision::Execute {
                index: i,
                rung: routes[i].rung(),
            },
            verdicts,
        );
    }
    if verdicts.iter().any(|v| matches!(v, Verdict::Defer { .. })) {
        return (
            Decision::Defer {
                retry_after_ms: DEFER_MS,
            },
            verdicts,
        );
    }
    let mut grants: Vec<Grant> = verdicts
        .iter()
        .filter_map(|v| match v {
            Verdict::NeedsGrant { grant } => Some(grant.clone()),
            _ => None,
        })
        .collect();
    grants.sort();
    grants.dedup();
    if !grants.is_empty() {
        return (Decision::Ask { grants }, verdicts);
    }
    (Decision::Refuse, verdicts)
}
