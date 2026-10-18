//! Process-wide state for the Tauri shell: the app service + router, per-window event subscriptions, and
//! the shell's reactions to events (tray, HUD, pill, login item, global shortcuts, OS notifications).
use crate::{paths, policy, tray, windows};
use qrtz_app::ports::{Notice, ShellHooks};
use qrtz_app::storage_port::StorageShell;
use qrtz_app::wiring::{build_app, AppBuild, DevKnobs};
use qrtz_app::{AppInfo, AppService, Shared};
use qrtz_helper_bridge::SupervisorOptions;
use qrtz_ipc::{Router, Subscriptions};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tauri::{AppHandle, Emitter, EventTarget, Manager};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_clipboard_manager::ClipboardExt as _;
use tauri_plugin_dialog::DialogExt as _;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_notification::NotificationExt as _;

pub const EVENT: &str = "qrtz:event";

/// Window effects + OS notifications. The AppHandle lands right after the service is built.
#[derive(Default)]
pub struct TauriShell(OnceLock<AppHandle>);

fn any_window_focused(app: &AppHandle) -> bool {
    app.webview_windows()
        .values()
        .any(|w| w.is_focused().unwrap_or(false))
}

impl ShellHooks for TauriShell {
    fn set_hud_expanded(&self, expanded: bool) -> bool {
        match self.0.get() {
            Some(app) => windows::set_hud_expanded(app, expanded),
            None => expanded,
        }
    }
    /// The in-app UI covers a focused window; otherwise tell the OS (Electron `onNoAudio` / `onDetectedPrompt`).
    fn notify(&self, n: Notice) {
        let Some(app) = self.0.get() else { return };
        let (title, body) = match &n {
            Notice::NoAudio { .. } => (
                crate::strings::UI_NO_AUDIO_TITLE.to_string(),
                crate::strings::UI_NO_AUDIO_BODY.to_string(),
            ),
            Notice::MeetingDetected { title, platform } => (
                crate::strings::UI_MEETING_DETECTED.to_string(),
                format!(
                    "record “{}”? open qrtz to start recording.",
                    if title.is_empty() { platform } else { title }
                ),
            ),
        };
        // e2e: every notice the core raised, focused or not (dev builds only).
        if cfg!(debug_assertions) {
            if let Some(p) = std::env::var_os("QRTZ_NOTICE_LOG") {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                    let _ = writeln!(f, "{}", json!({"title": title, "body": body}));
                }
            }
        }
        if any_window_focused(app) {
            return;
        }
        if let Err(e) = app.notification().builder().title(&title).body(&body).show() {
            tracing::warn!(err = %e, "notification failed");
        }
    }
    /// ADR 0083: Settings → export logs. The e2e export dir (debug builds only) skips the dialog.
    fn logs_save_path(&self, default_name: &str) -> Option<PathBuf> {
        if cfg!(debug_assertions) {
            if let Some(d) = std::env::var_os("QRTZ_EXPORT_DIR") {
                return Some(PathBuf::from(d).join(default_name));
            }
        }
        let app = self.0.get()?;
        let mut d = app
            .dialog()
            .file()
            .set_file_name(default_name)
            .add_filter("log", &["log", "txt"]);
        if let Ok(docs) = app.path().document_dir() {
            d = d.set_directory(docs);
        }
        d.blocking_save_file().and_then(|p| p.into_path().ok())
    }
    /// ADR 0083: Settings → show logs: Finder with qrtz.log selected (the folder elsewhere). No URL is ever opened.
    fn reveal(&self, dir: &std::path::Path) -> bool {
        crate::paths::reveal(dir)
    }
}

