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
    ctrl: std::sync::OnceLock<Arc<qrtz_ctrl::service::CtrlService>>,
    /// ADR 0116: ctrl's Laya model (consent + download), behind `ctrl:model` / `ctrl:model-consent`.
    #[cfg(feature = "runtime")]
    ctrl_model: std::sync::OnceLock<Arc<crate::ctrl_model::CtrlModelSlot>>,
    speech_tap: std::sync::OnceLock<SpeechTap>,
    /// ADR 0146 M2.4: the per-module helpers (`QRTZ_MODULE_HELPERS=split`); unset = the one `helper`.
    module_helpers: std::sync::OnceLock<crate::module_helpers::ModuleHelpers>,
    /// Split: whether qrtz-ctrl was last started (true) or stopped (false) for `modules.ctrl` (run 31).
    ctrl_on: Mutex<Option<bool>>,
    /// Split: called on every hello from echo's process (ADR 0150 N174: echo's own pending `models.warm`).
    echo_hello: std::sync::OnceLock<Arc<dyn Fn() + Send + Sync>>,
    /// The last helper status every port has seen (`on_helper_status` finished), for readiness checks.
    delivered: std::sync::Mutex<Option<HelperStatus>>,
    /// `log:write` rate limit (ADR 0083).
    renderer_log: Mutex<crate::logging::RendererGate>,
    /// The log folder `logs:export` reads (`set_log_dir`; default: the one `logging::init` opened).
    log_dir: std::sync::OnceLock<std::path::PathBuf>,
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
            ctrl: std::sync::OnceLock::new(),
            #[cfg(feature = "runtime")]
            ctrl_model: std::sync::OnceLock::new(),
            speech_tap: std::sync::OnceLock::new(),
            module_helpers: std::sync::OnceLock::new(),
            ctrl_on: Mutex::default(),
            echo_hello: std::sync::OnceLock::new(),
            delivered: Default::default(),
            renderer_log: Mutex::new(crate::logging::RendererGate::new()),
            log_dir: std::sync::OnceLock::new(),
        })
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
    async fn export_logs(&self, reveal: bool) -> Result<Value, AppError> {
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
        if let Some(m) = self.module_helpers.get() {
            m.shutdown(bye, grace).await;
        } else if let Some(h) = &self.helper {
            h.shutdown(bye, grace).await;
        }
    }

    /// Stop every helper process at once (no goodbye).
    pub fn stop_helpers(&self) {
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

    /// Start the helper and pump its lines/status into the ports and the event bus. Call once, inside tokio.
    /// Split: qrtz-ctrl only starts while ctrl is on (run 31); `config:set` starts or stops it after.
    pub fn start_helper(self: &Arc<Self>) {
        if let Some(h) = self.module_helpers.get().cloned() {
            for (src, sup) in h.processes() {
                let start = src != crate::module_helpers::HelperSource::Ctrl;
                self.pump(src, sup, start);
            }
            self.apply_ctrl_switch(&self.config.get());
            return;
        }
        let Some(sup) = self.helper.clone() else { return };
        self.pump(crate::module_helpers::HelperSource::Monolith, sup, true);
    }

    /// Split: qrtz-ctrl runs exactly while `modules.ctrl` (and the global switch) is on. Acts on a change only, so
    /// an unrelated `config:set` never relaunches a ctrl whose circuit opened.
    fn apply_ctrl_switch(&self, cfg: &Value) {
        let Some(h) = self.module_helpers.get() else {
            return;
        };
        let on = crate::modules::is_on(cfg, "ctrl");
        let mut last = self.ctrl_on.lock().unwrap_or_else(|p| p.into_inner());
        if *last == Some(on) {
            return;
        }
        *last = Some(on);
        if on {
            h.ctrl.start();
        } else {
            tracing::info!("ctrl is off: qrtz-ctrl not running");
            h.ctrl.stop();
        }
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
        self.apply_ctrl_switch(&next);
        Ok(next)
    }

    async fn app_channel(&self, channel: &str, p: Value, sub_app: &str) -> Result<Value, AppError> {
        match channel {
            "app:get-info" => Ok(json!({
                "name": self.info.name, "version": self.info.version, "platform": self.info.platform,
                "arch": self.info.arch, "electron": self.info.shell,
            })),
            "config:get" => Ok(self.config()),
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
            "logs:export" => self.export_logs(p["reveal"].as_bool().unwrap_or(false)).await,
            "hud:set-expanded" => {
                let e = self
                    .shell
                    .set_hud_expanded(p["expanded"].as_bool().unwrap_or(false));
                Ok(json!({"expanded": e}))
            }
            _ => Err(no_handler(channel)),
        }
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
        } else if channel.starts_with(PACKAGES_PREFIX) {
            Some("packages")
        } else {
            None
        }
    }
}

#[async_trait]
impl Handler for AppService {
    async fn handle(&self, channel: &str, payload: Value, ctx: &CallContext) -> Result<Value, AppError> {
        // ADR 0146: a switched-off module's views and channels are refused (config stays reachable).
        crate::modules::gate(&self.config.get(), &ctx.sub_app, channel)?;
        match Self::owner(channel) {
            Some("runtime") => self.runtime.call(channel, payload).await,
            Some("storage") => self.storage.call(channel, payload).await,
            Some("talk") => self.talk.call(channel, payload).await,
            Some("app") => self.app_channel(channel, payload, &ctx.sub_app).await,
            Some("ctrl") => self.ctrl_channel(channel, payload).await,
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
                Some(p) => p.call(channel, payload).await,
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
