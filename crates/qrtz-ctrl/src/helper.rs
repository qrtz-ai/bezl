//! The ctrl helper protocol (slice 5, ADR 0108): how the host asks the Swift helper's `QrtzCtrl` module to observe
//! and act. Newline-delimited JSON, one request → one reply with the same `id`, all under the `ctrl.` prefix so it
//! rides next to the echo/nano messages without touching them. Rust is the source of truth; the shared fixture
//! `native/qrtz-helper/Fixtures/ctrl/protocol.jsonl` holds one line per message for both sides.
//!
//! The wire may carry content the log never holds (window titles in a snapshot, the URL to open); the host turns
//! it into hashes and refs before anything is appended (ADR 0083). Nothing here can express an activation: `act`
//! carries a ladder [`Route`] and the helper re-checks it against the policy before doing anything (ADR 0100).
//!
//! [`HelperClient`] is the typed host side over any [`Wire`]; [`FakeHelper`] is an in-memory helper (a small
//! simulated Mac: running apps, browser tabs, windows, one AX tree, the user's frontmost app and cursor) used by the
//! Linux tests and the end-to-end executor tests. It can be told to misbehave (steal focus) to prove the guard.
use crate::ax::AxNodeWire;
use crate::guard::{Point, UiSnapshot};
use crate::ladder::{Env, Route, ScriptVerb, WorkspaceOp};
use crate::layers::{AppCatalog, AppEntry, AppSource};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub const CTRL_PROTOCOL_VERSION: u32 = 1;

/// What a verify step measures (the helper answers with a number, or `null` when it can't tell).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case")]
pub enum Check {
    /// Tabs across the app's windows (scripting `count tabs`).
    TabCount { app: String },
    /// The app's windows.
    WindowCount { app: String },
    /// 1 if the app is running, else 0.
    AppRunning { app: String },
    /// Re-snapshot this AX scope; verified by `obs.diff` (something changed), not by a number.
    AxScope { app: String, scope: Option<String> },
    /// Slice 7b: tabs that are neither pinned nor hold unsaved input (blank new-tab pages not counted).
    SavedTabs { app: String },
    /// Slice 7b: tabs that are not pinned (blank new-tab pages not counted).
    UnpinnedTabs { app: String },
    /// Planned files not yet where a file batch puts them (slice 7). Measured by the host from the disk, never
    /// sent to the helper.
    FilesRemaining,
    /// ADR 0130: of these apps, how many still run. Measured by the host from `ctrl.apps`.
    AppsRemaining { apps: Vec<String> },
    /// ADR 0134: of these apps, how many are still on screen (running, not hidden, not frontmost). Measured by the
    /// host from `ctrl.apps`.
    AppsVisible { apps: Vec<String> },
    /// ADR 0152: of these apps, how many are still hidden (running and hidden). Measured by the host from
    /// `ctrl.apps`.
    AppsHidden { apps: Vec<String> },
    /// ADR 0135: 1 if the Mac's sound output is muted, 0 if not (`null`: no output device that can mute).
    OutputMuted,
    /// ADR 0137: 1 if the Mac is in dark mode, 0 in light mode (read in the helper's own process from the global
    /// `AppleInterfaceStyle`; no app asked, no prompt).
    DarkMode,
    /// ADR 0140: the Mac's output volume in percent, 0–100 (`null`: no output device with a volume, or muted, so
    /// a level that can't be heard never reads as set).
    OutputVolume,
    /// ADR 0158: 1 if a Focus is on, 0 if off (`null`: can't tell; the Mac helper doesn't read it, since
    /// `INFocusStatusCenter` would be another prompt, so a focus run is verified by the shortcut's reply only).
    FocusOn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    Zero,
    Decreased,
    Increased,
    /// 1 after (app running).
    One,
    /// The AX scope changed.
    Changed,
    /// ADR 0140: this exact level after, give or take 1 (macOS keeps the volume as a fraction, so a set 30 can read
    /// back as 29 or 31).
    Level(i64),
}