/// Clipboard + save dialog for `note:export`.
pub struct TauriStorageShell(OnceLock<AppHandle>);
impl StorageShell for TauriStorageShell {
    fn write_clipboard(&self, text: &str) {
        if let Some(app) = self.0.get() {
            if let Err(e) = app.clipboard().write_text(text) {
                tracing::warn!(err = %e, "clipboard write failed");
            }
        }
    }
    fn save_path(&self, filename: &str) -> Option<PathBuf> {
        if let Some(d) = std::env::var_os("QRTZ_EXPORT_DIR") {
            return Some(PathBuf::from(d).join(filename));
        }
        let app = self.0.get()?;
        let mut d = app
            .dialog()
            .file()
            .set_file_name(filename)
            .add_filter("markdown", &["md"]);
        if let Ok(docs) = app.path().document_dir() {
            d = d.set_directory(docs);
        }
        d.blocking_save_file().and_then(|p| p.into_path().ok())
    }
    fn backup_save_path(&self, default_name: &str) -> Option<PathBuf> {
        let app = self.0.get()?;
        let mut d = app
            .dialog()
            .file()
            .set_file_name(default_name)
            .add_filter("qrtz backup", &["zip"]);
        if let Ok(docs) = app.path().document_dir() {
            d = d.set_directory(docs);
        }
        d.blocking_save_file().and_then(|p| p.into_path().ok())
    }
    fn backup_open_path(&self) -> Option<PathBuf> {
        let app = self.0.get()?;
        app.dialog()
            .file()
            .add_filter("qrtz backup", &["zip"])
            .blocking_pick_file()
            .and_then(|p| p.into_path().ok())
    }
}

#[derive(Default)]
struct Linger {
    gen: AtomicU64,
    hiding: AtomicBool,
}

/// The pill's error flash (`qrtz_app::pill`): which failures were shown (`PillGate`), and whether a flash is on
/// screen now.
#[derive(Default)]
struct Flash {
    gate: Mutex<qrtz_app::pill::PillGate>,
    on: AtomicBool,
}

pub struct Backend {
    pub service: Arc<AppService>,
    /// The app's database (playback asks it which meeting owns a recording, N140).
    pub db: qrtz_storage::StorageHandle,
    pub router: Router<Shared>,
    pub subs: Mutex<Subscriptions>,
    started: Instant,
    ready: AtomicBool,
    session: Mutex<Value>,
    hud: Linger,
    pill: Linger,
    flash: Flash,
    hotkeys: Mutex<Vec<(String, &'static str)>>,
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        o => o,
    }
}
fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        a => a,
    }
}

impl Backend {
    /// The main window finished booting (first subscription). Logged once; QRTZ_PERF=1 also prints it.
    pub fn mark_ready(&self) {
        if self.ready.swap(true, Ordering::SeqCst) {
            return;
        }
        let ms = self.started.elapsed().as_millis();
        tracing::info!(ready_ms = ms as u64, "main window ready");
        if std::env::var("QRTZ_PERF").as_deref() == Ok("1") {
            println!("QRTZ_PERF ready_ms={ms}");
        }
    }
}

/// One `QRTZ_SPEECH_*` value (the module URL and its signing and root keys). ADR 0078: only a debug build reads
/// them from the environment; a release build uses what was baked in at compile time, and nothing else, even with
/// `QRTZ_ALLOW_DEV_KNOBS=1` (that switch is for the E2E harness's test knobs, never for where code comes from or
/// which keys it trusts).
fn speech_var(k: &str, debug_build: bool, env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let run = if debug_build { env(k) } else { None };
    run.or_else(|| baked_speech_var(k).map(str::to_owned))
}

fn baked_speech_var(k: &str) -> Option<&'static str> {
    match k {
        "QRTZ_SPEECH_URL" => option_env!("QRTZ_SPEECH_URL"),
        "QRTZ_SPEECH_KEY_ID" => option_env!("QRTZ_SPEECH_KEY_ID"),
        "QRTZ_SPEECH_KEY" => option_env!("QRTZ_SPEECH_KEY"),
        "QRTZ_SPEECH_ROOT_KEY_ID" => option_env!("QRTZ_SPEECH_ROOT_KEY_ID"),
        "QRTZ_SPEECH_ROOT_KEY" => option_env!("QRTZ_SPEECH_ROOT_KEY"),
        _ => None,
    }
}

