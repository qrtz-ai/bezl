//! The app service: one `Handler` for the qrtz-ipc router that owns shell-level channels and routes the
//! rest to the runtime / storage / talk ports; a validated event bus; helper supervisor wiring.
use crate::config::ConfigStore;
use crate::ports::*;
use async_trait::async_trait;
use qrtz_helper_bridge::{encode_host_message, parse_helper_line, HelperStatus, Supervisor};
use qrtz_ipc::{no_handler, validate_event, AppError, CallContext, ErrorCode, Handler};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub arch: String,
    /// Shell identifier; the contract field is still called `electron` until cutover.
    pub shell: String,
}

/// Validated event bus (EventBus.emit in TS). The Tauri layer fans events out per window subscription.
#[derive(Clone)]
pub struct Events(broadcast::Sender<(String, Value)>);

impl Events {
    pub fn new() -> Self {
        Self(broadcast::channel(1024).0)
    }
    pub fn emit(&self, event: &str, payload: Value) {
        match validate_event(event, &payload) {
            Ok(()) => {
                let _ = self.0.send((event.to_owned(), payload));
            }
            // ADR 0083: the failing paths only; the issues quote the payload (`note:transcript` is transcript text).
            Err(_) => tracing::error!(
                event,
                at = ?qrtz_ipc::validate_event_paths(event, &payload),
                "dropping invalid event"
            ),
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<(String, Value)> {
        self.0.subscribe()
    }
}
impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

/// The config store shared by the service (config:* channels) and the runtime/talk config readers.
#[derive(Clone)]
pub struct SharedConfig(pub Arc<Mutex<ConfigStore>>);
impl SharedConfig {
    pub fn get(&self) -> Value {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).get().clone()
    }
}
impl From<ConfigStore> for SharedConfig {
    fn from(c: ConfigStore) -> Self {
        SharedConfig(Arc::new(Mutex::new(c)))
    }
}

pub struct AppDeps {
    pub info: AppInfo,
    pub config: SharedConfig,
    pub runtime: Arc<dyn RuntimePort>,
    pub storage: Arc<dyn StoragePort>,
    pub talk: Arc<dyn TalkPort>,
    pub shell: Arc<dyn ShellHooks>,
    pub helper: Option<Supervisor>,
    /// The bus the runtime's EventSink also writes to (build it first, hand clones to both).
    pub events: Events,
}

pub struct AppService {
    info: AppInfo,
    config: SharedConfig,
    pub runtime: Arc<dyn RuntimePort>,
    pub storage: Arc<dyn StoragePort>,
    pub talk: Arc<dyn TalkPort>,
    shell: Arc<dyn ShellHooks>,
    helper: Option<Supervisor>,
    pub events: Events,
    setup: std::sync::OnceLock<Arc<dyn crate::ports::SetupPort>>,
    packages: std::sync::OnceLock<Arc<dyn crate::ports::PackagesPort>>,
    /// ADR 0181: qrtz's own updater (`app-update:*`).
    self_update: std::sync::OnceLock<Arc<crate::self_update::SelfUpdate>>,
    ctrl: std::sync::OnceLock<Arc<qrtz_ctrl::service::CtrlService>>,
    /// ADR 0116: ctrl's Laya model (consent + download), behind `ctrl:model` / `ctrl:model-consent`.
    #[cfg(feature = "runtime")]
    ctrl_model: std::sync::OnceLock<Arc<crate::ctrl_model::CtrlModelSlot>>,
    speech_tap: std::sync::OnceLock<SpeechTap>,
    /// ADR 0146 M2.4: the per-module helpers (`QRTZ_MODULE_HELPERS=split`); unset = the one `helper`.
    module_helpers: std::sync::OnceLock<crate::module_helpers::ModuleHelpers>,
    /// Split: whether each module's helper was last started (true) or stopped (false) (run 31; ADR 0169).
    helpers_on: Mutex<std::collections::BTreeMap<&'static str, bool>>,
    /// ADR 0169: reads `qrtz.lock` (and links the installed models' views) and returns the `modules.<id>.installed`
    /// patch. Set once by the app (`track_installed_modules`); unset = every module counts as installed.
    module_sync: std::sync::OnceLock<ModuleSync>,
    /// ADR 0172: which running helpers an update left on an old version (see [`Relaunch`]).
    relaunch: Relaunch,
    /// ADR 0159: qrtz-snap (its own process in either layout), set once before `start_helper`.
    snap: std::sync::OnceLock<Arc<crate::snap::SnapHost>>,
    /// Split: called on every hello from echo's process (ADR 0150 N174: echo's own pending `models.warm`).
    echo_hello: std::sync::OnceLock<Arc<dyn Fn() + Send + Sync>>,
    /// The last helper status every port has seen (`on_helper_status` finished), for readiness checks.
    delivered: std::sync::Mutex<Option<HelperStatus>>,
    /// `log:write` rate limit (ADR 0083).
    renderer_log: Mutex<crate::logging::RendererGate>,
    /// The log folder `logs:export` reads (`set_log_dir`; default: the one `logging::init` opened).
    log_dir: std::sync::OnceLock<std::path::PathBuf>,
    /// Where `logs:export {diagnostics}` looks for macOS crash reports (tests; default
    /// `crate::diagnostics::default_report_dirs`).
    report_dirs: std::sync::OnceLock<Vec<std::path::PathBuf>>,
}

/// ADR 0169: the host's view of what is installed: `read` links the installed models' views and returns the
/// `modules.<id>.installed` patch with each installed module's locked version (see [`crate::installed`]); `root`
/// is the data dir the lock and `packages/` are under.
#[derive(Clone)]
pub struct ModuleSync {
    pub root: std::path::PathBuf,
    pub read: Arc<dyn Fn() -> crate::installed::InstalledModules + Send + Sync>,
}

/// ADR 0172 (v1, amended): an update of an installed module (its locked version changes while `installed` stays
/// true) doesn't relaunch its helper. The running process keeps its old version until qrtz restarts: the module
/// is `pendingRestart` (the marketplace says "restart to finish updating"), and its old package dir is kept out of
/// every prune (`packages-in-use.json`) while that process lives. The next launch starts the new version, rewrites
/// the in-use set (empty) and prunes the old dir.
#[derive(Default)]
struct Relaunch {
    /// The locked version of each installed module, as of the last sync.
    locked: Mutex<std::collections::BTreeMap<String, String>>,
    /// The version each running module helper was started from (absent: not running, or not from the lock).
    started: Mutex<std::collections::BTreeMap<&'static str, String>>,
    /// The in-use dirs last written (`None`: not written yet this launch).
    in_use: Mutex<Option<std::collections::BTreeSet<String>>>,
    /// A previous launch left dirs in use: they are pruned once at this launch's `start_helper`.
    prune_due: std::sync::atomic::AtomicBool,
}

/// Sees every valid helper line first; `true` = consumed (not routed to talk or the runtime).
pub type SpeechTap = Arc<dyn Fn(&Value) -> bool + Send + Sync>;

/// Helper → host message types that belong to qrtz talk; everything else goes to the recording runtime.
pub fn is_talk_message(ty: &str) -> bool {
    ty == "hotkey" || ty.starts_with("dictation.") || ty.starts_with("insert.")
}

impl AppService {
    pub fn new(d: AppDeps) -> Arc<Self> {
        Arc::new(Self {
            info: d.info,
            config: d.config,
            runtime: d.runtime,
            storage: d.storage,
            talk: d.talk,
            shell: d.shell,
            helper: d.helper,
            events: d.events,
            setup: std::sync::OnceLock::new(),
            packages: std::sync::OnceLock::new(),
            self_update: std::sync::OnceLock::new(),
            ctrl: std::sync::OnceLock::new(),
            #[cfg(feature = "runtime")]
            ctrl_model: std::sync::OnceLock::new(),
            speech_tap: std::sync::OnceLock::new(),
            module_helpers: std::sync::OnceLock::new(),
            helpers_on: Mutex::default(),
            module_sync: std::sync::OnceLock::new(),
            relaunch: Relaunch::default(),
            snap: std::sync::OnceLock::new(),
            echo_hello: std::sync::OnceLock::new(),
            delivered: Default::default(),
            renderer_log: Mutex::new(crate::logging::RendererGate::new()),
            log_dir: std::sync::OnceLock::new(),
            report_dirs: std::sync::OnceLock::new(),
        })
    }
    /// Where `logs:export {diagnostics}` reads crash reports from (tests; the app uses macOS's own folders).
    pub fn set_crash_report_dirs(&self, dirs: Vec<std::path::PathBuf>) {
        let _ = self.report_dirs.set(dirs);
    }
    /// Where `logs:export` reads the logs from (tests; the app uses the folder `logging::init` opened).
    pub fn set_log_dir(&self, dir: std::path::PathBuf) {
        let _ = self.log_dir.set(dir);
    }
    fn log_dir(&self) -> Option<std::path::PathBuf> {
        self.log_dir
            .get()
            .cloned()
            .or_else(|| crate::logging::dir().map(std::path::Path::to_path_buf))
    }