/// Host → helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HostMsg {
    /// LaunchServices / NSWorkspace: installed and running apps, the default `https` handler.
    #[serde(rename = "ctrl.catalog")]
    Catalog { id: String },
    /// The ladder's `Env` (grants checked silently, frontmost app, HID idle, secure input) and the guard's snapshot.
    #[serde(rename = "ctrl.probe")]
    Probe { id: String },
    /// A background app's front window top level (`scope: null`) or one element's children.
    #[serde(rename = "ctrl.snapshot")]
    Snapshot {
        id: String,
        app: String,
        scope: Option<String>,
    },
    /// Perform one ladder route. `content` resolves the route's refs (a URL) for this call only.
    #[serde(rename = "ctrl.act")]
    Act {
        id: String,
        #[serde(rename = "actionId")]
        action_id: String,
        route: Route,
        content: BTreeMap<String, String>,
    },
    #[serde(rename = "ctrl.measure")]
    Measure { id: String, check: Check },
    /// Ask for the previously frontmost app back (after a guard violation).
    #[serde(rename = "ctrl.restore")]
    Restore {
        id: String,
        #[serde(rename = "frontmostPid")]
        frontmost_pid: Option<i32>,
    },
    /// One Laya `choice` forward pass on the helper's Core ML runtime (consented model only).
    #[serde(rename = "ctrl.laya")]
    Laya {
        id: String,
        /// The consented, installed model the helper should run (ADR 0116). The host sends it only after the
        /// user agreed to the download in onboarding / settings; without it the helper answers `laya.no_model`
        /// (or uses the `QRTZ_CTRL_LAYA_MODEL` it was started with). Omitted when `None` (older fixture lines).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        state: String,
        question: String,
        options: Vec<String>,
    },
    /// ADR 0133: warm the consented, installed model at `model` (its tokenizer, then the Core ML load and compiled
    /// cache) on the helper's background queue. The helper answers at once (`started`, `warming` or `warm`) and
    /// does the work off its ctrl queue; nothing is downloaded. Sent only when [`crate::laya::LayaWarmer`] says so.
    #[serde(rename = "ctrl.laya.warm")]
    LayaWarm { id: String, model: String },
    /// The confirm gate's preview (slice 6, ADR 0111): what an irreversible action would touch in `app`, one item
    /// per window or tab, with its title (hashed by the host before anything is logged) and unsaved state.
    #[serde(rename = "ctrl.preview")]
    Preview {
        id: String,
        app: String,
        what: PreviewWhat,
    },
    /// ADR 0130: the running regular apps with the helper's last-used record of each (activation notifications it
    /// observed, never the screen), when it began watching, and local midnight on the Mac.
    #[serde(rename = "ctrl.apps")]
    Apps { id: String },
}

/// One running app as `ctrl.apps` reports it. `name` labels the gate's card and is hashed before the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningAppWire {
    pub bundle_id: String,
    pub name: String,
    /// The last activation the helper saw (UTC ms); `null` = none since it began watching.
    pub last_active_ms: Option<u64>,
    pub frontmost: bool,
    /// `false` only when the helper knows there's nothing to save (no windows); `null` = can't tell.
    pub unsaved: Option<bool>,
    /// ADR 0134: `NSRunningApplication.isHidden`; absent from an older helper (`null` = can't tell).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

/// The `ctrl.apps` reply.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppsReport {
    pub apps: Vec<RunningAppWire>,
    pub since_ms: Option<u64>,
    pub day_start_ms: u64,
}

/// What a confirm-gate preview enumerates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewWhat {
    Windows,
    Tabs,
}

/// One previewed item as the helper reports it. `unsaved: null` = the helper can't tell (counted as unsaved).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewItemWire {
    pub title: String,
    pub unsaved: Option<bool>,
    /// Tabs only (slice 7b): a pinned tab is never closed by "close all my tabs". Absent / `null` = can't tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogApp {
    pub bundle_id: String,
    pub name: String,
    pub browser: bool,
    pub running: bool,
}

/// Helper → host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HelperMsg {
    #[serde(rename = "ctrl.catalog.result")]
    CatalogResult {
        id: String,
        apps: Vec<CatalogApp>,
        #[serde(rename = "defaultBrowser")]
        default_browser: Option<String>,
    },
    #[serde(rename = "ctrl.probe.result")]
    ProbeResult { id: String, env: Env, ui: UiSnapshot },
    #[serde(rename = "ctrl.snapshot.result")]
    SnapshotResult { id: String, nodes: Vec<AxNodeWire> },
    #[serde(rename = "ctrl.act.result")]
    ActResult {
        id: String,
        ok: bool,
        err: Option<String>,
    },
    #[serde(rename = "ctrl.measure.result")]
    MeasureResult { id: String, value: Option<i64> },
    #[serde(rename = "ctrl.restore.result")]
    RestoreResult { id: String, ok: bool },
    #[serde(rename = "ctrl.laya.result")]
    LayaResult { id: String, p: Vec<f32> },
    /// ADR 0133: `state` is `started` (queued now), `warming` (already in flight) or `warm` (loaded).
    #[serde(rename = "ctrl.laya.warm.result")]
    LayaWarmResult { id: String, state: String },
    #[serde(rename = "ctrl.preview.result")]
    PreviewResult { id: String, items: Vec<PreviewItemWire> },
    #[serde(rename = "ctrl.apps.result")]
    AppsResult {
        id: String,
        apps: Vec<RunningAppWire>,
        #[serde(rename = "sinceMs")]
        since_ms: Option<u64>,
        #[serde(rename = "dayStartMs")]
        day_start_ms: u64,
    },
    /// A short code (`ax.not_trusted`, `app.not_running`, `policy.refused`, `laya.no_model`), never content.
    #[serde(rename = "ctrl.error")]
    Error { id: String, code: String },
}

