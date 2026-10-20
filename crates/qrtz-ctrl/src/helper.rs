//! The ctrl helper protocol (ADR 0108, reshaped by ADR 0214 for ctrl do): how the host asks the Swift helper's
//! `QrtzCtrl` module to read the accessibility tree and act on it. Newline-delimited JSON, one request → one reply
//! with the same `id`, all under the `ctrl.` prefix. Rust is the source of truth; the shared fixture
//! `native/qrtz-helper/Fixtures/ctrl/protocol.jsonl` holds one line per message for both sides.
//!
//! - `ctrl.catalog`: installed and running apps, the default browser.
//! - `ctrl.tree {app, scope?, depth}`: the front window of `app` (or one element's children), pruned of secure
//!   fields by the helper itself (it never reads an `AXSecureTextField`), each element under a handle it created.
//! - `ctrl.menu.shortcuts {app}`: the app's menu-bar items that have a key equivalent.
//! - `ctrl.ax.act {runId, action, sig?}`: one action from the fixed set ([`crate::act::Action`]) on a handle the
//!   helper created; `sig` is the element's fingerprint the host saw, re-checked by the helper (`act.stale`).
//! - `ctrl.observe {runId, arm?}`: the stop-key watcher's flags; `arm: true` starts it, `false` stops it.
//! - `ctrl.laya`, `ctrl.laya.warm`: the consented Laya model (ADR 0106, 0133).
//!
//! The wire carries content the log never holds (titles, the text to type); the host hashes it into tokens before
//! anything is appended. [`FakeHelper`] is an in-memory helper driven by fixture worlds (trees per state and
//! transitions), used by the Linux tests.
use crate::act::normalize_combo;
use crate::ax::{AxNodeWire, MenuShortcutWire};
use crate::chain::{AppCatalog, AppEntry};
use crate::guard::Observed;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Version 2: ctrl do (ADR 0214). Version 1 (probe / snapshot / act routes) is gone.
pub const CTRL_PROTOCOL_VERSION: u32 = 2;
/// How deep `ctrl.tree` reads before it reports `more` instead of children.
pub const TREE_DEPTH: u32 = 6;

/// Host → helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HostMsg {
    #[serde(rename = "ctrl.catalog")]
    Catalog { id: String },
    #[serde(rename = "ctrl.tree")]
    Tree {
        id: String,
        app: String,
        scope: Option<String>,
        depth: u32,
    },
    #[serde(rename = "ctrl.menu.shortcuts")]
    MenuShortcuts { id: String, app: String },
    /// `action` is [`crate::act::Action::wire`]: the action with its text resolved for this call only.
    #[serde(rename = "ctrl.ax.act")]
    AxAct {
        id: String,
        #[serde(rename = "runId")]
        run_id: String,
        action: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sig: Option<String>,
    },
    #[serde(rename = "ctrl.observe")]
    Observe {
        id: String,
        #[serde(rename = "runId")]
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arm: Option<bool>,
    },
    /// One Laya `choice` forward pass on the helper's Core ML runtime (consented model only). The host sends
    /// `model` only after Jared agreed to the download in ctrl's settings (ADR 0116).
    #[serde(rename = "ctrl.laya")]
    Laya {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        state: String,
        question: String,
        options: Vec<String>,
    },
    /// ADR 0133: warm the consented, installed model in the background. Nothing is downloaded.
    #[serde(rename = "ctrl.laya.warm")]
    LayaWarm { id: String, model: String },
}

impl HostMsg {
    pub fn id(&self) -> &str {
        match self {
            HostMsg::Catalog { id }
            | HostMsg::Tree { id, .. }
            | HostMsg::MenuShortcuts { id, .. }
            | HostMsg::AxAct { id, .. }
            | HostMsg::Observe { id, .. }
            | HostMsg::Laya { id, .. }
            | HostMsg::LayaWarm { id, .. } => id,
        }
    }
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
    /// `root`: the window (or the scope element) with its children; `focused`: the focused element's handle.
    #[serde(rename = "ctrl.tree.result")]
    TreeResult {
        id: String,
        root: Box<AxNodeWire>,
        #[serde(default)]
        focused: Option<String>,
    },
    #[serde(rename = "ctrl.menu.shortcuts.result")]
    MenuShortcutsResult {
        id: String,
        items: Vec<MenuShortcutWire>,
    },
    #[serde(rename = "ctrl.ax.act.result")]
    AxActResult {
        id: String,
        ok: bool,
        err: Option<String>,
    },
    #[serde(rename = "ctrl.observe.result")]
    ObserveResult {
        id: String,
        #[serde(flatten)]
        observed: Observed,
    },
    #[serde(rename = "ctrl.laya.result")]
    LayaResult { id: String, p: Vec<f32> },
    /// ADR 0133: `state` is `started`, `warming` or `warm`.
    #[serde(rename = "ctrl.laya.warm.result")]
    LayaWarmResult { id: String, state: String },
    /// A short code (`ax.not_trusted`, `app.not_running`, `act.stale`, `act.refused`, `guard.stopped`,
    /// `laya.no_model`), never content.
    #[serde(rename = "ctrl.error")]
    Error { id: String, code: String },
}