    /// `logs:export`: save one text file of every log (oldest first) where the user picks, or reveal the folder.
    /// `diagnostics` (audit P1-3): the same file plus macOS's crash reports for qrtz and its helpers. Local only.
    async fn export_logs(&self, reveal: bool, diagnostics: bool) -> Result<Value, AppError> {
        let dir = self.log_dir().ok_or_else(|| {
            AppError::new(ErrorCode::Unavailable, "no log folder in this build").retryable(false)
        })?;
        crate::logging::flush();
        if reveal {
            let shell = self.shell.clone();
            let d = dir.clone();
            let ok = tokio::task::spawn_blocking(move || shell.reveal(&d))
                .await
                .unwrap_or(false);
            if !ok {
                return Err(AppError::new(
                    ErrorCode::Unavailable,
                    "couldn't open the log folder",
                ));
            }
            let files = crate::logging::log_files(&dir).len();
            return Ok(json!({"revealed": true, "files": files, "bytes": 0}));
        }
        let shell = self.shell.clone();
        let header = format!(
            "qrtz {} logs · {}/{} · {} · {}\n",
            self.info.version,
            self.info.platform,
            self.info.arch,
            self.info.shell,
            crate::logging::os_version()
        );
        if diagnostics {
            let report_dirs = self
                .report_dirs
                .get()
                .cloned()
                .unwrap_or_else(crate::diagnostics::default_report_dirs);
            let name = format!("qrtz-diagnostics-{}.txt", self.info.version);
            return tokio::task::spawn_blocking(move || {
                let Some(dest) = shell.logs_save_path(&name) else {
                    return Ok(json!({"files": 0, "bytes": 0, "crashReports": 0}));
                };
                let e = crate::diagnostics::export_to(&dir, &report_dirs, &dest, &header).map_err(|e| {
                    AppError::new(
                        ErrorCode::Internal,
                        format!("couldn't save the diagnostics ({})", e.kind()),
                    )
                })?;
                tracing::info!(
                    files = e.logs,
                    crash_reports = e.crash_reports,
                    bytes = e.bytes,
                    "diagnostics exported"
                );
                Ok(
                    json!({"savedTo": dest.display().to_string(), "files": e.logs, "bytes": e.bytes,
                          "crashReports": e.crash_reports}),
                )
            })
            .await
            .map_err(|_| AppError::new(ErrorCode::Internal, "diagnostics export failed"))?;
        }
        let name = format!("qrtz-logs-{}.log", self.info.version);
        tokio::task::spawn_blocking(move || {
            let Some(dest) = shell.logs_save_path(&name) else {
                return Ok(json!({"files": 0, "bytes": 0}));
            };
            let (files, bytes) = crate::logging::export_to(&dir, &dest, &header).map_err(|e| {
                AppError::new(
                    ErrorCode::Internal,
                    format!("couldn't save the logs ({})", e.kind()),
                )
            })?;
            tracing::info!(files, bytes, "logs exported");
            Ok(json!({"savedTo": dest.display().to_string(), "files": files, "bytes": bytes}))
        })
        .await
        .map_err(|_| AppError::new(ErrorCode::Internal, "log export failed"))?
    }
    /// Speech routing sees every helper line first (`speech.attached` / `speech.attach.rejected`, N55; the
    /// part boundaries for note speech; the helper's own `asr` lines are dropped once the module transcribes).
    pub fn set_speech_tap(&self, tap: SpeechTap) {
        let _ = self.speech_tap.set(tap);
    }
    /// Plug in speech setup (set once by `build_app`; without it the status channels answer their idle
    /// samples and the actions are `E_UNAVAILABLE`).
    pub fn set_setup(&self, setup: Arc<dyn crate::ports::SetupPort>) {
        let _ = self.setup.set(setup);
    }

