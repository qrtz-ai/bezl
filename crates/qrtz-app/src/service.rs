//! The app service: one `Handler` for the qrtz-ipc router that owns shell-level channels and routes the
//! rest to the runtime / storage / talk ports; a validated event bus; helper supervisor wiring.
use crate::config::ConfigStore;
use crate::ports::*;
use crate::secrets::SecretStore;
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
            Err(e) => tracing::error!(event, err = ?e, "dropping invalid event"),
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
    pub secrets: Arc<dyn SecretStore>,
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
    secrets: Arc<dyn SecretStore>,
    pub runtime: Arc<dyn RuntimePort>,
    pub storage: Arc<dyn StoragePort>,
    pub talk: Arc<dyn TalkPort>,
    shell: Arc<dyn ShellHooks>,
    helper: Option<Supervisor>,
    pub events: Events,
    setup: std::sync::OnceLock<Arc<dyn crate::ports::SetupPort>>,
    speech_tap: std::sync::OnceLock<SpeechTap>,
    /// The last helper status every port has seen (`on_helper_status` finished), for readiness checks.
    delivered: std::sync::Mutex<Option<HelperStatus>>,
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
            secrets: d.secrets,
            runtime: d.runtime,
            storage: d.storage,
            talk: d.talk,
            shell: d.shell,
            helper: d.helper,
            events: d.events,
            setup: std::sync::OnceLock::new(),
            speech_tap: std::sync::OnceLock::new(),
            delivered: Default::default(),
        })
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

    pub fn config(&self) -> Value {
        self.config.get()
    }
    pub fn shared_config(&self) -> SharedConfig {
        self.config.clone()
    }

    pub fn helper(&self) -> Option<&Supervisor> {
        self.helper.as_ref()
    }

    /// Start the helper and pump its lines/status into the ports and the event bus. Call once, inside tokio.
    pub fn start_helper(self: &Arc<Self>) {
        let Some(sup) = self.helper.clone() else { return };
        let mut lines = sup.subscribe_lines();
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                match lines.recv().await {
                    Ok(line) => me.on_helper_line(&line),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(n, "helper lines lagged")
                    }
                    Err(_) => break,
                }
            }
        });
        let mut status = sup.watch_status();
        let me = self.clone();
        let sup2 = sup.clone();
        tokio::spawn(async move {
            let mut prev = None;
            loop {
                let s = status.borrow_and_update().clone();
                me.on_helper_status(&s);
                // Every (re)start of the helper: ask for permissions again (the new process may differ).
                if s.state == qrtz_helper_bridge::HelperState::Running && prev != Some(s.state) {
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
        sup.start();
    }

    pub fn on_helper_line(&self, line: &str) {
        match parse_helper_line(line) {
            Ok(msg) => {
                let ty = msg["type"].as_str().unwrap_or_default();
                if ty == "capture.fallback" {
                    // H8: structured capture fallback (was a free-text log line); same surface: a warn log.
                    tracing::warn!(from = %msg["from"], to = %msg["to"], reason = %msg["reason"], "capture fallback");
                }
                if ty == "permissions.status" {
                    self.events.emit("permissions:changed", msg["status"].clone());
                }
                if self.speech_tap.get().is_some_and(|t| t(&msg)) {
                    // consumed by speech routing
                } else if is_talk_message(ty) {
                    self.talk.helper_line(&msg);
                } else if ty != "heartbeat" {
                    // Talk also needs its own errors (dictation.busy / dictation.mic) and mic revocations; the
                    // runtime still sees them (it logs foreign codes and owns the capture side of a revocation).
                    let code = msg["code"].as_str().unwrap_or_default();
                    if (ty == "error" && code.starts_with("dictation.")) || ty == "permission.revoked" {
                        self.talk.helper_line(&msg);
                    }
                    // A new helper process concerns both apps: the runtime re-configures, talk ends a dictation the
                    // old process owned (T2).
                    if ty == "hello" {
                        self.talk.helper_line(&msg);
                    }
                    self.runtime.helper_line(&msg);
                }
            }
            Err(e) => {
                tracing::warn!(err = %e, line = %line.chars().take(200).collect::<String>(), "bad helper line")
            }
        }
    }

    pub fn on_helper_status(&self, s: &HelperStatus) {
        self.runtime.helper_status(s);
        self.talk.helper_status(s);
        self.events
            .emit("helper:status", serde_json::to_value(s).unwrap());
        *self.delivered.lock().unwrap_or_else(|p| p.into_inner()) = Some(s.clone());
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
        Ok(next)
    }

    async fn app_channel(&self, channel: &str, p: Value) -> Result<Value, AppError> {
        match channel {
            "app:get-info" => Ok(json!({
                "name": self.info.name, "version": self.info.version, "platform": self.info.platform,
                "arch": self.info.arch, "electron": self.info.shell,
            })),
            "config:get" => Ok(self.config()),
            "config:set" => self.set_config(&p["patch"]),
            "note:set-auto-record" => {
                let on = p["enabled"].as_bool().unwrap_or(false);
                let cfg = self.set_config(&json!({"note": {"autoRecord": on}}))?;
                Ok(json!({"autoRecord": cfg["note"]["autoRecord"]}))
            }
            "secrets:set" => {
                let name = p["name"].as_str().unwrap_or_default();
                // ADR 0076: no cloud keys are stored any more (a removal of a lingering one is still allowed).
                if p["value"].is_string() && crate::secrets::RETIRED_SECRETS.contains(&name) {
                    return Err(AppError::new(
                        ErrorCode::Validation,
                        format!("{name} is no longer used: qrtz runs entirely on this computer"),
                    ));
                }
                self.secrets.set(name, p["value"].as_str())?;
                Ok(json!({"ok": true}))
            }
            "secrets:list" => Ok(json!(self.secrets.list())),
            "helper:status" => Ok(match &self.helper {
                Some(h) => serde_json::to_value(h.status()).unwrap(),
                None => json!({"state": "stopped", "restarts": 0}),
            }),
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
        } else {
            None
        }
    }
}

#[async_trait]
impl Handler for AppService {
    async fn handle(&self, channel: &str, payload: Value, _ctx: &CallContext) -> Result<Value, AppError> {
        match Self::owner(channel) {
            Some("runtime") => self.runtime.call(channel, payload).await,
            Some("storage") => self.storage.call(channel, payload).await,
            Some("talk") => self.talk.call(channel, payload).await,
            Some("app") => self.app_channel(channel, payload).await,
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