impl HelperMsg {
    pub fn id(&self) -> &str {
        match self {
            HelperMsg::CatalogResult { id, .. }
            | HelperMsg::TreeResult { id, .. }
            | HelperMsg::MenuShortcutsResult { id, .. }
            | HelperMsg::AxActResult { id, .. }
            | HelperMsg::ObserveResult { id, .. }
            | HelperMsg::LayaResult { id, .. }
            | HelperMsg::LayaWarmResult { id, .. }
            | HelperMsg::Error { id, .. } => id,
        }
    }
}

/// One request line out, one reply line back: the helper process's stdio in the app, [`FakeHelper`] in tests.
pub trait Wire: Send + Sync {
    fn roundtrip(&self, line: &str) -> Result<String, String>;
    /// Whether the helper is there right now (it is supervised and may be restarting).
    fn up(&self) -> bool {
        true
    }
}

/// A tree read: the root with its children and the focused handle.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRead {
    pub root: AxNodeWire,
    pub focused: Option<String>,
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

    pub fn is_up(&self) -> bool {
        self.wire.up()
    }

    fn id(&self) -> String {
        format!("c{}", self.next.fetch_add(1, Ordering::Relaxed))
    }

    fn call(&self, msg: HostMsg) -> Result<HelperMsg, String> {
        let line = serde_json::to_string(&msg).map_err(|_| proto_err("encode"))?;
        let reply = self.wire.roundtrip(&line)?;
        let r: HelperMsg = serde_json::from_str(reply.trim()).map_err(|_| proto_err("bad_reply"))?;
        if r.id() != msg.id() {
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
            }),
            _ => Err(proto_err("unexpected")),
        }
    }

    pub fn tree(&self, app: &str, scope: Option<&str>) -> Result<TreeRead, String> {
        match self.call(HostMsg::Tree {
            id: self.id(),
            app: app.into(),
            scope: scope.map(str::to_string),
            depth: TREE_DEPTH,
        })? {
            HelperMsg::TreeResult { root, focused, .. } => Ok(TreeRead { root: *root, focused }),
            _ => Err(proto_err("unexpected")),
        }
    }

    pub fn menu_shortcuts(&self, app: &str) -> Result<Vec<MenuShortcutWire>, String> {
        match self.call(HostMsg::MenuShortcuts {
            id: self.id(),
            app: app.into(),
        })? {
            HelperMsg::MenuShortcutsResult { items, .. } => Ok(items),
            _ => Err(proto_err("unexpected")),
        }
    }

    pub fn act(&self, run_id: &str, action: serde_json::Value, sig: Option<&str>) -> Result<(), String> {
        match self.call(HostMsg::AxAct {
            id: self.id(),
            run_id: run_id.into(),
            action,
            sig: sig.filter(|s| !s.is_empty()).map(str::to_string),
        })? {
            HelperMsg::AxActResult { ok: true, .. } => Ok(()),
            HelperMsg::AxActResult { err, .. } => Err(err.unwrap_or_else(|| "act.failed".into())),
            _ => Err(proto_err("unexpected")),
        }
    }

    pub fn observe(&self, run_id: &str, arm: Option<bool>) -> Result<Observed, String> {
        match self.call(HostMsg::Observe {
            id: self.id(),
            run_id: run_id.into(),
            arm,
        })? {
            HelperMsg::ObserveResult { observed, .. } => Ok(observed),
            _ => Err(proto_err("unexpected")),
        }
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

// ---------------------------------------------------------------------------------------------------------------
// The fake helper

/// One app of a fake world: its window in each named state, its menu shortcuts, and what actions do.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FakeApp {
    pub bundle_id: String,
    pub name: String,
    #[serde(default)]
    pub browser: bool,
    #[serde(default)]
    pub running: bool,
    /// The window's state when the app is opened (or already running).
    pub start: String,
    /// State name → the window's tree.
    pub states: BTreeMap<String, AxNodeWire>,
    #[serde(default)]
    pub menu: Vec<MenuShortcutWire>,
    #[serde(default)]
    pub transitions: Vec<Transition>,
}