    /// Plug in the package manager (set once by `build_app`; without it `packages:list` answers its idle sample and
    /// the rest are `E_UNAVAILABLE`).
    pub fn set_packages(&self, p: Arc<dyn crate::ports::PackagesPort>) {
        let _ = self.packages.set(p);
    }

    /// ADR 0181: plug in qrtz's own updater (set once by `build_app`; without it `app-update:status` answers
    /// "not set up").
    pub fn set_self_update(&self, u: Arc<crate::self_update::SelfUpdate>) {
        let _ = self.self_update.set(u);
    }

    /// ADR 0181: the quiet update checks (a minute after launch, then every 6 hours). Call once, inside tokio.
    pub fn start_app_update_checks(&self) {
        if let Some(u) = self.self_update.get() {
            u.start_schedule();
        }
    }

    async fn app_update_channel(&self, channel: &str) -> Result<Value, AppError> {
        let Some(u) = self.self_update.get() else {
            return match channel {
                "app-update:status" | "app-update:check" => Ok(crate::self_update::to_json(
                    &self.info.version,
                    &crate::self_update::State::NotSetUp,
                    None,
                )),
                "app-update:restart" => Ok(json!({"restarting": false})),
                _ => Err(
                    AppError::new(ErrorCode::Unavailable, "updates aren't set up in this build")
                        .retryable(false),
                ),
            };
        };
        if channel == "app-update:restart" {
            // ADR 0172's pattern: only once the new version is in place, and only when the user asks.
            return Ok(json!({"restarting": u.ready() && self.shell.restart()}));
        }
        u.call(channel).await
    }

    /// Plug in ctrl (set once by `build_app`; without it every `ctrl:*` call is `E_UNAVAILABLE`).
    pub fn set_ctrl(&self, ctrl: Arc<qrtz_ctrl::service::CtrlService>) {
        let _ = self.ctrl.set(ctrl);
    }

    /// ADR 0116: plug in ctrl's Laya model slot (set once by `build_app`).
    #[cfg(feature = "runtime")]
    pub fn set_ctrl_model(&self, m: Arc<crate::ctrl_model::CtrlModelSlot>) {
        let _ = self.ctrl_model.set(m);
    }

