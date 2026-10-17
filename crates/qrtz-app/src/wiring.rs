//! The recipe in docs/rust-core-api.md §4.2 for the Tauri build (feature "runtime"): `HelperPort` over the
//! supervisor, `EventSink` to the validated bus plus OS notifications, `Store`/`CoreSteps` over the
//! `StorageHandle`, the config reader, and `boot()` which runs `AppDirs::prepare` before `Db::open`.
use crate::dirs::AppDirs;
use crate::ports::{Notice, ShellHooks};
use crate::service::{Events, SharedConfig};
use qrtz_helper_bridge::{encode_host_message, Supervisor};
use qrtz_lifecycle::runtime::{
    utc_recording_title, Clock, CoreEvent, CoreSteps, EventSink, HelperCommand, HelperPort, NoteRuntime,
    RuntimeConfig, RuntimeDeps, StepExecutor, Store, SystemClock,
};
use qrtz_model::StepId;
use qrtz_storage::StorageHandle;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// `llmParams`: `note.llmProvider` / `note.llmModel` / `privacy.localOnly`, read per attempt; the base URL is a dev
/// knob only (it would redirect the key).
pub fn llm_config(cfg: &Value, base_url: Option<&str>) -> qrtz_llm::LlmConfig {
    qrtz_llm::LlmConfig {
        provider: cfg["note"]["llmProvider"]
            .as_str()
            .unwrap_or("openrouter")
            .to_string(),
        model: cfg["note"]["llmModel"].as_str().unwrap_or("").to_string(),
        base_url: base_url.map(String::from),
        local_only: cfg["privacy"]["localOnly"].as_bool().unwrap_or(false),
    }
}

/// `HelperPort` over the supervisor: ordered, sync enqueue; `false` when the helper isn't running.
pub struct SupervisorHelper(pub Supervisor);
impl HelperPort for SupervisorHelper {
    fn send(&self, cmd: &HelperCommand) -> bool {
        match encode_host_message(&cmd.to_json()) {
            Ok(line) => self.0.try_send(line),
            Err(e) => {
                tracing::error!(err = %e, cmd = cmd.type_name(), "invalid host message");
                false
            }
        }
    }
    fn restart(&self) {
        self.0.restart()
    }
}

/// No helper at all (missing install): every send fails like a stopped supervisor.
pub struct NoHelper;
impl HelperPort for NoHelper {
    fn send(&self, _cmd: &HelperCommand) -> bool {
        false
    }
}

/// `deps.config().note` + `privacy.excludedApps` (read on every use).
pub fn runtime_config(cfg: &Value) -> RuntimeConfig {
    let mut note = cfg["note"].clone();
    if let Value::Object(o) = &mut note {
        o.insert("excludedApps".into(), cfg["privacy"]["excludedApps"].clone());
    }
    serde_json::from_value(note).unwrap_or_default()
}

/// The runtime's events → the IPC bus; the non-IPC ones → shell notifications. Also the one place the
/// `noAudio` edge is seen (N29 notification hook, port of `NoteService.emitSession`): one notice per meeting
/// until `noAudio` clears, and only when `note.notifyNoAudio` is on.
pub struct AppSink {
    pub events: Events,
    pub shell: Arc<dyn ShellHooks>,
    pub config: SharedConfig,
    no_audio_for: Mutex<Option<String>>,
}

impl AppSink {
    pub fn new(events: Events, shell: Arc<dyn ShellHooks>, config: SharedConfig) -> Self {
        Self {
            events,
            shell,
            config,
            no_audio_for: Mutex::new(None),
        }
    }
    fn no_audio_edge(&self, view: &qrtz_model::SessionView) {
        let key = view.meeting_id.clone().filter(|_| view.no_audio);
        let mut last = self.no_audio_for.lock().unwrap_or_else(|p| p.into_inner());
        match key {
            Some(id) if last.as_deref() != Some(&id) => {
                *last = Some(id.clone());
                if self.config.get()["note"]["notifyNoAudio"].as_bool() != Some(false) {
                    self.shell.notify(Notice::NoAudio { meeting_id: id });
                }
            }
            Some(_) => {}
            None => *last = None,
        }
    }
}

impl EventSink for AppSink {
    fn emit(&self, e: CoreEvent) {
        if let CoreEvent::Session(v) = &e {
            self.no_audio_edge(v);
        }
        match &e {
            CoreEvent::DetectedPrompt { title, platform } => self.shell.notify(Notice::MeetingDetected {
                title: title.clone(),
                platform: platform.as_str().to_owned(),
            }),
            _ => {
                if let Some(name) = e.ipc_name() {
                    self.events.emit(name, e.payload());
                }
            }
        }
    }
}