pub fn setup(app: &mut tauri::App, started: Instant) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let data = paths::data_dir(&handle);
    tracing::info!(data = %data.display(), "data dir");

    // Spawning tasks (supervisor, storage, runtime timers) needs the tokio runtime Tauri owns.
    let rt = tauri::async_runtime::handle();
    let _enter = rt.inner().enter();

    let helper = paths::helper_spawn();
    match &helper {
        Some((sp, av)) => tracing::info!(helper = %sp.program.display(), availability = ?av, "helper"),
        None => tracing::warn!("no capture helper found; recording is unavailable"),
    }
    let shell = Arc::new(TauriShell::default());
    let _ = shell.0.set(handle.clone());
    let storage_shell = Arc::new(TauriStorageShell(OnceLock::new()));
    let _ = storage_shell.0.set(handle.clone());

    let version = app.package_info().version.to_string();
    // The speech module source is baked in at compile time; only a debug build may point it elsewhere at run
    // time (the mock bundle server). ADR 0078: never a release build, whatever the environment says.
    let speech = qrtz_app::speech::SpeechSetup::from_vars(&data, &version, |k| {
        speech_var(k, cfg!(debug_assertions), |k| std::env::var(k).ok())
    })
    .unwrap_or_else(|e| {
        tracing::error!(err = %e, "speech module configuration");
        None
    });
    if speech.is_none() {
        tracing::warn!("no speech module configured in this build; transcripts use the helper's recogniser");
    }
    let built = build_app(AppBuild {
        info: AppInfo {
            name: "qrtz".into(),
            version,
            platform: platform().into(),
            arch: arch().into(),
            shell: format!("tauri {}", tauri::VERSION),
        },
        data_dir: data,
        helper,
        supervisor: SupervisorOptions::default(),
        shell,
        storage_shell,
        talk: None,
        speech,
        steps: None,
        knobs: DevKnobs::from_env(),
    })?;
    let service = built.service.clone();
    let router = Router::new(Shared(service.clone())).validate_responses(cfg!(debug_assertions));
    app.manage(Backend {
        service: service.clone(),
        db: built.db.clone(),
        router,
        subs: Mutex::default(),
        started,
        ready: AtomicBool::new(false),
        session: Mutex::new(json!({"phase": "idle", "helper": "ok", "elapsedMs": 0})),
        hud: Linger::default(),
        pill: Linger::default(),
        flash: Flash::default(),
        hotkeys: Mutex::default(),
    });

    pump_events(&handle);
    service.start_helper();
    // A speech download a quit cut short resumes in the background (not one the user paused).
    {
        let setup = built.setup.clone();
        tauri::async_runtime::spawn(async move { setup.speech.resume_pending() });
    }
    // Startup recovery (R1–R3 + audio retention): finish what a crash left behind. start() waits for it.
    let runtime = built.runtime.clone();
    let svc = service.clone();
    tauri::async_runtime::spawn(async move {
        match runtime.recover().await {
            Ok(r) => tracing::info!(report = ?r, "startup recovery"),
            Err(e) => tracing::warn!(err = %e.message, "startup recovery skipped"),
        }
        svc.events.emit("note:changed", json!({"id": "*"}));
    });

    windows::create_main(&handle)?;
    // Menu bar item (macOS/Windows; Linux tray support varies by desktop, opt in with QRTZ_TRAY=1, like Electron).
    if !cfg!(target_os = "linux") || std::env::var("QRTZ_TRAY").as_deref() == Ok("1") {
        if let Err(e) = tray::create(&handle) {
            tracing::warn!(err = %e, "tray unavailable");
        }
    }
    apply_config(&handle, &service.config(), true);
    if std::env::var("QRTZ_SHOW_HUD").as_deref() == Ok("1") {
        windows::show_hud(&handle);
    }
    Ok(())
}

/// Service events → subscribed windows (`qrtz:event`, only to sub apps that subscribed and hold the
/// capability) and → shell reactions.
fn pump_events(app: &AppHandle) {
    let app = app.clone();
    let app2 = app.clone();
    let mut rx = app.state::<Backend>().service.events.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            let (event, payload) = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "event pump lagged");
                    continue;
                }
                Err(_) => break,
            };
            let b = app.state::<Backend>();
            let targets = b.subs.lock().unwrap().fanout(&event);
            for (label, sub_apps) in targets {
                let msg = json!({"event": event, "payload": payload, "subApps": sub_apps});
                if let Err(e) = app.emit_to(EventTarget::webview_window(label.clone()), EVENT, msg) {
                    tracing::warn!(label, err = %e, "emit failed");
                }
            }
            match event.as_str() {
                "note:session" => on_session(&app, payload),
                "talk:state" => on_talk(&app, &payload),
                "config:changed" => apply_config(&app, &payload, false),
                _ => {}
            }
        }
    });
    // Tray elapsed clock while recording.
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            tick.tick().await;
            let s = app2.state::<Backend>().session.lock().unwrap().clone();
            if tray::is_recording(&s) {
                tray::tick(&app2, &s);
            }
        }
    });
}