/// When an action matching `on` happens in state `from` (any state when omitted), the window moves to `to`.
/// `on` is `press:<handle>`, `key:<combo>`, `set_value:<handle>` (`set_value:<handle>:go` when submitted),
/// `focus:<handle>`, `open_url`, `increment:<handle>`, … A `{text}` in `to`'s tree values is the typed text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    #[serde(default)]
    pub from: Option<String>,
    pub on: String,
    pub to: String,
    /// The focused handle afterwards (`None`: keep what the state's tree marks focused).
    #[serde(default)]
    pub focus: Option<String>,
}

/// A small simulated Mac for the chain tests.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FakeWorld {
    pub apps: Vec<FakeApp>,
    #[serde(default)]
    pub default_browser: Option<String>,
    #[serde(default)]
    pub frontmost: Option<String>,
    /// Current state per app (opened apps only).
    #[serde(default)]
    pub state: BTreeMap<String, String>,
    #[serde(default)]
    pub focused: BTreeMap<String, String>,
    /// The last text typed per app (substituted for `{text}`).
    #[serde(default)]
    pub typed: BTreeMap<String, String>,
    /// Every `ctrl.ax.act` the helper received (the wire action), for the tests.
    #[serde(default)]
    pub acts: Vec<serde_json::Value>,
    #[serde(default)]
    pub armed: bool,
    /// Simulate Jared: after this many acts, the watcher reports Esc (`stop_key`) or input (`user_input`).
    #[serde(default)]
    pub trip_after: Option<(usize, String)>,
    #[serde(default)]
    pub tripped: Option<String>,
    /// The watcher can't start (no event-tap permission).
    #[serde(default)]
    pub no_watcher: bool,
    /// What the unarmed watcher says is missing (review (d)).
    #[serde(default)]
    pub missing: Option<String>,
    /// The trees read so far (every `ctrl.tree` reply), for the log-scan test.
    #[serde(default)]
    pub reads: u32,
    /// Simulate Jared pressing Esc while the app catalog is read (the first thing a run does, right after the
    /// ctrl+fn release); like the real tap, only an armed watcher sees it (review H-A).
    #[serde(default)]
    pub esc_on_catalog: bool,
    /// Like the real watcher (CtrlStopWatch): a run's first arm clears the flags, a disarm ends the run.
    #[serde(default)]
    pub session: bool,
}

fn find_wire<'a>(n: &'a AxNodeWire, id: &str) -> Option<&'a AxNodeWire> {
    if n.id == id {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_wire(c, id))
}

fn fill(n: &mut AxNodeWire, text: &str, focused: Option<&str>) {
    if let Some(v) = n.value.as_mut() {
        *v = v.replace("{text}", text);
    }
    n.title = n.title.replace("{text}", text);
    if let Some(f) = focused {
        n.focused = n.id == f;
    }
    for c in &mut n.children {
        fill(c, text, focused);
    }
}

fn focused_of(n: &AxNodeWire) -> Option<String> {
    if n.focused {
        return Some(n.id.clone());
    }
    n.children.iter().find_map(focused_of)
}

impl FakeWorld {
    pub fn from_json(s: &str) -> Result<FakeWorld, serde_json::Error> {
        serde_json::from_str(s)
    }

    fn app(&self, id: &str) -> Option<&FakeApp> {
        self.apps.iter().find(|a| a.bundle_id == id)
    }

    /// The window of `app` as it is now (typed text filled in, focus applied).
    pub fn window(&self, app: &str) -> Option<AxNodeWire> {
        let a = self.app(app)?;
        let st = self.state.get(app)?;
        let mut w = a.states.get(st)?.clone();
        let text = self.typed.get(app).cloned().unwrap_or_default();
        fill(&mut w, &text, self.focused.get(app).map(String::as_str));
        Some(w)
    }