/// Dev/test knobs (unpackaged builds only, like Electron's `!app.isPackaged` guards).
#[derive(Debug, Clone, Default)]
pub struct DevKnobs {
    pub retry_scale: Option<f64>,
    pub fault_step: Option<StepId>,
    /// `QRTZ_LLM_BASE_URL`: the LLM requests (with the API key) go to this host instead (tests, mock servers).
    pub llm_base_url: Option<String>,
    /// `QRTZ_TALK_CLOUD=echo|fail`: the cloud talk engines are `qrtz_talk::mock::MockCloud` (Echo or Refuse).
    pub talk_cloud: Option<String>,
}

impl DevKnobs {
    /// `QRTZ_RETRY_SCALE`, `QRTZ_FAULT_STEP`; ignored in release builds.
    pub fn from_env() -> Self {
        Self::from_vars(cfg!(debug_assertions), |k| std::env::var(k).ok())
    }

    /// N87: in a release build every knob (`QRTZ_LLM_BASE_URL` above all: it would send the API key and the
    /// transcript to another host) is ignored unless `QRTZ_ALLOW_DEV_KNOBS=1` (the E2E harness).
    pub fn from_vars(debug_build: bool, get: impl Fn(&str) -> Option<String>) -> Self {
        if !debug_build && get("QRTZ_ALLOW_DEV_KNOBS").as_deref() != Some("1") {
            return Self::default();
        }
        Self {
            retry_scale: get("QRTZ_RETRY_SCALE").and_then(|v| v.parse().ok()),
            fault_step: get("QRTZ_FAULT_STEP").and_then(|v| serde_json::from_value(Value::String(v)).ok()),
            llm_base_url: get("QRTZ_LLM_BASE_URL").filter(|v| !v.is_empty()),
            talk_cloud: get("QRTZ_TALK_CLOUD").filter(|v| v == "echo" || v == "fail"),
        }
    }
}

/// uuid v4 (lowercase, hyphenated): meeting and note ids; `qrtz-media` only serves `<uuid>.wav`.
pub fn uuid_v4() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// What `boot` opened.
pub struct Booted {
    pub dirs: AppDirs,
    pub db: StorageHandle,
    pub runtime: NoteRuntime,
}

/// Inputs to [`boot`].
pub struct BootOptions {
    pub data_dir: PathBuf,
    pub events: Events,
    pub shell: Arc<dyn ShellHooks>,
    pub config: SharedConfig,
    pub helper: Arc<dyn HelperPort>,
    /// `None` = `LlmSteps` → `CoreSteps` (summary by the configured provider; finalize; index is a no-op).
    pub steps: Option<Arc<dyn StepExecutor>>,
    /// `None` = deferred deletes run at once (tests); the app passes `PendingDeletes::new(data dir)`.
    pub pending_delete: Option<Arc<qrtz_storage::PendingDeletes>>,
    pub clock: Option<Arc<dyn Clock>>,
    /// API keys for the summary provider (`None` = on-device summaries only).
    pub secrets: Option<Arc<dyn crate::secrets::SecretStore>>,
    pub knobs: DevKnobs,
}

/// Startup order (N25/N26): owner-only dirs first, then the DB (0600 files), then the runtime. The caller runs
/// `runtime.recover()` once the helper is started (start() waits for it).
pub fn boot(o: BootOptions) -> std::io::Result<Booted> {
    let dirs = AppDirs::prepare(&o.data_dir)?;
    let db = StorageHandle::spawn(dirs.data.join("qrtz.db"))
        .map_err(|e| std::io::Error::other(format!("database: {e}")))?;
    let cfg = o.config.clone();
    let now: Arc<dyn Fn() -> i64 + Send + Sync> = Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    });
    // N83 (ADR 0042): LlmSteps (summary: the configured provider, extractive fallback) → CoreSteps (finalize;
    // the index step is a no-op: search is full-text over the notes DB, no embeddings since the chat removal).
    // Every LLM request goes through `qrtz_llm::http_client()` (no redirects, N81).
    let core: Arc<dyn StepExecutor> = Arc::new(CoreSteps { db: db.clone() });
    let llm_steps: Arc<dyn StepExecutor> = match o.secrets.clone() {
        Some(sec) => {
            let (c, base) = (cfg.clone(), o.knobs.llm_base_url.clone());
            Arc::new(qrtz_llm::LlmSteps::new(
                db.clone(),
                core,
                now.clone(),
                Arc::new(move || llm_config(&c.get(), base.as_deref())),
                Arc::new(move |n: &str| sec.get(n)),
            ))
        }
        None => Arc::new(qrtz_llm::LlmSteps::on_device(db.clone(), core, now.clone())),
    };
    let steps = o.steps.unwrap_or(llm_steps);
    let runtime = NoteRuntime::new(RuntimeDeps {
        store: Arc::new(Store(db.clone())),
        steps,
        helper: o.helper,
        events: Arc::new(AppSink::new(o.events, o.shell, o.config)),
        config: Arc::new(move || runtime_config(&cfg.get())),
        clock: o.clock.unwrap_or_else(|| Arc::new(SystemClock)),
        ids: Arc::new(uuid_v4),
        random: Arc::new(rand::random::<f64>),
        recordings_dir: dirs.recordings.clone(),
        retry_scale: o.knobs.retry_scale.unwrap_or(1.0),
        fault_step: o.knobs.fault_step,
        default_title: Arc::new(utc_recording_title),
        pending_delete: o.pending_delete,
    });
    Ok(Booted { dirs, db, runtime })
}