fn on_session(app: &AppHandle, session: Value) {
    let b = app.state::<Backend>();
    *b.session.lock().unwrap() = session.clone();
    tray::update(app, &session);
    let want = policy::hud_wanted(&b.service.config(), &session);
    linger(app, &b.hud, windows::HUD, want, policy::HUD_LINGER, true);
}

/// The pill is up from the press until the dictation is done (text inserted); a failure flashes it (or notifies,
/// with the pill off) once (`qrtz_app::pill`).
fn on_talk(app: &AppHandle, talk: &Value) {
    use qrtz_app::pill::{PillAction, ERROR_FLASH};
    let b = app.state::<Backend>();
    let action = b
        .flash
        .gate
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .next(&b.service.config(), talk);
    match action {
        PillAction::Show => {
            b.pill.gen.fetch_add(1, Ordering::SeqCst);
            b.flash.on.store(false, Ordering::SeqCst);
            windows::show_pill(app);
        }
        PillAction::Hide => {
            if !b.flash.on.load(Ordering::SeqCst) {
                b.pill.gen.fetch_add(1, Ordering::SeqCst);
                windows::hide(app, windows::PILL);
            }
        }
        PillAction::Flash { error, notify, .. } => {
            if notify {
                if let Err(e) = app
                    .notification()
                    .builder()
                    .title(crate::strings::UI_TALK_FAILED)
                    .body(&error)
                    .show()
                {
                    tracing::warn!(err = %e, "notification failed");
                }
                return;
            }
            let g = b.pill.gen.fetch_add(1, Ordering::SeqCst) + 1;
            b.flash.on.store(true, Ordering::SeqCst);
            windows::show_pill(app);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(ERROR_FLASH).await;
                let b = app.state::<Backend>();
                if b.pill.gen.load(Ordering::SeqCst) == g {
                    b.flash.on.store(false, Ordering::SeqCst);
                    windows::hide(&app, windows::PILL);
                }
            });
        }
    }
}

/// Show now, or hide/close after `delay` unless something wants it back first.
fn linger(
    app: &AppHandle,
    l: &Linger,
    label: &'static str,
    want: bool,
    delay: std::time::Duration,
    close: bool,
) {
    if want {
        l.gen.fetch_add(1, Ordering::SeqCst);
        l.hiding.store(false, Ordering::SeqCst);
        windows::show_hud(app)
    } else if app.get_webview_window(label).is_some() && !l.hiding.swap(true, Ordering::SeqCst) {
        let g = l.gen.load(Ordering::SeqCst);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay).await;
            let b = app.state::<Backend>();
            let l = &b.hud;
            if l.gen.load(Ordering::SeqCst) == g && l.hiding.swap(false, Ordering::SeqCst) {
                if close {
                    windows::close(&app, label)
                } else {
                    windows::hide(&app, label)
                }
            }
        });
    }
}

/// Settings that the shell enforces: menu bar icon, open at login, global shortcuts, HUD/pill toggles.
fn apply_config(app: &AppHandle, cfg: &Value, first: bool) {
    tray::set_visible(app, cfg["app"]["menuBarIcon"].as_bool().unwrap_or(true));

    // Login item: packaged macOS/Windows builds only (dev and Linux are no-ops, like Electron).
    if !cfg!(debug_assertions) && !cfg!(target_os = "linux") {
        let want = cfg["app"]["launchAtLogin"].as_bool().unwrap_or(false);
        let al = app.autolaunch();
        if al.is_enabled().ok() != Some(want) {
            let r = if want { al.enable() } else { al.disable() };
            if let Err(e) = r {
                tracing::warn!(err = %e, "login item update failed");
            }
        }
    }

    let b = app.state::<Backend>();
    let next = policy::hotkeys(cfg);
    let changed = { *b.hotkeys.lock().unwrap() != next };
    if first || changed {
        bind_hotkeys(app, &next);
        *b.hotkeys.lock().unwrap() = next;
    }
    if !first {
        let s = b.session.lock().unwrap().clone();
        if !policy::hud_wanted(cfg, &s) {
            linger(app, &b.hud, windows::HUD, false, policy::HUD_LINGER, true);
        }
    }
}