    /// ADR 0105: in-process ctrl scheduler. Call once inside tokio (next to `start_helper`). Ticks every second
    /// and once immediately so a catch-up after sleep/quit and `once` ASAP both fire without a window.
    pub fn start_ctrl_scheduler(self: &Arc<Self>) {
        let Some(ctrl) = self.ctrl.get().cloned() else {
            return;
        };
        let events = self.events.clone();
        let shell = self.shell.clone();
        let config = self.config.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                // ADR 0146: ctrl switched off pauses its cadences; nothing runs until it's back on.
                let c = ctrl.clone();
                let cfg = config.get();
                let fired =
                    match tokio::task::spawn_blocking(move || crate::modules::ctrl_scheduler_pass(&cfg, &c))
                        .await
                    {
                        Ok(None) => continue,
                        Ok(Some(Ok(ids))) => ids,
                        Ok(Some(Err(e))) => {
                            tracing::error!(err = %e.message, "ctrl scheduler tick");
                            continue;
                        }
                        Err(_) => {
                            tracing::error!("ctrl scheduler tick join failed");
                            continue;
                        }
                    };
                if !fired.is_empty() {
                    events.emit("ctrl:changed", serde_json::json!({ "ids": fired }));
                }
                Self::notify_ctrl_confirms(&ctrl, shell.as_ref());
            }
        });
    }

    /// ADR 0111: a confirm gate that started waiting raises one non-activating notification.
    fn notify_ctrl_confirms(ctrl: &qrtz_ctrl::service::CtrlService, shell: &dyn ShellHooks) {
        for (task_id, run_id) in ctrl.take_confirm_notices() {
            shell.notify(crate::ports::Notice::CtrlConfirm { task_id, run_id });
        }
    }

    /// `ctrl:*`: the store writes and `fsync`s, so off the async runtime.
    async fn ctrl_channel(&self, channel: &str, p: Value) -> Result<Value, AppError> {
        use qrtz_ctrl::service::CtrlErrorCode as C;
        #[cfg(feature = "runtime")]
        if let Some(m) = self
            .ctrl_model
            .get()
            .filter(|_| channel.starts_with("ctrl:model"))
        {
            let st = match channel {
                "ctrl:model" => m.state(),
                "ctrl:model-consent" => {
                    let yes = p.get("consent").and_then(Value::as_bool).ok_or_else(|| {
                        AppError::new(ErrorCode::Validation, "consent must be true or false")
                    })?;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    m.set_consent(yes, now).await.map_err(|e| {
                        tracing::error!(err = %e.kind(), "ctrl.model consent");
                        AppError::new(ErrorCode::Internal, "could not save the model consent")
                    })?
                }
                _ => return Err(AppError::new(ErrorCode::NotFound, "no such ctrl channel")),
            };
            return serde_json::to_value(st)
                .map_err(|_| AppError::new(ErrorCode::Internal, "ctrl model state"));
        }
        let Some(ctrl) = self.ctrl.get().cloned() else {
            return Err(
                AppError::new(ErrorCode::Unavailable, "ctrl is not available in this build").retryable(false),
            );
        };
        let ch = channel.to_owned();
        let c = ctrl.clone();
        let r = tokio::task::spawn_blocking(move || c.call(&ch, p))
            .await
            .map_err(|_| AppError::new(ErrorCode::Internal, "ctrl call failed"))?;
        Self::notify_ctrl_confirms(&ctrl, self.shell.as_ref());
        r.map_err(|e| {
            let code = match e.code {
                C::Validation => ErrorCode::Validation,
                C::NotFound => ErrorCode::NotFound,
                C::Unavailable => ErrorCode::Unavailable,
                C::Internal => ErrorCode::Internal,
            };
            if code == ErrorCode::Internal {
                // ADR 0083: the message names a file and a line, never task text.
                tracing::error!(channel, err = %e.message, "ctrl");
            }
            AppError::new(code, e.message)
        })
    }

    pub fn config(&self) -> Value {
        self.config.get()
    }
    pub fn shared_config(&self) -> SharedConfig {
        self.config.clone()
    }

    pub fn helper(&self) -> Option<&Supervisor> {
        self.helper.as_ref()
    }

    /// ADR 0146 M2.4: run with the per-module helpers (set once, before `start_helper`). A monolith set is ignored.
    pub fn set_module_helpers(&self, h: crate::module_helpers::ModuleHelpers) {
        if h.is_split() {
            let _ = self.module_helpers.set(h);
        }
    }

    /// Quit: the goodbye to every helper process (the monolith, or nano, echo and ctrl), then kill after `grace`.
    pub async fn shutdown_helpers(&self, grace: std::time::Duration) {
        let bye = encode_host_message(&json!({"type": "shutdown"})).unwrap_or_default();
        if let Some(s) = self.snap.get() {
            s.sup.shutdown(bye.clone(), grace).await;
        }
        if let Some(m) = self.module_helpers.get() {
            m.shutdown(bye, grace).await;
        } else if let Some(h) = &self.helper {
            h.shutdown(bye, grace).await;
        }
    }

    /// Stop every helper process at once (no goodbye).
    pub fn stop_helpers(&self) {
        if let Some(s) = self.snap.get() {
            s.sup.stop();
        }
        if let Some(m) = self.module_helpers.get() {
            for (_, s) in m.processes() {
                s.stop();
            }
        } else if let Some(h) = &self.helper {
            h.stop();
        }
    }

    /// The split helpers, when the host runs them.
    pub fn module_helpers(&self) -> Option<&crate::module_helpers::ModuleHelpers> {
        self.module_helpers.get()
    }

    /// Split: `f` runs on every hello from echo's process (set once, before `start_helper`).
    pub fn set_echo_hello_hook(&self, f: Arc<dyn Fn() + Send + Sync>) {
        let _ = self.echo_hello.set(f);
    }

    /// ADR 0159: run qrtz-snap (set once, before `start_helper`).
    pub fn set_snap(&self, s: Arc<crate::snap::SnapHost>) {
        let _ = self.snap.set(s);
    }

    /// qrtz-snap runs while snap is on and gets snap's settings when they change (ADR 0159).
    fn apply_snap(&self, cfg: &Value) {
        if let Some(s) = self.snap.get() {
            let on = crate::modules::is_on(cfg, crate::snap::MODULE_ID);
            let was = self
                .relaunch
                .started
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(crate::snap::MODULE_ID);
            if s.apply(cfg) {
                self.events.emit("snap:changed", s.status());
            }
            // ADR 0172: the version qrtz-snap was started from (apply starts it when it turns on).
            match (was, on) {
                (false, true) => self.note_started(crate::snap::MODULE_ID),
                (true, false) => self.note_stopped(crate::snap::MODULE_ID),
                _ => {}
            }
            self.refresh_in_use();
        }
    }

    /// qrtz-snap's lines and status: its reports reach snap's view, its menu's "disable for <app>" the config.
    fn pump_snap(self: &Arc<Self>, s: Arc<crate::snap::SnapHost>) {
        let mut lines = s.sup.subscribe_lines();
        let me = self.clone();
        let s2 = s.clone();
        tokio::spawn(async move {
            loop {
                match lines.recv().await {
                    Ok(line) => match s2.on_line(&line, &me.config.get()) {
                        crate::snap::SnapLine::Status => {
                            let mut st = s2.status();
                            if crate::tcc::ax_trusted() {
                                st["axTrusted"] = json!(true);
                            } else if st["running"].as_bool() == Some(true) {
                                // snap is on and can't move windows: qrtz-app asks macOS once per launch, which
                                // adds qrtz to the Accessibility list and shows its own prompt (snap's view has
                                // the button to the pane too).
                                crate::tcc::ask_accessibility_once();
                            }
                            me.events.emit("snap:changed", st)
                        }
                        crate::snap::SnapLine::Patch(p) => {
                            if let Err(e) = me.set_config(&p) {
                                tracing::warn!(error = %e, "snap: disable-app not saved");
                            }
                        }
                        crate::snap::SnapLine::None => {}
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
        });
        let mut status = s.sup.watch_status();
        let me = self.clone();
        tokio::spawn(async move {
            let mut prev = None;
            loop {
                let st = status.borrow_and_update().state;
                if prev != Some(st) && s.on_state(st, &me.config.get()) {
                    me.events.emit("snap:changed", s.status());
                }
                prev = Some(st);
                if status.changed().await.is_err() {
                    break;
                }
            }
        });
        self.apply_snap(&self.config.get());
    }

    /// Start the helper and pump its lines/status into the ports and the event bus. Call once, inside tokio.
    /// Split: qrtz-ctrl only starts while ctrl is on (run 31); `config:set` starts or stops it after.
    pub fn start_helper(self: &Arc<Self>) {
        // ADR 0172: what a previous launch's helpers kept from an update goes now.
        self.prune_at_launch();
        if let Some(s) = self.snap.get().cloned() {
            self.pump_snap(s);
        }
        if let Some(h) = self.module_helpers.get().cloned() {
            for (src, sup) in h.processes() {
                self.pump(src, sup, false);
            }
            self.apply_module_switches(&self.config.get());
            return;
        }
        let Some(sup) = self.helper.clone() else { return };
        self.pump(crate::module_helpers::HelperSource::Monolith, sup, true);
    }

    /// Split: qrtz-ctrl runs exactly while `modules.ctrl` (and the global switch) is on (run 31); qrtz-nano and
    /// qrtz-echo run while their module is installed (ADR 0169; their switches still gate their channels, as
    /// before). Acts on a change only, so an unrelated `config:set` never relaunches a helper whose circuit opened.
    /// qrtz-snap (either layout) runs exactly while snap is installed and on ([`crate::snap::SnapHost::apply`]): a
    /// snap that isn't installed is never spawned, so its supervisor never retries a launch that can't work
    /// (Bozniak's 902bcab review, design call (e)).
    fn apply_module_switches(&self, cfg: &Value) {
        self.apply_snap(cfg);
        let Some(h) = self.module_helpers.get() else {
            return;
        };
        let g = qrtz_modules::settings::GlobalSettings::from_config(cfg);
        let mut last = self.helpers_on.lock().unwrap_or_else(|p| p.into_inner());
        for (id, sup) in [("nano", &h.nano), ("echo", &h.echo), ("ctrl", &h.ctrl)] {
            let on = if id == "ctrl" {
                g.is_on(id)
            } else {
                g.is_installed(id)
            };
            if last.get(id) == Some(&on) {
                continue;
            }
            last.insert(id, on);
            if on {
                self.note_started(id);
                sup.start();
            } else {
                tracing::info!(
                    module = id,
                    "module off or not installed: its helper is not running"
                );
                self.note_stopped(id);
                sup.stop();
            }
        }
        drop(last);
        self.refresh_in_use();
    }

    /// ADR 0172: module `id`'s helper is (re)started now: from its locked version (none outside the lock).
    fn note_started(&self, id: &'static str) {
        let v = self
            .relaunch
            .locked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned();
        let mut started = self.relaunch.started.lock().unwrap_or_else(|p| p.into_inner());
        match v {
            Some(v) => started.insert(id, v),
            None => started.remove(id),
        };
    }

    fn note_stopped(&self, id: &'static str) {
        self.relaunch
            .started
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
    }

    /// Running helpers started from a version the lock no longer resolves: (module, running, locked).
    fn stale_helpers(&self) -> Vec<(&'static str, String, String)> {
        let locked = self
            .relaunch
            .locked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let started = self.relaunch.started.lock().unwrap_or_else(|p| p.into_inner());
        started
            .iter()
            .filter_map(|(id, old)| {
                let new = locked.get(*id)?;
                (new != old).then(|| (*id, old.clone(), new.clone()))
            })
            .collect()
    }

    /// Keep every stale helper's package dir out of every prune (`packages-in-use.json`; rewritten on a change).
    fn refresh_in_use(&self) {
        let Some(sync) = self.module_sync.get() else {
            return;
        };
        let dirs: std::collections::BTreeSet<String> = self
            .stale_helpers()
            .into_iter()
            .map(|(id, old, _)| qrtz_modules::qrtz_lock::package_dir(id, &old))
            .collect();
        let mut last = self.relaunch.in_use.lock().unwrap_or_else(|p| p.into_inner());
        if last.as_ref() == Some(&dirs) {
            return;
        }
        if last.is_none() && !qrtz_modules::install::read_in_use(&sync.root).is_empty() {
            self.relaunch
                .prune_due
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        match qrtz_modules::install::write_in_use(&sync.root, &dirs) {
            Ok(()) => *last = Some(dirs),
            Err(e) => tracing::warn!(error = %e, "in-use package dirs not saved"),
        }
    }

    /// ADR 0172: the modules an update left running an old version until qrtz restarts (sorted).
    pub fn pending_restart(&self) -> Vec<String> {
        self.stale_helpers()
            .into_iter()
            .map(|(id, _, _)| id.to_owned())
            .collect()
    }

    /// ADR 0172: at launch the in-use set starts empty (no helper of this launch runs an old version yet), so the
    /// dirs a previous launch's helpers held go now (only when it left some, so no prune races a first apply).
    /// `start_helper` runs it (once, inside tokio).
    fn prune_at_launch(self: &Arc<Self>) {
        if !self
            .relaunch
            .prune_due
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            if let Some(p) = me.packages.get() {
                if let Err(e) = p.call("packages:prune", Value::Null).await {
                    tracing::warn!(error = %e, "prune at launch failed");
                }
            }
        });
    }

    /// ADR 0169: from now on, `modules.<id>.installed` follows `qrtz.lock`: synced now (call before
    /// `start_helper`) and after every `packages:apply`, which also prunes what the lock no longer needs (an
    /// uninstalled module's models, unless another installed module uses them).
    pub fn track_installed_modules(&self, sync: ModuleSync) {
        if self.module_sync.set(sync).is_ok() {
            self.sync_installed();
        }
    }

    /// Re-read what is installed and apply it (helpers start or stop, views appear or go). No-op when untracked.
    /// ADR 0172: an installed module whose locked version changed is `pendingRestart` (its helper keeps running).
    pub fn sync_installed(&self) {
        let Some(sync) = self.module_sync.get() else {
            return;
        };
        let now = (sync.read)();
        *self.relaunch.locked.lock().unwrap_or_else(|p| p.into_inner()) = now.versions;
        let patch = now.patch;
        let cur = self.config.get();
        let changed = patch["modules"].as_object().is_some_and(|m| {
            m.iter()
                .any(|(id, v)| cur["modules"][id]["installed"] != v["installed"])
        });
        if changed {
            if let Err(e) = self.set_config(&patch) {
                tracing::warn!(error = %e, "installed modules not saved");
            }
        }
        self.refresh_in_use();
    }

    pub fn tracks_installed_modules(&self) -> bool {
        self.module_sync.get().is_some()
    }

    fn pump(self: &Arc<Self>, src: crate::module_helpers::HelperSource, sup: Supervisor, start: bool) {
        use crate::module_helpers::HelperSource;
        // qrtz-ctrl's lines are ctrl's own (its wire subscribes to them); only its status matters here.
        if src != HelperSource::Ctrl {
            let mut lines = sup.subscribe_lines();
            let me = self.clone();
            tokio::spawn(async move {
                loop {
                    match lines.recv().await {
                        Ok(line) => me.on_helper_line_from(src, &line),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(n, ?src, "helper lines lagged")
                        }
                        Err(_) => break,
                    }
                }
            });
        }
        let mut status = sup.watch_status();
        let me = self.clone();
        let sup2 = sup.clone();
        tokio::spawn(async move {
            let mut prev = None;
            loop {
                let s = status.borrow_and_update().clone();
                me.on_helper_status_from(src, &s);
                // Every (re)start of the helper: ask for permissions again (the new process may differ).
                if src != HelperSource::Ctrl
                    && s.state == qrtz_helper_bridge::HelperState::Running
                    && prev != Some(s.state)
                {
                    let sup3 = sup2.clone();
                    tokio::spawn(async move {
                        let _ = sup3
                            .send(encode_host_message(&json!({"type": "permissions.check"})).unwrap())
                            .await;
                    });
                }
                prev = Some(s.state);
                if status.changed().await.is_err() {
                    break;
                }
            }
        });
        if start {
            sup.start();
        }
    }

    pub fn on_helper_line(&self, line: &str) {
        self.on_helper_line_from(crate::module_helpers::HelperSource::Monolith, line)
    }

    /// A helper line from `src`. In the split layout (ADR 0146 M2.4) echo's process feeds talk only and nano's
    /// feeds the runtime only, so one module's `hello` or revocation never resets the other.
    pub fn on_helper_line_from(&self, src: crate::module_helpers::HelperSource, line: &str) {
        match parse_helper_line(line) {
            Ok(msg) => {
                let ty = msg["type"].as_str().unwrap_or_default();
                // ADR 0146: meeting detection is nano's; with nano switched off nothing prompts.
                if crate::modules::drops_helper_event(&self.config.get(), ty) {
                    return;
                }
                if ty == "log" {
                    crate::logging::helper_log(
                        msg["level"].as_str().unwrap_or("info"),
                        msg["msg"].as_str().unwrap_or_default(),
                    );
                }
                if ty == "capture.fallback" {
                    // H8: structured capture fallback (was a free-text log line); same surface: a warn log.
                    tracing::warn!(from = %msg["from"], to = %msg["to"], reason = %msg["reason"], "capture fallback");
                }
                // The extensions share qrtz's own TCC grants (each helper is spawned by the host, which stays its
                // responsible process), so in split nano's report is the app's; echo's says the same and isn't
                // re-emitted.
                if ty == "permissions.status" && src.serves_nano() {
                    self.events.emit("permissions:changed", msg["status"].clone());
                }
                let (echo, nano) = (src.serves_echo(), src.serves_nano());
                // Echo's process only reaches the tap with its own speech attach answers.
                let tap = nano || ty.starts_with("speech.");
                if tap && self.speech_tap.get().is_some_and(|t| t(&msg)) {
                    // consumed by speech routing
                } else if is_talk_message(ty) {
                    if echo {
                        self.talk.helper_line(&msg);
                    }
                } else if ty != "heartbeat" {
                    // Talk also needs its own errors (dictation.busy / dictation.mic) and mic revocations; the
                    // runtime still sees them (it logs foreign codes and owns the capture side of a revocation).
                    let code = msg["code"].as_str().unwrap_or_default();
                    if echo
                        && ((ty == "error" && code.starts_with("dictation.")) || ty == "permission.revoked")
                    {
                        self.talk.helper_line(&msg);
                    }
                    // A new helper process concerns both apps: the runtime re-configures, talk ends a dictation the
                    // old process owned (T2).
                    if echo && ty == "hello" {
                        self.talk.helper_line(&msg);
                    }
                    if src == crate::module_helpers::HelperSource::Echo && ty == "hello" {
                        if let Some(f) = self.echo_hello.get() {
                            f();
                        }
                    }
                    if nano {
                        self.runtime.helper_line(&msg);
                    }
                }
            }
            // ADR 0110: ctrl's replies ride the same stdio but not the echo/nano schema; its wire takes them.
            Err(_) if crate::ctrl_helper::ctrl_line_id(line).is_some() => {}
            // ADR 0083: never the line itself (an `asr` or `dictation.*` line carries what was said).
            Err(e) => tracing::warn!(err = %e, bytes = line.len(), "bad helper line"),
        }
    }

    pub fn on_helper_status(&self, s: &HelperStatus) {
        self.on_helper_status_from(crate::module_helpers::HelperSource::Monolith, s)
    }

    /// Split: nano's status goes to the runtime, echo's to talk; the UI's `helper:status` is all of them together.
    pub fn on_helper_status_from(&self, src: crate::module_helpers::HelperSource, s: &HelperStatus) {
        if src.serves_nano() {
            self.runtime.helper_status(s);
        }
        if src.serves_echo() {
            self.talk.helper_status(s);
        }
        // PROPOSAL (echo updates on its own): once echo runs, say whether it is the bundled fallback (B5).
        if src == crate::module_helpers::HelperSource::Echo
            && s.state == qrtz_helper_bridge::HelperState::Running
        {
            if let Some(patch) = crate::echo_updates::last_fallback()
                .and_then(|f| crate::echo_updates::bundled_fallback_patch(&self.config.get(), f))
            {
                if let Err(e) = self.set_config(&patch) {
                    tracing::warn!(error = %e, "echo bundledFallback not saved");
                }
            }
        }
        let shown = match (src, self.module_helpers.get()) {
            (crate::module_helpers::HelperSource::Monolith, _) | (_, None) => s.clone(),
            (_, Some(h)) => h.status(),
        };
        self.events
            .emit("helper:status", serde_json::to_value(&shown).unwrap());
        *self.delivered.lock().unwrap_or_else(|p| p.into_inner()) = Some(shown);
    }

    /// N91: the helper is running and the runtime and talk have both been told so (the runtime's initial
    /// `simulated` health is not readiness: the first `Stopped` → `Starting` → `Running` statuses come after it).
    pub fn helper_ready(&self) -> bool {
        self.delivered
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|s| s.state == qrtz_helper_bridge::HelperState::Running)
    }

    fn set_config(&self, patch: &Value) -> Result<Value, AppError> {
        let next = self
            .config
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .set(patch)?;
        self.events.emit("config:changed", next.clone());
        self.talk.config_changed();
        self.apply_module_switches(&next);
        Ok(next)
    }

    async fn app_channel(&self, channel: &str, p: Value, sub_app: &str) -> Result<Value, AppError> {
        match channel {
            "app:get-info" => Ok(json!({
                "name": self.info.name, "version": self.info.version, "platform": self.info.platform,
                "arch": self.info.arch, "electron": self.info.shell,
            })),
            "app:licenses" => Ok(json!({ "text": crate::THIRD_PARTY_NOTICES })),
            "config:get" => Ok(self.config()),
            // ADR 0169: what is installed is the host's to say (from qrtz.lock), never a renderer's.
            "config:set" if crate::installed::patch_touches_installed(&p["patch"]) => Err(AppError::new(
                ErrorCode::Validation,
                "modules are installed from the marketplace, not through config",
            )),
            // ADR 0181: whether echo runs the copy in qrtz.app is the host's to say, never a renderer's.
            "config:set" if crate::installed::patch_touches_bundled_fallback(&p["patch"]) => {
                Err(AppError::new(
                    ErrorCode::Validation,
                    "bundledFallback is set by qrtz, not through config",
                ))
            }
            "config:set" => self.set_config(&p["patch"]),
            // ADR 0146 M1.4: the caller's module comes from its view, so a page can only reach its own settings.
            "module:get-settings" => {
                let m = crate::modules::settings_module_of(sub_app)?;
                Ok(crate::modules::pick_settings(&self.config(), m))
            }
            "module:set-settings" => {
                let m = crate::modules::settings_module_of(sub_app)?;
                crate::modules::check_settings_patch(m, &p["patch"])?;
                let next = self.set_config(&p["patch"])?;
                Ok(crate::modules::pick_settings(&next, m))
            }
            // ADR 0161: the module manager switches one extension on or off, and nothing else in config.
            "packages:set-enabled" => {
                let name = p["name"].as_str().unwrap_or_default();
                if !qrtz_modules::EXTENSION_NAMES.contains(&name) {
                    return Err(AppError::new(ErrorCode::Validation, "not an extension"));
                }
                let on = p["enabled"].as_bool().unwrap_or(true);
                // ADR 0169: a module that isn't installed has nothing to switch on.
                if on
                    && !qrtz_modules::settings::GlobalSettings::from_config(&self.config()).is_installed(name)
                {
                    return Err(AppError::new(
                        ErrorCode::Validation,
                        format!("{name} isn't installed: install it from the marketplace first"),
                    )
                    .retryable(false));
                }
                // Settings no longer has the "all modules" switch: turning one on while it is off leaves only that
                // one on (the others keep their own state as off), so nothing else starts behind the user's back.
                let all = self.config()["modules"]["enabled"].as_bool().unwrap_or(true);
                let mut patch = json!({name: {"enabled": on}});
                if on && !all {
                    patch["enabled"] = json!(true);
                    for other in qrtz_modules::EXTENSION_NAMES.iter().filter(|o| **o != name) {
                        patch[*other] = json!({"enabled": false});
                    }
                }
                let cfg = self.set_config(&json!({ "modules": patch }))?;
                Ok(json!({"name": name, "enabled": crate::modules::is_on(&cfg, name)}))
            }
            "note:set-auto-record" => {
                let on = p["enabled"].as_bool().unwrap_or(false);
                let cfg = self.set_config(&json!({"note": {"autoRecord": on}}))?;
                Ok(json!({"autoRecord": cfg["note"]["autoRecord"]}))
            }
            "helper:status" => Ok(match (&self.helper, self.module_helpers.get()) {
                (_, Some(m)) => serde_json::to_value(m.status()).unwrap(),
                (Some(h), None) => serde_json::to_value(h.status()).unwrap(),
                (None, None) => json!({"state": "stopped", "restarts": 0}),
            }),
            "log:write" => {
                let accepted = crate::logging::renderer_log(
                    &self.renderer_log,
                    p["level"].as_str().unwrap_or("info"),
                    p["scope"].as_str().unwrap_or_default(),
                    p["message"].as_str().unwrap_or_default(),
                );
                Ok(json!({"accepted": accepted}))
            }
            "logs:export" => {
                self.export_logs(
                    p["reveal"].as_bool().unwrap_or(false),
                    p["diagnostics"].as_bool().unwrap_or(false),
                )
                .await
            }
            // Every permission prompt's button: System Settings at that permission's pane (never a hunt).
            "permissions:open-settings" => {
                let pane = p["pane"].as_str().unwrap_or_default();
                if crate::tcc::settings_url(pane).is_none() {
                    return Err(AppError::new(ErrorCode::Validation, "unknown settings pane"));
                }
                Ok(
                    json!({"opened": crate::tcc::open_settings(pane), "restart": crate::tcc::needs_restart(pane)}),
                )
            }
            // "restart qrtz", for a permission a running process only sees after a relaunch.
            "app:restart" => Ok(json!({"restarting": self.shell.restart()})),
            "hud:set-expanded" => {
                let e = self
                    .shell
                    .set_hud_expanded(p["expanded"].as_bool().unwrap_or(false));
                Ok(json!({"expanded": e}))
            }
            _ => Err(no_handler(channel)),
        }
    }

    /// `permissions:get` / `permissions:request`: the runtime's (helper's) answer with qrtz-app's own live reads on
    /// top. Accessibility and screen recording are asked from this process, so macOS lists qrtz itself; the
    /// microphone stays with the helper (AVFoundation). Every answer is re-read, never cached.
    async fn permissions_channel(&self, channel: &str, p: Value) -> Result<Value, AppError> {
        let native = crate::tcc::native_status();
        let base = || qrtz_ipc::idle_response("permissions:get").unwrap_or(Value::Null);
        let mut v = if channel == "permissions:request" {
            let perm = p["permission"].as_str().unwrap_or_default();
            if native.iter().any(|(k, _)| *k == perm) {
                crate::tcc::request(perm);
                self.runtime
                    .call("permissions:get", Value::Null)
                    .await
                    .unwrap_or_else(|_| base())
            } else {
                self.runtime.call(channel, p).await?
            }
        } else {
            self.runtime.call(channel, p).await.unwrap_or_else(|_| base())
        };
        crate::tcc::overlay(&mut v, &native);
        Ok(v)
    }

    /// Which port owns a channel (None = unrouted).
    pub fn owner(channel: &str) -> Option<&'static str> {
        if RUNTIME_CHANNELS.contains(&channel) {
            Some("runtime")
        } else if STORAGE_CHANNELS.contains(&channel) {
            Some("storage")
        } else if APP_CHANNELS.contains(&channel) {
            Some("app")
        } else if SETUP_CHANNELS.contains(&channel) {
            Some("setup")
        } else if channel.starts_with(TALK_PREFIX) {
            Some("talk")
        } else if channel.starts_with(CTRL_PREFIX) {
            Some("ctrl")
        } else if SNAP_CHANNELS.contains(&channel) {
            Some("snap")
        } else if channel.starts_with(PACKAGES_PREFIX) {
            Some("packages")
        } else if channel.starts_with(APP_UPDATE_PREFIX) {
            Some("app-update")
        } else {
            None
        }
    }
}

