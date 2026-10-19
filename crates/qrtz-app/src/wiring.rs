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

/// ADR 0079 / N121: `models.warm` after a speech download (`speech.warmAfterDownload`, on by default), so the first
/// recording doesn't pay the OS model compile. N129: a warm the helper couldn't take (not running) is remembered
/// and sent on its next `hello`, then forgotten; the setting is read again at that point.
pub struct ModelsWarm {
    helper: Arc<dyn HelperPort>,
    enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    pending: std::sync::atomic::AtomicBool,
}

impl ModelsWarm {
    pub fn new(helper: Arc<dyn HelperPort>, enabled: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        ModelsWarm {
            helper,
            enabled,
            pending: Default::default(),
        }
    }

    /// Warm now, or on the next helper `hello` if the helper isn't running.
    pub fn request(&self) {
        use std::sync::atomic::Ordering;
        if !(self.enabled)() {
            return;
        }
        if !self.helper.send(&HelperCommand::ModelsWarm) {
            self.pending.store(true, Ordering::SeqCst);
            tracing::info!("models.warm: helper not running; sent when it says hello");
        }
    }

    /// A helper process said `hello`: send the warm it missed, if any.
    pub fn on_hello(&self) {
        if self.pending.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.request();
        }
    }

    /// A warm is waiting for the helper (tests).
    pub fn pending(&self) -> bool {
        self.pending.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// What the app takes from helper lines before the runtime and talk see them: `speech.*` (module audio
/// attach), note speech's lines (`asr` is consumed once speech is configured), and `hello` for a pending
/// [`ModelsWarm`] (N129; the line still reaches the runtime and talk).
pub fn helper_line_tap(
    attach: Arc<crate::speech::HelperAttach>,
    note_speech: Option<Arc<crate::speech::NoteSpeech>>,
    warm: Option<Arc<ModelsWarm>>,
) -> crate::service::SpeechTap {
    Arc::new(move |m: &Value| {
        let ty = m["type"].as_str().unwrap_or_default();
        if ty == "hello" {
            if let Some(w) = &warm {
                w.on_hello();
            }
        }
        if ty.starts_with("speech.") {
            attach.on_line(m);
            return true;
        }
        match &note_speech {
            Some(ns) => {
                ns.on_helper_line(m);
                // The module is note's only recogniser once speech is configured.
                ty == "asr"
            }
            None => false,
        }
    })
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
    let mut rc: RuntimeConfig = serde_json::from_value(note).unwrap_or_default();
    // ADR 0075: an on-device summary of a long meeting is many model calls.
    rc.summary_timeout_ms = crate::summary::ON_DEVICE_SUMMARY_TIMEOUT_MS;
    rc
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
    /// ADR 0079, tests only (never read from the environment): offer the helper's local models on any OS and fetch
    /// them from this loopback base instead of the pinned model host.
    pub local_models_base: Option<String>,
}

impl DevKnobs {
    /// `QRTZ_RETRY_SCALE`, `QRTZ_FAULT_STEP`; ignored in release builds.
    pub fn from_env() -> Self {
        Self::from_vars(cfg!(debug_assertions), |k| std::env::var(k).ok())
    }

    /// N87: in a release build every knob is ignored unless `QRTZ_ALLOW_DEV_KNOBS=1` (the E2E harness).
    pub fn from_vars(debug_build: bool, get: impl Fn(&str) -> Option<String>) -> Self {
        if !debug_build && get("QRTZ_ALLOW_DEV_KNOBS").as_deref() != Some("1") {
            return Self::default();
        }
        Self {
            retry_scale: get("QRTZ_RETRY_SCALE").and_then(|v| v.parse().ok()),
            fault_step: get("QRTZ_FAULT_STEP").and_then(|v| serde_json::from_value(Value::String(v)).ok()),
            local_models_base: None,
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
    /// The on-device summary engine (`None` when the caller passed its own `steps`). `note:delete` forgets a
    /// meeting's cached part summaries through it (N123).
    pub summaries: Option<Arc<crate::summary::OnDeviceSummaries>>,
}

/// Inputs to [`boot`].
pub struct BootOptions {
    pub data_dir: PathBuf,
    pub events: Events,
    pub shell: Arc<dyn ShellHooks>,
    pub config: SharedConfig,
    pub helper: Arc<dyn HelperPort>,
    /// `None` = `SummarySteps` → `CoreSteps` (ADR 0075: the summary on this Mac; finalize; index is a no-op).
    pub steps: Option<Arc<dyn StepExecutor>>,
    /// `None` = deferred deletes run at once (tests); the app passes `PendingDeletes::new(data dir)`.
    pub pending_delete: Option<Arc<qrtz_storage::PendingDeletes>>,
    pub clock: Option<Arc<dyn Clock>>,
    /// The on-device summary model (ADR 0075; the helper in the app). `None` = no on-device model: an on-device
    /// summary attempt fails as unavailable (never a cloud call).
    pub summarizer: Option<Arc<dyn crate::summary::OnDeviceModel>>,
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
    // ADR 0075: summaries are on this Mac only (Apple Intelligence through the helper); no provider is called.
    let (steps, summaries) = match o.steps {
        Some(s) => (s, None),
        None => {
            let mut e = crate::summary::OnDeviceSummaries::new(
                o.summarizer.unwrap_or_else(|| Arc::new(crate::summary::NoModel)),
            );
            // N123: the same tombstones `rt.pending_deletes()` lists (read here directly: the runtime owns this
            // engine, so a closure over the runtime would be a reference cycle).
            if let Some(pd) = o.pending_delete.clone() {
                e = e.with_pending_delete(Arc::new(move |id: &str| pd.has(id)));
            }
            let e = Arc::new(e);
            let steps: Arc<dyn StepExecutor> = Arc::new(crate::summary::SummarySteps {
                db: db.clone(),
                inner: Arc::new(CoreSteps { db: db.clone() }),
                now: now.clone(),
                on_device: e.clone(),
            });
            (steps, Some(e))
        }
    };
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
    Ok(Booted {
        dirs,
        db,
        runtime,
        summaries,
    })
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
    /// ADR 0146 M2.4: the monolith helper (default) or one per module (`QRTZ_MODULE_HELPERS=split`); ADR 0169:
    /// each installed module's own package (the shipping app).
    pub helper_layout: crate::module_helpers::HelperLayout,
    /// ADR 0169: `modules.<id>.installed` follows `<data>/qrtz.lock` (the shipping app: qrtz starts empty). Off,
    /// every module counts as installed (headless tests, the simulated helper).
    pub modules_from_lock: bool,
    /// The package manager (tests: a loopback or fixture index); `None` = the pinned hosts on the data dir.
    pub packages: Option<crate::module_hosting::Packages>,
    pub shell: Arc<dyn ShellHooks>,
    pub storage_shell: Arc<dyn crate::storage_port::StorageShell>,
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
    /// Settings "speech" (and switching echo or nano on in the module manager, ADR 0163): the module download. Call
    /// `setup.speech.resume_pending()` inside the tokio runtime after boot (a download a quit cut short resumes).
    pub setup: Arc<crate::setup::Setup>,
}

/// ADR 0169: what `AppService::track_installed_modules` runs at launch and after every `packages:apply`: link (or
/// drop) the helper's model views for what `<root>/qrtz.lock` holds, then say which modules are installed.
pub fn module_sync(
    root: PathBuf,
    views: Option<crate::local_models::LocalModels>,
) -> crate::service::ModuleSync {
    let lock_root = root.clone();
    crate::service::ModuleSync {
        root,
        read: Arc::new(move || {
            if let Some(v) = &views {
                for p in v.sync_views() {
                    tracing::warn!(problem = %p, "model view not linked");
                }
            }
            crate::installed::installed_modules(&crate::installed::read_lock(&lock_root))
        }),
    }
}

/// Assemble: dirs (0700) → config → DB (0600) → runtime → ports → service. Call `service.start_helper()` and
/// then `runtime.recover()` inside the tokio runtime.
pub fn build_app(b: AppBuild) -> std::io::Result<BuiltApp> {
    use crate::runtime_port::{HelperAvailability, NoteRuntimePort};
    let dirs = AppDirs::prepare(&b.data_dir)?;
    let app_version = b.info.version.clone();
    let config: SharedConfig = crate::ConfigStore::open(dirs.data.join("config.json")).into();
    let events = Events::new();
    // ADR 0079: without a speech module source (the Mac build today) the helper transcribes with its own models,
    // downloaded from settings into <data>/models and read by the helper from there only.
    let model_views =
        b.speech.is_none() && (cfg!(target_os = "macos") || b.knobs.local_models_base.is_some());
    let local_models = model_views.then(|| {
        crate::local_models::LocalModels::new(dirs.data.join("models"), b.knobs.local_models_base.clone())
    });
    let mut snap_spawn = None;
    let (supervisor, availability) = match b.helper {
        Some((mut spawn, av)) => (
            Some({
                // ADR 0092: the macOS helper draws echo's pill (no web pill window).
                if cfg!(target_os = "macos") {
                    spawn.env.push(("QRTZ_NATIVE_PILL".to_owned(), "1".to_owned()));
                }
                if let Some(l) = &local_models {
                    spawn.env.push((
                        crate::local_models::ENV.to_owned(),
                        l.root().to_string_lossy().into_owned(),
                    ));
                }
                // ADR 0159: qrtz-snap, its own process in either layout, next to the helper. The Rust mock
                // helper plays no snap role, so Linux dev runs without it.
                snap_spawn = crate::snap::spawner_for(&b.helper_layout, &spawn);
                crate::module_helpers::ModuleHelpers::spawn(&b.helper_layout, spawn, b.supervisor.clone())
            }),
            av,
        ),
        None => (None, HelperAvailability::Missing),
    };
    let helpers = supervisor;
    if helpers.as_ref().is_some_and(|h| h.is_split()) {
        tracing::info!("module helpers: qrtz-nano, qrtz-echo, qrtz-ctrl (ADR 0146 M2.4)");
    }
    // Nano's helper (the monolith in the default layout) carries recordings, meetings, summaries and `models.warm`;
    // echo's carries dictation; ctrl's carries `ctrl.*`.
    let supervisor = helpers.as_ref().map(|h| h.nano.clone());
    let echo_helper = helpers.as_ref().map(|h| h.echo.clone());
    let ctrl_helper = helpers.as_ref().map(|h| h.ctrl.clone());
    let helper: Arc<dyn HelperPort> = match &supervisor {
        Some(s) => Arc::new(SupervisorHelper(s.clone())),
        None => Arc::new(NoHelper),
    };
    // ADR 0046: the speech module's audio comes from the helper (N55); note parts get a module session each.
    let attach = crate::speech::HelperAttach::split(supervisor.clone(), echo_helper.clone());
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
    // ADR 0078: qrtz keeps no keys; delete any an older build saved (Keychain items, secrets.json).
    crate::secrets::drop_saved_keys(&dirs.data, crate::secrets::platform_keychain().as_ref());
    let booted = boot(BootOptions {
        data_dir: dirs.data.clone(),
        events: events.clone(),
        shell: b.shell.clone(),
        config: config.clone(),
        helper,
        steps: b.steps,
        pending_delete: Some(pending.clone()),
        clock: None,
        summarizer: Some(Arc::new(crate::summary::HelperModel::new(supervisor.clone()))),
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
                helper: echo_helper.clone(),
                events: events.clone(),
                shell: talk_shell,
                config: config.clone(),
                store_path: dirs.data.join("talk.json"),
                local: b.speech.as_ref().map(|s| {
                    let e = qrtz_speech::spawn::OnDeviceSpeech::new(s.store.clone(), s.opts.clone())
                        .with_attach(attach.clone());
                    Arc::new(e) as Arc<dyn qrtz_speech::SpeechEngine>
                }),
                speech_missing: crate::talk::echo_speech_missing(local_models.as_ref()),
            },
        ))),
    };
    let setup = Arc::new(crate::setup::Setup {
        speech: crate::setup::SpeechManager::with_local(b.speech.clone(), local_models, events.clone()),
    });
    // Audit P0-3: the local models download only what the switched-on extensions load (echo: Parakeet; nano:
    // Nemotron, the diarizer and LocalVQE), not every pinned set; `speech:download` may name the extensions itself.
    {
        let cfg = config.clone();
        setup.speech.select_extensions(Arc::new(move || {
            let c = cfg.get();
            ["echo", "nano"]
                .into_iter()
                .filter(|m| crate::modules::is_on(&c, m))
                .map(str::to_owned)
                .collect()
        }));
    }
    // ADR 0079 / N121: warm the just-downloaded models so the first recording doesn't spend its diarizer backlog
    // on the OS model compile; N129: a warm the helper misses is sent on its next hello.
    let warm_on = {
        let cfg = config.clone();
        Arc::new(move || cfg.get()["speech"]["warmAfterDownload"].as_bool().unwrap_or(true))
            as Arc<dyn Fn() -> bool + Send + Sync>
    };
    let warm = supervisor
        .clone()
        .map(|sup| Arc::new(ModelsWarm::new(Arc::new(SupervisorHelper(sup)), warm_on.clone())));
    // ADR 0150 N174: in split, echo's models live in echo's process, so the end of a download also warms echo
    // (qrtz-echo answers `models.warm` with its keyed prewarm); a warm echo's app misses is sent on its own hello.
    let echo_warm = helpers.as_ref().filter(|h| h.is_split()).map(|h| {
        Arc::new(ModelsWarm::new(
            Arc::new(SupervisorHelper(h.echo.clone())),
            warm_on.clone(),
        ))
    });
    if warm.is_some() || echo_warm.is_some() {
        let (n, e) = (warm.clone(), echo_warm.clone());
        setup.speech.on_installed(Arc::new(move || {
            for w in n.iter().chain(e.iter()) {
                w.request();
            }
        }));
    }
    let service = crate::AppService::new(crate::AppDeps {
        info: b.info,
        config,
        runtime: Arc::new(
            NoteRuntimePort::new(booted.runtime.clone(), availability)
                .forget_summaries_on_delete(booted.summaries.clone()),
        ),
        storage: Arc::new(storage),
        talk,
        shell: b.shell,
        helper: supervisor.clone(),
        events,
    });
    service.set_setup(setup.clone());
    // ADR 0157: the Extensions screen's package manager, on the app's data dir (nothing is reached until the user
    // asks: `packages:refresh`, or a plan that needs the network).
    if let Some(p) = b
        .packages
        .or_else(|| crate::module_hosting::Packages::pinned(Some(dirs.data.clone())))
    {
        service.set_packages(Arc::new(
            crate::packages_ipc::PackagesIpc::new(p)
                .with_events(service.events.clone())
                .with_app_version(app_version),
        ));
    }
    if b.modules_from_lock {
        let views = model_views.then(|| {
            crate::local_models::LocalModels::new(dirs.data.join("models"), b.knobs.local_models_base.clone())
        });
        service.track_installed_modules(module_sync(dirs.data.clone(), views));
    }
    if let Some(h) = &helpers {
        service.set_module_helpers(h.clone());
    }
    if let Some(sp) = snap_spawn {
        let mut o = b.supervisor.clone();
        o.name = "qrtz-snap".into();
        // qrtz-snap speaks only `snap.*` (no heartbeat line), like qrtz-ctrl.
        o.heartbeat_timeout = std::time::Duration::ZERO;
        let sup = qrtz_helper_bridge::Supervisor::spawn(o, sp);
        service.set_snap(Arc::new(crate::snap::SnapHost::new(sup)));
    }
    if let Some(w) = echo_warm {
        service.set_echo_hello_hook(Arc::new(move || w.on_hello()));
    }
    // ADR 0103: ctrl's task store and run logs, <data>/ctrl (0700, files 0600).
    let mut ctrl = qrtz_ctrl::service::CtrlService::open(
        dirs.data.join("ctrl"),
        Box::new(qrtz_ctrl::service::SystemClock::default()),
    );
    // ADR 0110: on macOS the helper process carries ctrl's `ctrl.*` protocol; runs execute through it while it is
    // up and stop at `planned` while it isn't. Elsewhere the helper has no ctrl backend, so ctrl stays plan-only.
    if let (true, Some(sup), Ok(rt)) = (
        cfg!(target_os = "macos"),
        ctrl_helper.clone(),
        tokio::runtime::Handle::try_current(),
    ) {
        let helper = Arc::new(qrtz_ctrl::helper::HelperClient::new(Box::new(
            crate::ctrl_helper::SupervisorWire::new(sup, rt),
        )));
        // ADR 0116: the Laya scorer only with the user's consent and the installed model, whose path rides on every
        // `ctrl.laya`; otherwise (and on builds without the `laya` feature) the heuristic layer answers.
        let laya = crate::ctrl_model::CtrlLayaModel::new(
            dirs.data.join("models").join("ctrl"),
            &dirs.data.join("ctrl"),
            None,
        );
        let consent = laya.consent();
        let (_, fallback) = qrtz_ctrl::laya::select_scorer(&consent, None);
        tracing::info!(?fallback, "ctrl.laya: scorer at start (re-read per question)");
        // Run 19: re-selected per question, so a download that finishes (or an off) applies without a restart.
        let scorer = qrtz_ctrl::laya::LiveScorer {
            consent: Box::new(move || laya.consent()),
            helper: helper.clone(),
            model: qrtz_ctrl::decide::ModelId {
                id: "laya".into(),
                version: crate::ctrl_model::CtrlManifest::pinned().manifest.version,
                quant: None,
            },
            runtime_build: qrtz_ctrl::laya::RUNTIME_BUILD,
        };
        // ADR 0133: warm the model on the helper once it can be used (consent + the installed model on disk +
        // the helper up and settled), from a low-priority thread of its own. The helper answers at once and loads
        // on its background queue, so no echo/nano start or ctrl question waits on it; nothing is downloaded.
        if qrtz_ctrl::laya::RUNTIME_BUILD {
            let warm_model = crate::ctrl_model::CtrlLayaModel::new(
                dirs.data.join("models").join("ctrl"),
                &dirs.data.join("ctrl"),
                None,
            );
            let warmer = qrtz_ctrl::laya::LayaWarmer::new(
                Box::new(move || warm_model.consent()),
                helper.clone(),
                true,
            )
            .with_settle(2);
            let spawned = std::thread::Builder::new()
                .name("ctrl-laya-warm".into())
                .spawn(move || loop {
                    if let Some(r) = warmer.tick() {
                        tracing::info!(?r, "ctrl.laya: warm-up asked");
                    }
                    std::thread::sleep(std::time::Duration::from_secs(5));
                });
            if spawned.is_err() {
                tracing::warn!("ctrl.laya: no warm-up thread; the first question loads the model");
            }
        }
        ctrl = ctrl.with_helper(helper).with_scorer(Box::new(scorer));
    }
    service.set_ctrl(Arc::new(ctrl));
    // ADR 0116: the model's consent and verified download, on every OS (only a macOS `laya` build runs it).
    service.set_ctrl_model(Arc::new(crate::ctrl_model::CtrlModelSlot::new(
        crate::ctrl_model::CtrlLayaModel::new(
            dirs.data.join("models").join("ctrl"),
            &dirs.data.join("ctrl"),
            None,
        ),
    )));
    service.set_speech_tap(helper_line_tap(attach.clone(), note_speech.clone(), warm));
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
    fn n87_release_builds_ignore_dev_knobs_unless_allowed() {
        let env = |allow: bool| {
            move |k: &str| match k {
                "QRTZ_RETRY_SCALE" => Some("0.01".to_string()),
                "QRTZ_ALLOW_DEV_KNOBS" if allow => Some("1".to_string()),
                _ => None,
            }
        };
        assert_eq!(DevKnobs::from_vars(false, env(false)).retry_scale, None);
        assert_eq!(DevKnobs::from_vars(false, env(true)).retry_scale, Some(0.01));
        assert_eq!(DevKnobs::from_vars(true, env(false)).retry_scale, Some(0.01));
    }
}