fn bind_hotkeys(app: &AppHandle, keys: &[(String, &'static str)]) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    for (accel, action) in keys {
        let action = *action;
        let r = gs.on_shortcut(accel.as_str(), move |app, _sc, ev| {
            if ev.state() != ShortcutState::Pressed {
                return;
            }
            let svc = app.state::<Backend>().service.clone();
            tauri::async_runtime::spawn(async move {
                let r = match action {
                    "record" => svc.runtime.toggle().await,
                    "pause" => svc.runtime.call("note:pause", json!({})).await,
                    _ => svc.talk.toggle().await,
                };
                if let Err(e) = r {
                    tracing::warn!(action, err = ?e, "shortcut failed");
                }
            });
        });
        if let Err(e) = r {
            tracing::warn!(accel, err = %e, "global shortcut unavailable");
        }
    }
}

/// Tray menu actions (tray.rs builds the menu; ids are the TrayAction names).
pub fn tray_action(app: &AppHandle, id: &str) {
    match id {
        "open" => windows::show_main(app),
        "quit" => app.exit(0),
        "start" | "stop" | "dismiss" => {
            let svc = app.state::<Backend>().service.clone();
            let channel = format!("note:{id}");
            tauri::async_runtime::spawn(async move {
                let payload = if channel == "note:start" {
                    json!({})
                } else {
                    Value::Null
                };
                if let Err(e) = svc.runtime.call(&channel, payload).await {
                    tracing::warn!(channel, err = ?e, "tray action failed");
                }
            });
        }
        _ => {}
    }
}

/// A webview navigated or closed: drop its subscriptions (they re-subscribe on boot).
pub fn forget_webview(app: &AppHandle, label: &str) {
    if let Some(b) = app.try_state::<Backend>() {
        b.subs.lock().unwrap().remove_target(label);
    }
}

/// Graceful helper shutdown on quit.
pub fn shutdown(app: &AppHandle) {
    if let Some(b) = app.try_state::<Backend>() {
        if let Some(h) = b.service.helper() {
            let h = h.clone();
            let bye =
                qrtz_helper_bridge::encode_host_message(&json!({"type": "shutdown"})).unwrap_or_default();
            tauri::async_runtime::block_on(h.shutdown(bye, std::time::Duration::from_millis(1500)));
        }
    }
}

#[cfg(test)]
mod speech_source_tests {
    use super::{baked_speech_var, speech_var};

    const VARS: [&str; 5] = [
        "QRTZ_SPEECH_URL",
        "QRTZ_SPEECH_KEY_ID",
        "QRTZ_SPEECH_KEY",
        "QRTZ_SPEECH_ROOT_KEY_ID",
        "QRTZ_SPEECH_ROOT_KEY",
    ];

    /// An environment that sets every speech var to an attacker's value and turns the dev knobs on.
    fn hostile(k: &str) -> Option<String> {
        match k {
            "QRTZ_ALLOW_DEV_KNOBS" => Some("1".into()),
            _ => Some(format!("evil-{k}")),
        }
    }

    #[test]
    fn a_release_build_never_takes_the_speech_url_or_keys_from_the_environment() {
        for k in VARS {
            assert_eq!(
                speech_var(k, false, hostile),
                baked_speech_var(k).map(str::to_owned),
                "{k}"
            );
        }
    }

    #[test]
    fn a_debug_build_may_point_speech_at_the_mock_server() {
        for k in VARS {
            assert_eq!(speech_var(k, true, hostile), Some(format!("evil-{k}")));
            assert_eq!(
                speech_var(k, true, |_| None),
                baked_speech_var(k).map(str::to_owned)
            );
        }
    }

    #[test]
    fn setup_reads_speech_vars_only_through_the_release_safe_resolver() {
        // A guard against wiring the dev-knob switch back in: the speech source must follow `cfg!(debug_assertions)`
        // alone.
        let src = include_str!("backend.rs");
        let setup = &src[src.find("SpeechSetup::from_vars").expect("speech setup")..];
        let call = &setup[..setup.find("})").expect("end of the call")];
        assert!(call.contains("speech_var(k, cfg!(debug_assertions)"), "{call}");
        assert!(!call.contains("QRTZ_ALLOW_DEV_KNOBS"), "{call}");
    }
}