#[async_trait]
impl Handler for AppService {
    async fn handle(&self, channel: &str, payload: Value, ctx: &CallContext) -> Result<Value, AppError> {
        // ADR 0146: a switched-off module's views and channels are refused (config stays reachable).
        let cfg = self.config.get();
        crate::modules::gate(&cfg, &ctx.sub_app, channel)?;
        crate::modules::check_view_config_patch(&ctx.sub_app, channel, &payload)?;
        // ADR 0175 (N181): a module view's speech calls name exactly its own models.
        crate::modules::check_view_speech_call(&cfg, &ctx.sub_app, channel, &payload)?;
        match Self::owner(channel) {
            Some("runtime") if channel.starts_with("permissions:") => {
                self.permissions_channel(channel, payload).await
            }
            Some("runtime") => self.runtime.call(channel, payload).await,
            Some("storage") => self.storage.call(channel, payload).await,
            Some("talk") => self.talk.call(channel, payload).await,
            Some("app") => self.app_channel(channel, payload, &ctx.sub_app).await,
            Some("ctrl") => self.ctrl_channel(channel, payload).await,
            Some("app-update") => self.app_update_channel(channel).await,
            Some("snap") => {
                if channel == "snap:request-access" {
                    // The banner's button: qrtz-app asks (macOS adds qrtz to the list and prompts) and opens the
                    // Accessibility pane; snap's view polls snap:status until it's on.
                    let opened = crate::tcc::request("accessibility");
                    return Ok(json!({"opened": opened, "axTrusted": crate::tcc::ax_trusted()}));
                }
                let mut st = self.snap.get().map_or_else(
                    || json!({"running": false, "axTrusted": false, "registered": {}, "refused": {}}),
                    |s| s.status(),
                );
                // qrtz-snap reports trust when it (re)configures; the grant is qrtz-app's (its responsible
                // process), so the live read here wins over a report from before the user switched qrtz on.
                if st["running"].as_bool() == Some(true) && crate::tcc::ax_trusted() {
                    st["axTrusted"] = json!(true);
                }
                Ok(st)
            }
            Some("setup") => match self.setup.get() {
                Some(s) => s.call(channel, payload).await,
                None => qrtz_ipc::idle_response(channel).ok_or_else(|| {
                    AppError::new(
                        ErrorCode::Unavailable,
                        "speech setup is not available in this build",
                    )
                    .retryable(false)
                }),
            },
            Some("packages") => match self.packages.get() {
                Some(p) => {
                    let mut r = p.call(channel, payload).await;
                    // ADR 0169: an install or removal changes what runs. The old dirs and blobs nothing needs any
                    // more go too (a removed module's models, unless another installed module shares them).
                    if r.is_ok() && channel == "packages:apply" && self.tracks_installed_modules() {
                        self.sync_installed();
                        if let Err(e) = p.call("packages:prune", Value::Null).await {
                            tracing::warn!(error = %e, "prune after apply failed");
                        }
                    }
                    // ADR 0172: the marketplace's "restart to finish updating" (the modules an update left running
                    // their old version), on every packages state.
                    if let Ok(v) = &mut r {
                        if v.get("packages").is_some() && v.get("available").is_some() {
                            v["pendingRestart"] = json!(self.pending_restart());
                        }
                    }
                    r
                }
                None => qrtz_ipc::idle_response(channel).ok_or_else(|| {
                    AppError::new(
                        ErrorCode::Unavailable,
                        "the package manager is not available in this build",
                    )
                    .retryable(false)
                }),
            },
            _ => Err(no_handler(channel)),
        }
    }
}

/// Router over a shared service (Handler for Arc<AppService>).
pub struct Shared(pub Arc<AppService>);
#[async_trait]
impl Handler for Shared {
    async fn handle(&self, channel: &str, payload: Value, ctx: &CallContext) -> Result<Value, AppError> {
        self.0.handle(channel, payload, ctx).await
    }
}