/// Everything the Tauri shell (and the headless tests) need to assemble the app in one call.
pub struct AppBuild {
    pub info: crate::AppInfo,
    pub data_dir: PathBuf,
    /// The helper child process (`None` = no capture backend at all → health `missing`).
    pub helper: Option<(
        qrtz_helper_bridge::ProcessSpawn,
        crate::runtime_port::HelperAvailability,
    )>,
    pub supervisor: qrtz_helper_bridge::SupervisorOptions,
    pub shell: Arc<dyn ShellHooks>,
    pub storage_shell: Arc<dyn crate::storage_port::StorageShell>,
    /// `None` = file store (non-macOS) / Keychain (macOS) under the data dir.
    pub secrets: Option<Arc<dyn crate::secrets::SecretStore>>,
    /// The speech module (store, spawn options, update server); `None` = not configured in this build.
    pub speech: Option<crate::speech::SpeechSetup>,
    /// `None` = qrtz talk over the helper (data/talk.json).
    pub talk: Option<Arc<dyn crate::ports::TalkPort>>,
    pub steps: Option<Arc<dyn StepExecutor>>,
    pub knobs: DevKnobs,
}

pub struct BuiltApp {
    pub service: Arc<crate::AppService>,
    pub runtime: NoteRuntime,
    pub db: StorageHandle,
    pub dirs: AppDirs,
    /// N55: hands speech module audio fds to the helper (the on-device engine's `AudioAttach`).
    pub attach: Arc<crate::speech::HelperAttach>,
    pub note_speech: Option<Arc<crate::speech::NoteSpeech>>,
    pub speech: Option<crate::speech::SpeechSetup>,
    /// Onboarding / settings "speech": the module download. Call
    /// `setup.speech.resume_pending()` inside the tokio runtime after boot (a download a quit cut short resumes).
    pub setup: Arc<crate::setup::Setup>,
}

/// The default secret store for this platform (env overrides in front).
pub fn platform_secrets(data: &std::path::Path) -> Arc<dyn crate::secrets::SecretStore> {
    use crate::secrets::EnvOverride;
    #[cfg(target_os = "macos")]
    {
        Arc::new(EnvOverride(crate::secrets::KeychainSecretStore::new(
            "ai.qrtz.desktop",
            data.join("secrets-index.json"),
        )))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(EnvOverride(crate::secrets::FileSecretStore::new(
            data.join("secrets.json"),
        )))
    }
}