impl HelperMsg {
    pub fn id(&self) -> &str {
        match self {
            HelperMsg::CatalogResult { id, .. }
            | HelperMsg::ProbeResult { id, .. }
            | HelperMsg::SnapshotResult { id, .. }
            | HelperMsg::ActResult { id, .. }
            | HelperMsg::MeasureResult { id, .. }
            | HelperMsg::RestoreResult { id, .. }
            | HelperMsg::LayaResult { id, .. }
            | HelperMsg::LayaWarmResult { id, .. }
            | HelperMsg::PreviewResult { id, .. }
            | HelperMsg::AppsResult { id, .. }
            | HelperMsg::Error { id, .. } => id,
        }
    }
}

/// One request line out, one reply line back. The real one is the helper process's stdio (wired through
/// `qrtz-helper-bridge` once the Swift side lands); tests use [`FakeHelper`].
pub trait Wire: Send + Sync {
    fn roundtrip(&self, line: &str) -> Result<String, String>;
    /// Whether the helper is there right now (slice 5b: the real helper process is supervised and may be
    /// restarting). While it isn't, the service runs as if no helper were connected (runs stop at `planned`).
    fn up(&self) -> bool {
        true
    }
}

/// The typed host side of the protocol.
pub struct HelperClient {
    wire: Box<dyn Wire>,
    next: AtomicU64,
}

fn proto_err(code: &str) -> String {
    format!("helper.{code}")
}

impl HelperClient {
    pub fn new(wire: Box<dyn Wire>) -> Self {
        HelperClient {
            wire,
            next: AtomicU64::new(0),
        }
    }

    /// See [`Wire::up`].
    pub fn is_up(&self) -> bool {
        self.wire.up()
    }

    fn id(&self) -> String {
        format!("c{}", self.next.fetch_add(1, Ordering::Relaxed))
    }

    fn call(&self, msg: HostMsg) -> Result<HelperMsg, String> {
        let line = serde_json::to_string(&msg).map_err(|_| proto_err("encode"))?;
        let want = match &msg {
            HostMsg::Catalog { id }
            | HostMsg::Probe { id }
            | HostMsg::Snapshot { id, .. }
            | HostMsg::Act { id, .. }
            | HostMsg::Measure { id, .. }
            | HostMsg::Restore { id, .. }
            | HostMsg::Laya { id, .. }
            | HostMsg::LayaWarm { id, .. }
            | HostMsg::Preview { id, .. }
            | HostMsg::Apps { id } => id.clone(),
        };
        let reply = self.wire.roundtrip(&line)?;
        let r: HelperMsg = serde_json::from_str(reply.trim()).map_err(|_| proto_err("bad_reply"))?;
        if r.id() != want {
            return Err(proto_err("wrong_id"));
        }
        match r {
            HelperMsg::Error { code, .. } => Err(code),
            r => Ok(r),
        }
    }

    pub fn catalog(&self) -> Result<AppCatalog, String> {
        match self.call(HostMsg::Catalog { id: self.id() })? {
            HelperMsg::CatalogResult {
                apps,
                default_browser,
                ..
            } => Ok(AppCatalog {
                apps: apps
                    .into_iter()
                    .map(|a| AppEntry {
                        id: a.bundle_id,
                        name: a.name,
                        browser: a.browser,
                        running: a.running,
                    })
                    .collect(),
                default_browser,
                ax: true,
            }),
            _ => Err(proto_err("unexpected")),
        }
    }

    pub fn probe(&self) -> Result<(Env, UiSnapshot), String> {
        match self.call(HostMsg::Probe { id: self.id() })? {
            HelperMsg::ProbeResult { env, ui, .. } => Ok((env, ui)),
            _ => Err(proto_err("unexpected")),
        }
    }

    pub fn snapshot(&self, app: &str, scope: Option<&str>) -> Result<Vec<AxNodeWire>, String> {
        match self.call(HostMsg::Snapshot {
            id: self.id(),
            app: app.into(),
            scope: scope.map(str::to_string),
        })? {
            HelperMsg::SnapshotResult { nodes, .. } => Ok(nodes),
            _ => Err(proto_err("unexpected")),
        }
    }