    fn open(&mut self, app: &str) -> Result<(), String> {
        let a = self.app(app).ok_or("app.not_found")?.clone();
        self.state.entry(app.into()).or_insert(a.start);
        self.frontmost = Some(app.into());
        Ok(())
    }

    fn act(&mut self, v: &serde_json::Value, sig: Option<&str>) -> Result<(), String> {
        if self.tripped.is_some() {
            return Err("guard.stopped".into());
        }
        self.acts.push(v.clone());
        if let Some((n, what)) = self.trip_after.clone() {
            if self.acts.len() >= n {
                self.tripped = Some(what);
            }
        }
        let kind = v["action"].as_str().unwrap_or("");
        let handle = v["handle"].as_str().map(str::to_string);
        if kind == "open_app" {
            return self.open(v["app"].as_str().unwrap_or(""));
        }
        let app = match v["app"].as_str() {
            Some(a) => a.to_string(),
            None => self.frontmost.clone().ok_or("app.none")?,
        };
        if self.frontmost.as_deref() != Some(app.as_str()) {
            return Err("app.not_frontmost".into());
        }
        let win = self.window(&app).ok_or("app.not_running")?;
        // A handle must exist in the window or the menu; its sig must match what the host saw.
        if let Some(h) = &handle {
            match find_wire(&win, h) {
                Some(n) => {
                    if sig.is_some_and(|s| s != n.sig) {
                        return Err("act.stale".into());
                    }
                    if n.role == "AXSecureTextField" {
                        return Err("act.refused".into());
                    }
                }
                None => {
                    let in_menu = self.app(&app).is_some_and(|a| a.menu.iter().any(|m| &m.id == h));
                    if !in_menu {
                        return Err("act.stale".into());
                    }
                }
            }
        }
        let on = match kind {
            "key_combo" => format!("key:{}", normalize_combo(v["combo"].as_str().unwrap_or(""))),
            "open_url" => "open_url".into(),
            "set_value" => {
                let text = v["text"].as_str().unwrap_or("").to_string();
                self.typed.insert(app.clone(), text);
                let go = v["submit"].as_bool().unwrap_or(false);
                format!(
                    "set_value:{}{}",
                    handle.clone().unwrap_or_default(),
                    if go { ":go" } else { "" }
                )
            }
            k => format!("{k}:{}", handle.clone().unwrap_or_default()),
        };
        if kind == "open_url" {
            self.typed
                .insert(app.clone(), v["url"].as_str().unwrap_or("").to_string());
        }
        if kind == "focus" {
            if let Some(h) = &handle {
                self.focused.insert(app.clone(), h.clone());
            }
        }
        let cur = self.state.get(&app).cloned().unwrap_or_default();
        let t = self
            .app(&app)
            .and_then(|a| {
                a.transitions
                    .iter()
                    .find(|t| t.on == on && t.from.as_ref().is_none_or(|f| *f == cur))
            })
            .cloned();
        if let Some(t) = t {
            self.state.insert(app.clone(), t.to.clone());
            match t.focus {
                Some(f) => {
                    self.focused.insert(app.clone(), f);
                }
                None => {
                    let focus = self
                        .app(&app)
                        .and_then(|a| a.states.get(&t.to))
                        .and_then(focused_of);
                    match focus {
                        Some(f) => {
                            self.focused.insert(app.clone(), f);
                        }
                        None if kind != "focus" => {
                            self.focused.remove(&app);
                        }
                        None => {}
                    }
                }
            }
        }
        Ok(())
    }