/// Assemble: dirs (0700) → config → DB (0600) → runtime → ports → service. Call `service.start_helper()` and
/// then `runtime.recover()` inside the tokio runtime.
pub fn build_app(b: AppBuild) -> std::io::Result<BuiltApp> {
    use crate::runtime_port::{HelperAvailability, NoteRuntimePort};
    let dirs = AppDirs::prepare(&b.data_dir)?;
    let config: SharedConfig = crate::ConfigStore::open(dirs.data.join("config.json")).into();
    let events = Events::new();
    let (supervisor, availability) = match b.helper {
        Some((spawn, av)) => (
            Some(qrtz_helper_bridge::Supervisor::spawn(
                b.supervisor,
                Arc::new(spawn),
            )),
            av,
        ),
        None => (None, HelperAvailability::Missing),
    };
    let helper: Arc<dyn HelperPort> = match &supervisor {
        Some(s) => Arc::new(SupervisorHelper(s.clone())),
        None => Arc::new(NoHelper),
    };
    // ADR 0046: the speech module's audio comes from the helper (N55); note parts get a module session each.
    let attach = crate::speech::HelperAttach::new(supervisor.clone());
    let note_speech = b
        .speech
        .as_ref()
        .map(|s| crate::speech::NoteSpeech::new(s, attach.clone(), config.clone()));
    let helper: Arc<dyn HelperPort> = match &note_speech {
        Some(ns) => Arc::new(crate::speech::RoutedHelper {
            inner: helper,
            speech: ns.clone(),
        }),
        None => helper,
    };
    // One tombstone store for the runtime (undo window) and backup import (skips meetings pending delete).
    let pending = Arc::new(qrtz_storage::PendingDeletes::new(&dirs.data));
    let secrets = b.secrets.unwrap_or_else(|| platform_secrets(&dirs.data));
    let booted = boot(BootOptions {
        data_dir: dirs.data.clone(),
        events: events.clone(),
        shell: b.shell.clone(),
        config: config.clone(),
        helper,
        steps: b.steps,
        pending_delete: Some(pending.clone()),
        clock: None,
        secrets: Some(secrets.clone()),
        knobs: b.knobs.clone(),
    })?;
    if let Some(ns) = &note_speech {
        ns.set_runtime(booted.runtime.clone());
    }
    let talk_shell = b.storage_shell.clone();
    let rt = booted.runtime.clone();
    let storage = crate::storage_port::SqliteStorage {
        db: booted.db.clone(),
        events: events.clone(),
        live_meeting: Arc::new(move || rt.session().meeting_id),
        hidden: Some({
            let rt = booted.runtime.clone();
            Arc::new(move || rt.pending_deletes())
        }),
        shell: b.storage_shell.clone(),
        recordings_dir: dirs.recordings.clone(),
        pending: Some(pending.clone()),
        app_version: b.info.version.clone(),
    };
    let talk: Arc<dyn crate::ports::TalkPort> = match b.talk {
        Some(t) => t,
        None => Arc::new(crate::talk::Talk(crate::talk::talk_service(
            crate::talk::TalkWiring {
                helper: supervisor.clone(),
                events: events.clone(),
                secrets: secrets.clone(),
                shell: talk_shell,
                config: config.clone(),
                store_path: dirs.data.join("talk.json"),
                cloud: b.knobs.talk_cloud.clone(),
                local: b.speech.as_ref().map(|s| {
                    let e = qrtz_speech::spawn::OnDeviceSpeech::new(s.store.clone(), s.opts.clone())
                        .with_attach(attach.clone());
                    Arc::new(e) as Arc<dyn qrtz_speech::SpeechEngine>
                }),
            },
        ))),
    };
    let setup = Arc::new(crate::setup::Setup {
        speech: crate::setup::SpeechManager::new(b.speech.clone(), events.clone()),
    });
    let service = crate::AppService::new(crate::AppDeps {
        info: b.info,
        config,
        secrets,
        runtime: Arc::new(NoteRuntimePort::new(booted.runtime.clone(), availability)),
        storage: Arc::new(storage),
        talk,
        shell: b.shell,
        helper: supervisor,
        events,
    });
    service.set_setup(setup.clone());
    {
        let (a, ns) = (attach.clone(), note_speech.clone());
        service.set_speech_tap(Arc::new(move |m: &serde_json::Value| {
            let ty = m["type"].as_str().unwrap_or_default();
            if ty.starts_with("speech.") {
                a.on_line(m);
                return true;
            }
            match &ns {
                Some(ns) => {
                    ns.on_helper_line(m);
                    // The module is note's only recogniser once speech is configured.
                    ty == "asr"
                }
                None => false,
            }
        }));
    }
    Ok(BuiltApp {
        service,
        runtime: booted.runtime,
        db: booted.db,
        dirs: booted.dirs,
        attach,
        note_speech,
        speech: b.speech,
        setup,
    })
}

#[cfg(test)]
mod knob_tests {
    use super::DevKnobs;
    #[test]
    fn the_talk_cloud_knob_is_dev_only_and_validated() {
        let env = |allow: bool, v: &'static str| {
            move |k: &str| match k {
                "QRTZ_TALK_CLOUD" => Some(v.to_string()),
                "QRTZ_ALLOW_DEV_KNOBS" if allow => Some("1".to_string()),
                _ => None,
            }
        };
        assert_eq!(DevKnobs::from_vars(false, env(false, "echo")).talk_cloud, None);
        assert_eq!(
            DevKnobs::from_vars(false, env(true, "fail"))
                .talk_cloud
                .as_deref(),
            Some("fail")
        );
        assert_eq!(
            DevKnobs::from_vars(true, env(false, "echo"))
                .talk_cloud
                .as_deref(),
            Some("echo")
        );
        assert_eq!(DevKnobs::from_vars(true, env(false, "bogus")).talk_cloud, None);
    }
    #[test]
    fn n87_release_builds_ignore_the_llm_base_url_unless_dev_knobs_are_allowed() {
        let env = |allow: bool| {
            move |k: &str| match k {
                "QRTZ_LLM_BASE_URL" => Some("http://elsewhere.example".to_string()),
                "QRTZ_ALLOW_DEV_KNOBS" if allow => Some("1".to_string()),
                _ => None,
            }
        };
        assert_eq!(DevKnobs::from_vars(false, env(false)).llm_base_url, None);
        assert_eq!(
            DevKnobs::from_vars(false, env(true)).llm_base_url.as_deref(),
            Some("http://elsewhere.example")
        );
        assert!(DevKnobs::from_vars(true, env(false)).llm_base_url.is_some());
    }
}