    pub fn act(
        &self,
        action_id: &str,
        route: &Route,
        content: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        match self.call(HostMsg::Act {
            id: self.id(),
            action_id: action_id.into(),
            route: route.clone(),
            content: content.clone(),
        })? {
            HelperMsg::ActResult { ok: true, .. } => Ok(()),
            HelperMsg::ActResult { err, .. } => Err(err.unwrap_or_else(|| "act.failed".into())),
            _ => Err(proto_err("unexpected")),
        }
    }

    pub fn measure(&self, check: &Check) -> Option<i64> {
        match self.call(HostMsg::Measure {
            id: self.id(),
            check: check.clone(),
        }) {
            Ok(HelperMsg::MeasureResult { value, .. }) => value,
            _ => None,
        }
    }

    pub fn restore(&self, before: &UiSnapshot) -> bool {
        matches!(
            self.call(HostMsg::Restore {
                id: self.id(),
                frontmost_pid: before.frontmost_pid,
            }),
            Ok(HelperMsg::RestoreResult { ok: true, .. })
        )
    }

    pub fn laya(
        &self,
        model: Option<&std::path::Path>,
        state: &str,
        question: &str,
        options: &[String],
    ) -> Result<Vec<f32>, String> {
        match self.call(HostMsg::Laya {
            id: self.id(),
            model: model.map(|p| p.to_string_lossy().into_owned()),
            state: state.into(),
            question: question.into(),
            options: options.to_vec(),
        })? {
            HelperMsg::LayaResult { p, .. } => Ok(p),
            _ => Err(proto_err("unexpected")),
        }
    }

    /// ADR 0133: ask the helper to warm the model at `model` in the background; the reply comes at once.
    pub fn laya_warm(&self, model: &std::path::Path) -> Result<String, String> {
        match self.call(HostMsg::LayaWarm {
            id: self.id(),
            model: model.to_string_lossy().into_owned(),
        })? {
            HelperMsg::LayaWarmResult { state, .. } => Ok(state),
            _ => Err(proto_err("unexpected")),
        }
    }
}

impl HelperClient {
    /// The confirm gate's preview of `app`'s windows or tabs (an error code when the helper can't enumerate them).
    pub fn preview(&self, app: &str, what: PreviewWhat) -> Result<Vec<PreviewItemWire>, String> {
        match self.call(HostMsg::Preview {
            id: self.id(),
            app: app.into(),
            what,
        })? {
            HelperMsg::PreviewResult { items, .. } => Ok(items),
            _ => Err(proto_err("unexpected")),
        }
    }

    /// ADR 0130: running apps with their last-used record (an error code when the helper doesn't watch).
    pub fn apps(&self) -> Result<AppsReport, String> {
        match self.call(HostMsg::Apps { id: self.id() })? {
            HelperMsg::AppsResult {
                apps,
                since_ms,
                day_start_ms,
                ..
            } => Ok(AppsReport {
                apps,
                since_ms,
                day_start_ms,
            }),
            _ => Err(proto_err("unexpected")),
        }
    }
}