    pub fn handle(&mut self, msg: HostMsg) -> HelperMsg {
        let err = |id: String, code: &str| HelperMsg::Error {
            id,
            code: code.into(),
        };
        match msg {
            HostMsg::Catalog { id } => {
                if self.armed && self.esc_on_catalog {
                    self.tripped = Some("stop_key".into());
                }
                HelperMsg::CatalogResult {
                    id,
                    apps: self
                        .apps
                        .iter()
                        .map(|a| CatalogApp {
                            bundle_id: a.bundle_id.clone(),
                            name: a.name.clone(),
                            browser: a.browser,
                            running: a.running || self.state.contains_key(&a.bundle_id),
                        })
                        .collect(),
                    default_browser: self.default_browser.clone(),
                }
            }
            HostMsg::Tree { id, app, scope, .. } => {
                self.reads += 1;
                let Some(win) = self.window(&app) else {
                    return err(id, "app.not_running");
                };
                let focused = self.focused.get(&app).cloned().or_else(|| focused_of(&win));
                match scope {
                    None => HelperMsg::TreeResult {
                        id,
                        root: Box::new(win),
                        focused,
                    },
                    Some(s) => match find_wire(&win, &s) {
                        Some(n) => HelperMsg::TreeResult {
                            id,
                            root: Box::new(n.clone()),
                            focused,
                        },
                        None => err(id, "ax.gone"),
                    },
                }
            }
            HostMsg::MenuShortcuts { id, app } => match self.app(&app) {
                Some(a) if self.state.contains_key(&app) => HelperMsg::MenuShortcutsResult {
                    id,
                    items: a.menu.clone(),
                },
                _ => err(id, "app.not_running"),
            },
            HostMsg::AxAct { id, action, sig, .. } => match self.act(&action, sig.as_deref()) {
                Ok(()) => HelperMsg::AxActResult {
                    id,
                    ok: true,
                    err: None,
                },
                Err(e) if e == "guard.stopped" || e == "act.refused" => err(id, &e),
                Err(e) => HelperMsg::AxActResult {
                    id,
                    ok: false,
                    err: Some(e),
                },
            },
            HostMsg::Observe { id, arm, .. } => {
                match arm {
                    Some(true) => {
                        if !self.session {
                            self.session = true;
                            self.tripped = None;
                        }
                        self.armed = !self.no_watcher;
                    }
                    Some(false) => {
                        self.armed = false;
                        self.session = false;
                    }
                    None => {}
                }
                let focused = self.frontmost.clone().and_then(|f| self.focused.get(&f).cloned());
                HelperMsg::ObserveResult {
                    id,
                    observed: Observed {
                        frontmost: self.frontmost.clone(),
                        focused,
                        stopped: self.tripped.as_deref() == Some("stop_key"),
                        user_input: self.tripped.as_deref() == Some("user_input"),
                        armed: self.armed,
                        missing: (!self.armed).then(|| self.missing.clone()).flatten(),
                    },
                }
            }
            HostMsg::Laya { id, .. } => err(id, "laya.no_model"),
            HostMsg::LayaWarm { id, .. } => err(id, "laya.no_model"),
        }
    }
}

/// [`FakeWorld`] behind the wire.
pub struct FakeHelper(pub Mutex<FakeWorld>);

impl FakeHelper {
    pub fn new(world: FakeWorld) -> Self {
        FakeHelper(Mutex::new(world))
    }
    pub fn world(&self) -> FakeWorld {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    pub fn with<R>(&self, f: impl FnOnce(&mut FakeWorld) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}

impl Wire for FakeHelper {
    fn roundtrip(&self, line: &str) -> Result<String, String> {
        let msg: HostMsg = serde_json::from_str(line).map_err(|_| "helper.bad_request".to_string())?;
        let reply = self.0.lock().map_err(|_| "helper.poisoned")?.handle(msg);
        serde_json::to_string(&reply).map_err(|_| "helper.encode".into())
    }
}

impl<W: Wire> Wire for std::sync::Arc<W> {
    fn roundtrip(&self, line: &str) -> Result<String, String> {
        (**self).roundtrip(line)
    }
    fn up(&self) -> bool {
        (**self).up()
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::{HelperMsg, HostMsg};

    /// Every line of the shared fixture is a message of this protocol, and survives a round trip.
    #[test]
    fn the_shared_protocol_fixture_parses() {
        let raw = include_str!("../../../native/qrtz-helper/Fixtures/ctrl/protocol.jsonl");
        let (mut host, mut helper) = (0, 0);
        for line in raw.lines().filter(|l| !l.trim().is_empty()) {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let ty = v["type"].as_str().unwrap();
            let is_reply = ty.ends_with(".result") || ty == "ctrl.error";
            if is_reply {
                let m: HelperMsg = serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}"));
                let back: HelperMsg = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
                assert_eq!(m, back);
                helper += 1;
            } else {
                let m: HostMsg = serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}"));
                let back: HostMsg = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
                assert_eq!(m, back);
                host += 1;
            }
        }
        assert!(host >= 10 && helper >= 8, "{host} {helper}");
    }
}