/// The app catalog from the helper (LaunchServices: running apps and the real default browser). Replaces
/// [`crate::layers::ScanApps`] whenever a helper is connected; an unreachable helper gives an empty catalog
/// (the chain then asks instead of guessing).
pub struct HelperApps<'a>(pub &'a HelperClient);
impl AppSource for HelperApps<'_> {
    fn catalog(&self) -> AppCatalog {
        self.0.catalog().unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The fake helper

#[derive(Debug, Clone, PartialEq)]
pub struct FakeApp {
    pub bundle_id: String,
    pub name: String,
    pub browser: bool,
    pub running: bool,
    pub pid: i32,
    pub tabs: i64,
    pub windows: i64,
    /// Front window top level, and children per element id.
    pub ax_top: Vec<AxNodeWire>,
    pub ax_children: BTreeMap<String, Vec<AxNodeWire>>,
    /// Scripting verbs this app understands.
    pub scriptable: bool,
    /// Window titles and unsaved state for the confirm gate's preview (empty: `window 1` … `window n`).
    pub docs: Vec<(String, bool)>,
    /// The fake can't enumerate this app's windows (the preview errors, the host falls back to a count).
    pub no_preview: bool,
    /// Slice 7b: pinned tabs and tabs with unsaved form input (titles), on top of `tabs` plain ones.
    pub pinned_tabs: Vec<String>,
    pub unsaved_tabs: Vec<String>,
    /// ADR 0134: hidden (`NSRunningApplication.hide`).
    pub hidden: bool,
}

impl FakeApp {
    pub fn new(bundle_id: &str, name: &str, pid: i32) -> Self {
        FakeApp {
            bundle_id: bundle_id.into(),
            name: name.into(),
            browser: false,
            running: true,
            pid,
            tabs: 0,
            windows: 1,
            ax_top: Vec::new(),
            ax_children: BTreeMap::new(),
            scriptable: false,
            docs: Vec::new(),
            no_preview: false,
            pinned_tabs: Vec::new(),
            unsaved_tabs: Vec::new(),
            hidden: false,
        }
    }
    pub fn browser(mut self, tabs: i64) -> Self {
        self.browser = true;
        self.scriptable = true;
        self.tabs = tabs;
        self
    }
}

/// The simulated session. `frontmost`, `cursor` and `focused` are the user's and nothing ctrl does may change them,
/// unless `steal_focus` is set (a deliberately broken helper, to prove the guard catches it).
#[derive(Debug, Clone)]
pub struct FakeWorld {
    pub apps: Vec<FakeApp>,
    pub default_browser: Option<String>,
    pub frontmost_pid: i32,
    pub cursor: Point,
    pub focused: String,
    pub env: Env,
    pub steal_focus: bool,
    /// Acts that "succeed" without changing anything (a verify must catch it).
    pub no_effect: bool,
    /// Every act the helper performed, for assertions: (action id, route).
    pub acted: Vec<(String, Route)>,
    pub opened_urls: Vec<String>,
    pub laya: Option<Vec<f32>>,
    /// The `model` of every `ctrl.laya` request, for assertions (ADR 0116).
    pub laya_models: Vec<Option<String>>,
    /// ADR 0133: the `model` of every `ctrl.laya.warm` request, in order.
    pub laya_warms: Vec<String>,
    /// ADR 0130: last activation per bundle id, when the watch began (`None`: `ctrl.apps` errors), local midnight.
    pub last_active: BTreeMap<String, u64>,
    pub watch_since_ms: Option<u64>,
    pub day_start_ms: u64,
    /// ADR 0135: the sound output's mute (`None`: no output that can mute).
    pub muted: Option<bool>,
    /// ADR 0137: the system appearance (true = dark).
    pub dark: bool,
    /// ADR 0140: the output volume in percent (read back only while unmuted).
    pub volume: i64,
    /// ADR 0158: the user made the "qrtz Set Focus" shortcut (false: `focus.no_shortcut`).
    pub focus_shortcut: bool,
    /// ADR 0158: Automation for Shortcuts Events was denied (`automation.denied`).
    pub shortcuts_denied: bool,
    /// ADR 0158: the Focus the shortcut last set (`Off`: none).
    pub focus: crate::ladder::FocusMode,
}

pub struct FakeHelper(pub Mutex<FakeWorld>);

impl FakeHelper {
    pub fn new(world: FakeWorld) -> Self {
        FakeHelper(Mutex::new(world))
    }
    pub fn world(&self) -> FakeWorld {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl FakeWorld {
    /// A Mac with Jared in Notes (frontmost), Safari as default browser with `tabs` tabs in the background, and every
    /// grant given.
    pub fn typical(tabs: i64) -> FakeWorld {
        let notes = FakeApp::new("com.apple.Notes", "Notes", 100);
        let safari = FakeApp::new("com.apple.Safari", "Safari", 200).browser(tabs);
        let chrome = FakeApp {
            running: false,
            ..FakeApp::new("com.google.Chrome", "Google Chrome", 300).browser(0)
        };
        let finder = FakeApp::new("com.apple.finder", "Finder", 50);
        let mut env = Env {
            frontmost_app: Some("com.apple.Notes".into()),
            user_idle_ms: Some(30_000),
            files: Some(crate::ladder::Consent::Granted),
            accessibility: Some(crate::ladder::Consent::Granted),
            post_event: Some(crate::ladder::Consent::Granted),
            ..Env::default()
        };
        for a in ["com.apple.Safari", "com.google.Chrome", "com.apple.TextEdit"] {
            env.automation.insert(a.into(), crate::ladder::Consent::Granted);
        }
        FakeWorld {
            apps: vec![notes, safari, chrome, finder],
            default_browser: Some("com.apple.Safari".into()),
            frontmost_pid: 100,
            cursor: Point { x: 400.0, y: 300.0 },
            focused: "e:notes-body".into(),
            env,
            steal_focus: false,
            no_effect: false,
            acted: Vec::new(),
            opened_urls: Vec::new(),
            laya: None,
            laya_models: Vec::new(),
            laya_warms: Vec::new(),
            last_active: BTreeMap::new(),
            watch_since_ms: None,
            day_start_ms: 0,
            muted: Some(false),
            dark: false,
            focus_shortcut: false,
            shortcuts_denied: false,
            focus: crate::ladder::FocusMode::Off,
            volume: 50,
        }
    }

    fn app_mut(&mut self, id: &str) -> Option<&mut FakeApp> {
        self.apps.iter_mut().find(|a| a.bundle_id == id)
    }

    fn running_env(&self) -> Env {
        let mut env = self.env.clone();
        env.running_apps = self
            .apps
            .iter()
            .filter(|a| a.running)
            .map(|a| a.bundle_id.clone())
            .collect();
        env.frontmost_app = self
            .apps
            .iter()
            .find(|a| a.pid == self.frontmost_pid)
            .map(|a| a.bundle_id.clone());
        env
    }

    fn ui(&self) -> UiSnapshot {
        UiSnapshot {
            frontmost_pid: Some(self.frontmost_pid),
            cursor: Some(self.cursor),
            focused_element: Some(self.focused.clone()),
            key_window: Some(1),
            hid_idle_ms: self.env.user_idle_ms,
        }
    }

    /// The helper re-checks the ladder before acting (ADR 0100 §6): a route the policy refuses never runs.
    fn act(&mut self, route: &Route, content: &BTreeMap<String, String>) -> Result<(), String> {
        let env = self.running_env();
        if crate::ladder::judge(route, &env) != crate::ladder::Verdict::Allowed {
            return Err("policy.refused".into());
        }
        if self.steal_focus {
            if let Some(pid) = route
                .app()
                .and_then(|a| self.apps.iter().find(|x| x.bundle_id == a))
                .map(|a| a.pid)
            {
                self.frontmost_pid = pid;
            }
        }
        if self.no_effect {
            return Ok(());
        }
        match route {
            // ADR 0137: the appearance, through System Events only.
            Route::Scripting {
                app,
                verb: v @ (ScriptVerb::SetDarkMode | ScriptVerb::SetLightMode),
            } => {
                if app != crate::exec::SYSTEM_EVENTS {
                    return Err("policy.refused".into());
                }
                self.dark = *v == ScriptVerb::SetDarkMode;
                Ok(())
            }
            Route::Scripting { app, verb } => {
                let a = self.app_mut(app).ok_or("app.not_running")?;
                if !a.scriptable || !a.running {
                    return Err("script.unsupported".into());
                }
                match verb {
                    ScriptVerb::CloseAllTabs => {
                        a.tabs = 0;
                        a.pinned_tabs.clear();
                        a.unsaved_tabs.clear();
                    }
                    ScriptVerb::CloseSavedTabs => a.tabs = 0,
                    ScriptVerb::CloseUnpinnedTabs => {
                        a.tabs = 0;
                        a.unsaved_tabs.clear();
                    }
                    ScriptVerb::CloseTab => a.tabs = (a.tabs - 1).max(0),
                    ScriptVerb::CloseWindow => a.windows = (a.windows - 1).max(0),
                    ScriptVerb::CountTabs | ScriptVerb::CountWindows => {}
                    ScriptVerb::SetDarkMode | ScriptVerb::SetLightMode => {
                        return Err("script.unsupported".into())
                    }
                }
                Ok(())
            }
            Route::Workspace(WorkspaceOp::OpenUrl { app, url }) => {
                let u = content.get(&url.0).ok_or("content.missing")?.clone();
                let a = self.app_mut(app).ok_or("app.not_running")?;
                a.tabs += 1;
                self.opened_urls.push(u);
                Ok(())
            }
            Route::Workspace(WorkspaceOp::LaunchHidden { app }) => {
                self.app_mut(app).ok_or("app.unknown")?.running = true;
                Ok(())
            }
            // ADR 0130: terminate each (no activation); one that already quit is fine.
            Route::Workspace(WorkspaceOp::QuitApps { apps }) => {
                for id in apps {
                    if let Some(a) = self.app_mut(id) {
                        a.running = false;
                        a.windows = 0;
                        a.tabs = 0;
                        a.docs.clear();
                    }
                }
                Ok(())
            }
            // ADR 0134: hide each (no activation); never the frontmost (the helper skips it, like the Mac side).
            Route::Workspace(WorkspaceOp::HideApps { apps }) => {
                let front = self.frontmost_pid;
                for id in apps {
                    if let Some(a) = self.app_mut(id) {
                        if a.pid != front {
                            a.hidden = true;
                        }
                    }
                }
                Ok(())
            }
            // ADR 0152: unhide each (no activation: the frontmost app stays frontmost); one not running is skipped.
            Route::Workspace(WorkspaceOp::UnhideApps { apps }) => {
                for id in apps {
                    if let Some(a) = self.app_mut(id) {
                        if a.running {
                            a.hidden = false;
                        }
                    }
                }
                Ok(())
            }
            // ADR 0135: no output that can mute fails like the Mac's `set volume` would.
            Route::Workspace(WorkspaceOp::MuteOutput) => match self.muted {
                Some(_) => {
                    self.muted = Some(true);
                    Ok(())
                }
                None => Err("sound.no_output".into()),
            },
            Route::Workspace(WorkspaceOp::SetVolume { level }) => match self.muted {
                Some(_) => {
                    self.muted = Some(false);
                    self.volume = i64::from(*level);
                    Ok(())
                }
                None => Err("sound.no_output".into()),
            },
            // ADR 0158: the same answers as the Mac helper: denied Automation, then a missing shortcut, stop.
            Route::Workspace(WorkspaceOp::SetFocus { mode }) => {
                if self.shortcuts_denied {
                    return Err("automation.denied".into());
                }
                if !self.focus_shortcut {
                    return Err("focus.no_shortcut".into());
                }
                self.focus = *mode;
                Ok(())
            }
            Route::Workspace(WorkspaceOp::UnmuteOutput) => match self.muted {
                Some(_) => {
                    self.muted = Some(false);
                    Ok(())
                }
                None => Err("sound.no_output".into()),
            },
            Route::Ax { app, element, .. } => {
                let a = self.app_mut(app).ok_or("app.not_running")?;
                let before = a.ax_top.len();
                a.ax_top.retain(|n| n.id != element.0);
                for kids in a.ax_children.values_mut() {
                    kids.retain(|n| n.id != element.0);
                }
                if before != a.ax_top.len() {
                    a.windows = (a.windows - 1).max(0);
                }
                Ok(())
            }
            Route::PidKey { app, combo } => {
                let a = self.app_mut(app).ok_or("app.not_running")?;
                match combo.as_str() {
                    "cmd+w" if a.browser => a.tabs = (a.tabs - 1).max(0),
                    "cmd+w" => a.windows = (a.windows - 1).max(0),
                    "cmd+option+w" => a.tabs = a.tabs.min(1),
                    // Quit: the app's windows go with it (a real app would show its save sheet for unsaved work).
                    "cmd+q" => {
                        a.running = false;
                        a.windows = 0;
                        a.tabs = 0;
                        a.docs.clear();
                    }
                    _ => {}
                }
                Ok(())
            }
            Route::Files(_) => Err("files.not_in_helper".into()),
            Route::Click { .. } | Route::GlobalKey { .. } | Route::Activate { .. } => {
                Err("policy.refused".into())
            }
        }
    }

    pub fn handle(&mut self, msg: HostMsg) -> HelperMsg {
        match msg {
            HostMsg::Catalog { id } => HelperMsg::CatalogResult {
                id,
                apps: self
                    .apps
                    .iter()
                    .map(|a| CatalogApp {
                        bundle_id: a.bundle_id.clone(),
                        name: a.name.clone(),
                        browser: a.browser,
                        running: a.running,
                    })
                    .collect(),
                default_browser: self.default_browser.clone(),
            },
            HostMsg::Probe { id } => HelperMsg::ProbeResult {
                id,
                env: self.running_env(),
                ui: self.ui(),
            },
            HostMsg::Snapshot { id, app, scope } => match self.apps.iter().find(|a| a.bundle_id == app) {
                Some(a) if a.running => HelperMsg::SnapshotResult {
                    id,
                    nodes: match scope {
                        None => a.ax_top.clone(),
                        Some(s) => a.ax_children.get(&s).cloned().unwrap_or_default(),
                    },
                },
                _ => HelperMsg::Error {
                    id,
                    code: "app.not_running".into(),
                },
            },
            HostMsg::Act {
                id,
                action_id,
                route,
                content,
            } => {
                let r = self.act(&route, &content);
                if r.is_ok() {
                    self.acted.push((action_id, route));
                }
                HelperMsg::ActResult {
                    id,
                    ok: r.is_ok(),
                    err: r.err(),
                }
            }
            HostMsg::Measure { id, check } => {
                let value = match &check {
                    Check::TabCount { app } => self
                        .apps
                        .iter()
                        .find(|a| &a.bundle_id == app && a.running && a.scriptable)
                        .map(|a| a.tabs + a.pinned_tabs.len() as i64 + a.unsaved_tabs.len() as i64),
                    Check::SavedTabs { app } => self
                        .apps
                        .iter()
                        .find(|a| &a.bundle_id == app && a.running && a.scriptable)
                        .map(|a| a.tabs),
                    Check::UnpinnedTabs { app } => self
                        .apps
                        .iter()
                        .find(|a| &a.bundle_id == app && a.running && a.scriptable)
                        .map(|a| a.tabs + a.unsaved_tabs.len() as i64),
                    Check::WindowCount { app } => self
                        .apps
                        .iter()
                        .find(|a| &a.bundle_id == app && a.running)
                        .map(|a| a.windows),
                    Check::AppRunning { app } => Some(i64::from(
                        self.apps.iter().any(|a| &a.bundle_id == app && a.running),
                    )),
                    Check::AxScope { .. }
                    | Check::FilesRemaining
                    | Check::AppsRemaining { .. }
                    | Check::AppsVisible { .. }
                    | Check::AppsHidden { .. } => None,
                    Check::OutputMuted => self.muted.map(i64::from),
                    Check::DarkMode => Some(i64::from(self.dark)),
                    Check::FocusOn => Some(i64::from(self.focus != crate::ladder::FocusMode::Off)),
                    Check::OutputVolume => match self.muted {
                        Some(false) => Some(self.volume),
                        _ => None,
                    },
                };
                HelperMsg::MeasureResult { id, value }
            }
            HostMsg::Restore { id, frontmost_pid } => {
                if let Some(p) = frontmost_pid {
                    self.frontmost_pid = p;
                }
                HelperMsg::RestoreResult {
                    id,
                    ok: frontmost_pid.is_some(),
                }
            }
            HostMsg::Preview { id, app, what } => match self.apps.iter().find(|a| a.bundle_id == app) {
                Some(a) if a.running && !a.no_preview => {
                    let items = match what {
                        PreviewWhat::Windows if !a.docs.is_empty() => a
                            .docs
                            .iter()
                            .map(|(t, u)| PreviewItemWire {
                                title: t.clone(),
                                unsaved: Some(*u),
                                pinned: None,
                            })
                            .collect(),
                        PreviewWhat::Windows => (1..=a.windows)
                            .map(|i| PreviewItemWire {
                                title: format!("window {i}"),
                                unsaved: Some(false),
                                pinned: None,
                            })
                            .collect(),
                        PreviewWhat::Tabs => {
                            let tab = |title: String, unsaved, pinned| PreviewItemWire {
                                title,
                                unsaved: Some(unsaved),
                                pinned: Some(pinned),
                            };
                            a.pinned_tabs
                                .iter()
                                .map(|t| tab(t.clone(), false, true))
                                .chain((1..=a.tabs).map(|i| tab(format!("tab {i}"), false, false)))
                                .chain(a.unsaved_tabs.iter().map(|t| tab(t.clone(), true, false)))
                                .collect()
                        }
                    };
                    HelperMsg::PreviewResult { id, items }
                }
                Some(a) if a.running => HelperMsg::Error {
                    id,
                    code: "preview.unsupported".into(),
                },
                _ => HelperMsg::Error {
                    id,
                    code: "app.not_running".into(),
                },
            },
            HostMsg::Apps { id } => match self.watch_since_ms {
                None => HelperMsg::Error {
                    id,
                    code: "apps.not_watching".into(),
                },
                Some(since) => HelperMsg::AppsResult {
                    id,
                    apps: self
                        .apps
                        .iter()
                        .filter(|a| a.running)
                        .map(|a| RunningAppWire {
                            bundle_id: a.bundle_id.clone(),
                            name: a.name.clone(),
                            last_active_ms: self.last_active.get(&a.bundle_id).copied(),
                            frontmost: a.pid == self.frontmost_pid,
                            unsaved: if !a.docs.is_empty() {
                                Some(a.docs.iter().any(|(_, u)| *u))
                            } else if a.windows == 0 {
                                Some(false)
                            } else {
                                None
                            },
                            hidden: Some(a.hidden),
                        })
                        .collect(),
                    since_ms: Some(since),
                    day_start_ms: self.day_start_ms,
                },
            },
            HostMsg::LayaWarm { id, model } => {
                let again = self.laya_warms.contains(&model);
                self.laya_warms.push(model);
                HelperMsg::LayaWarmResult {
                    id,
                    state: if again { "warm" } else { "started" }.into(),
                }
            }
            HostMsg::Laya {
                id, options, model, ..
            } => {
                self.laya_models.push(model);
                match &self.laya {
                    Some(p) if p.len() == options.len() => HelperMsg::LayaResult { id, p: p.clone() },
                    _ => HelperMsg::Error {
                        id,
                        code: "laya.no_model".into(),
                    },
                }
            }
        }
    }
}

/// The fake helper speaks the real wire: JSON lines in and out.
impl Wire for FakeHelper {
    fn roundtrip(&self, line: &str) -> Result<String, String> {
        let msg: HostMsg = serde_json::from_str(line).map_err(|_| "helper.bad_request".to_string())?;
        let reply = self.0.lock().unwrap_or_else(|e| e.into_inner()).handle(msg);
        serde_json::to_string(&reply).map_err(|_| "helper.encode".into())
    }
}

/// Lets tests keep a handle on the fake world while the service owns the client.
impl<W: Wire> Wire for std::sync::Arc<W> {
    fn roundtrip(&self, line: &str) -> Result<String, String> {
        (**self).roundtrip(line)
    }
    fn up(&self) -> bool {
        (**self).up()
    }
}
