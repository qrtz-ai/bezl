//! Speech setup (ADR 0046 §7, ADR 0061): what the settings "speech" section shows
//! and do. The speech module downloads in the background with pause, resume and retry, survives a quit (a
//! marker file in the speech store resumes it on the next launch, from the `.part` files), checks for the newest
//! release, re-downloads and deletes. No engine or model names leave this module: a version, a size and a state.
//! ADR 0079: a build without a speech module source (the Mac today) offers the helper's own pinned models here
//! instead ([`LocalModels`]), with the same status, progress, pause, resume, retry and delete.
use crate::local_models::LocalModels;
use crate::ports::SetupPort;
use crate::service::Events;
use crate::speech::SpeechSetup;
use async_trait::async_trait;
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_speech::download::{DownloadControl, Downloader, Progress};
use qrtz_speech::spawn::ModuleHealth;
use qrtz_speech::update::{UpdateOutcome, Updater};
use qrtz_speech::SpeechError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn wall() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Progress events at most this often (the last one always goes out).
const EMIT_EVERY: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Dl {
    state: &'static str,
    done: u64,
    total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_kind: Option<&'static str>,
}

impl Dl {
    fn idle() -> Self {
        Dl {
            state: "idle",
            ..Default::default()
        }
    }
}

/// Persisted while a download is wanted (started and not finished, deleted or cancelled): the next launch
/// resumes it (unless the user had paused it).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Marker {
    force: bool,
    paused: bool,
    /// The extensions the download was asked for (`speech:download` `extensions`), so a resume fetches the same sets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    extensions: Option<Vec<String>>,
}

fn error_kind(e: &SpeechError) -> &'static str {
    match e {
        SpeechError::Network(_) => "network",
        SpeechError::Checksum(_) | SpeechError::Missing(_) | SpeechError::TooLarge => "checksum",
        SpeechError::Signature(_)
        | SpeechError::Invalid(_)
        | SpeechError::Rollback { .. }
        | SpeechError::Denied(_) => "signature",
        SpeechError::RevocationExpired | SpeechError::Stale { .. } => "list-expired",
        SpeechError::Health(_) | SpeechError::Incompatible(_) | SpeechError::Unconfined(_) => "health",
        SpeechError::DiskCap | SpeechError::Io(_) => "disk",
        SpeechError::Busy => "busy",
        _ => "other",
    }
}

/// Lowercase, no engine detail (N72): what the settings row says.
fn error_text(kind: &str) -> &'static str {
    match kind {
        "network" => "couldn't reach the download server",
        "checksum" => "the download was damaged; retry to fetch it again",
        "signature" => "the download failed its signature check",
        "list-expired" => "connect to the internet so speech can check for revocations",
        "health" => "speech didn't pass its self-test on this computer",
        "disk" => "not enough disk space for speech",
        "busy" => "speech is in use; try again when recording and dictation stop",
        _ => "speech couldn't be installed",
    }
}

pub struct SpeechManager {
    setup: Option<SpeechSetup>,
    local: Option<Arc<LocalModels>>,
    events: Events,
    dl: Mutex<Dl>,
    latest: Mutex<Option<(String, u64)>>,
    ctl: DownloadControl,
    running: AtomicBool,
    last_emit: Mutex<Option<Instant>>,
    /// Runs after the local models finished installing (wiring: `models.warm` to the helper, ADR 0079 / N121).
    on_installed: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Audit P0-3: the extensions whose models the local download fetches when a request names none (wiring: the
    /// modules switched on). Without one, every pinned set.
    selector: Mutex<Option<Selector>>,
    /// The extensions the current (or last) download request named.
    wanted: Mutex<Option<Vec<String>>>,
}

type Selector = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

impl SpeechManager {
    pub fn new(setup: Option<SpeechSetup>, events: Events) -> Arc<Self> {
        Self::with_local(setup, None, events)
    }

    /// `local`: the helper's pinned models, used only when there is no speech module source (`setup` is None).
    pub fn with_local(setup: Option<SpeechSetup>, local: Option<LocalModels>, events: Events) -> Arc<Self> {
        let local = if setup.is_none() {
            local.map(Arc::new)
        } else {
            None
        };
        Arc::new(Self {
            setup,
            local,
            events,
            dl: Mutex::new(Dl::idle()),
            latest: Mutex::default(),
            ctl: DownloadControl::default(),
            running: AtomicBool::new(false),
            last_emit: Mutex::default(),
            on_installed: Mutex::default(),
            selector: Mutex::default(),
            wanted: Mutex::default(),
        })
    }

    /// Which extensions' models the local download fetches when a request names none (audit P0-3).
    pub fn select_extensions(&self, f: Selector) {
        *self.selector.lock().unwrap() = Some(f);
    }

    /// The pinned sets the next download covers: the request's extensions, else the selector's, else all.
    fn local_view(&self) -> Option<Arc<LocalModels>> {
        let full = self.local.clone()?;
        let exts = self
            .wanted
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.selector.lock().unwrap().clone().map(|f| f()));
        Some(match exts {
            Some(e) => Arc::new(full.for_extensions(&e)),
            None => full,
        })
    }

    /// Called once per completed local model download (not for a module bundle, not when already installed).
    pub fn on_installed(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.on_installed.lock().unwrap() = Some(f);
    }

    /// What a finished install runs (tests stand in for a download with it).
    pub fn installed_hook(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        self.on_installed.lock().unwrap().clone()
    }

    fn marker_path(&self) -> Option<PathBuf> {
        match (&self.setup, &self.local) {
            (Some(s), _) => Some(s.store.root().join("download.json")),
            (None, Some(l)) => Some(l.root().join("download.json")),
            _ => None,
        }
    }
    fn marker(&self) -> Option<Marker> {
        let b = std::fs::read(self.marker_path()?).ok()?;
        serde_json::from_slice(&b).ok()
    }
    fn set_marker(&self, m: Option<Marker>) {
        let Some(p) = self.marker_path() else { return };
        let r = match m {
            Some(m) => qrtz_speech::fsx::write_private_atomic(&p, &serde_json::to_vec(&m).unwrap()),
            None => match std::fs::remove_file(&p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        };
        if let Err(e) = r {
            tracing::warn!(err = %e, "speech download marker");
        }
    }

    pub fn status(&self) -> Value {
        self.status_for(None)
    }

    /// [`Self::status`] of only the models `extensions` need (ADR 0174: echo's view asks about Parakeet before
    /// switching echo on, whatever else is on). `None` = the next download's sets.
    pub fn status_for(&self, extensions: Option<&[String]>) -> Value {
        let dl = self.dl.lock().unwrap().clone();
        let view = match (extensions, &self.local) {
            (Some(e), Some(full)) => Some(Arc::new(full.for_extensions(e))),
            _ => self.local_view(),
        };
        if let (None, Some(l)) = (&self.setup, &view) {
            let installed = l.installed();
            return json!({
                "configured": true,
                "installed": installed.then(|| l.version().to_owned()),
                "bytes": if installed { l.bytes_on_disk() } else { 0 },
                "latest": l.version(),
                "latestBytes": l.total_size(),
                "waiting": null,
                "listExpired": false,
                "download": dl,
            });
        }
        let latest = self.latest.lock().unwrap().clone();
        let (installed, bytes, waiting, expired) = match &self.setup {
            Some(s) => {
                let st = s.store.status(wall());
                (st.current, st.bytes, st.waiting, st.revocation.expired)
            }
            None => (None, 0, None, false),
        };
        json!({
            "configured": self.setup.is_some(),
            "installed": installed,
            "bytes": bytes,
            "latest": latest.as_ref().map(|l| l.0.clone()),
            "latestBytes": latest.as_ref().map(|l| l.1),
            "waiting": waiting,
            "listExpired": expired,
            "download": dl,
        })
    }

    fn emit(&self, force: bool) {
        {
            let mut last = self.last_emit.lock().unwrap();
            if !force && last.is_some_and(|t| t.elapsed() < EMIT_EVERY) {
                return;
            }
            *last = Some(Instant::now());
        }
        self.events.emit("speech:status", self.status());
    }

    fn set_dl(&self, d: Dl) {
        *self.dl.lock().unwrap() = d;
        self.emit(true);
    }

    fn unconfigured() -> AppError {
        AppError::new(ErrorCode::Unavailable, "speech isn't available in this build").retryable(false)
    }

    /// Launch: resume a download a quit interrupted (not one the user paused: that one shows paused).
    pub fn resume_pending(self: &Arc<Self>) {
        let Some(m) = self.marker() else { return };
        *self.wanted.lock().unwrap() = m.extensions.clone();
        if m.paused {
            self.ctl.pause();
            *self.dl.lock().unwrap() = Dl {
                state: "paused",
                ..Default::default()
            };
            return;
        }
        self.spawn(m.force);
    }

    /// Start (or resume) the background download; answers at once with the new status.
    pub fn download(self: &Arc<Self>, force: bool) -> Result<Value, AppError> {
        let wanted = self.wanted.lock().unwrap().clone();
        self.download_for(force, wanted)
    }

    /// [`Self::download`] of only the models `extensions` need (audit P0-3); `None` = the selector's (the modules
    /// switched on), or every set without one. The speech module source ignores it (one bundle).
    pub fn download_for(
        self: &Arc<Self>,
        force: bool,
        extensions: Option<Vec<String>>,
    ) -> Result<Value, AppError> {
        if self.setup.is_none() && self.local.is_none() {
            return Err(Self::unconfigured());
        }
        if !self.running.load(Ordering::SeqCst) {
            *self.wanted.lock().unwrap() = extensions;
        }
        self.ctl.resume();
        if !self.running.load(Ordering::SeqCst) {
            self.spawn(force);
        }
        Ok(self.status())
    }

    fn spawn(self: &Arc<Self>, force: bool) {
        let Some(setup) = self.setup.clone() else {
            return self.spawn_local(force);
        };
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        self.set_marker(Some(Marker {
            force,
            paused: false,
            extensions: None,
        }));
        let keep = self.dl.lock().unwrap().clone();
        self.set_dl(Dl {
            state: "checking",
            done: keep.done,
            total: keep.total,
            ..Default::default()
        });
        let me = self.clone();
        tokio::spawn(async move {
            let dl = Downloader::new(&setup.base_url);
            let health = ModuleHealth {
                opts: setup.opts.clone(),
            };
            let now = wall;
            let up = Updater {
                store: &setup.store,
                dl: &dl,
                health: &health,
                now_ms: &now,
            };
            if let Ok((m, _, _)) = up.latest().await {
                *me.latest.lock().unwrap() = Some((m.version.clone(), m.total_size()));
            }
            let me2 = me.clone();
            let on_progress = move |p: Progress| {
                let installing = p.total > 0 && p.done >= p.total;
                *me2.dl.lock().unwrap() = Dl {
                    state: if installing { "installing" } else { "downloading" },
                    done: p.done,
                    total: p.total,
                    ..Default::default()
                };
                me2.emit(installing);
            };
            let r = up.install_latest(force, &me.ctl, &on_progress).await;
            // N101: `running` goes false last, after the marker and state are final, so a download/resume that
            // starts in between can't have its fresh marker/state overwritten by this run's outcome.
            match r {
                Ok(UpdateOutcome::Quarantined { .. }) => {
                    me.set_marker(None);
                    me.set_dl(Dl {
                        state: "error",
                        error: Some(
                            "this release failed on this computer; re-download to try it again".into(),
                        ),
                        error_kind: Some("health"),
                        ..Default::default()
                    });
                }
                Ok(o) => {
                    tracing::info!(outcome = ?o, "speech install");
                    me.set_marker(None);
                    me.set_dl(Dl::idle());
                }
                Err(SpeechError::Paused) => {
                    me.set_marker(Some(Marker {
                        force,
                        paused: true,
                        extensions: None,
                    }));
                    let d = me.dl.lock().unwrap().clone();
                    me.set_dl(Dl {
                        state: "paused",
                        done: d.done,
                        total: d.total,
                        ..Default::default()
                    });
                }
                Err(e) => {
                    tracing::warn!(err = %e, "speech download failed");
                    let kind = error_kind(&e);
                    let d = me.dl.lock().unwrap().clone();
                    me.set_dl(Dl {
                        state: "error",
                        done: d.done,
                        total: d.total,
                        error: Some(error_text(kind).into()),
                        error_kind: Some(kind),
                    });
                }
            }
            me.running.store(false, Ordering::SeqCst);
        });
    }

    /// The helper's pinned models: one resumable transfer, verified file by file, each set marked when complete.
    fn spawn_local(self: &Arc<Self>, force: bool) {
        let Some(local) = self.local_view() else { return };
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Err(e) = qrtz_speech::fsx::mkdir_private(local.root()) {
            tracing::warn!(err = %e, "speech models dir");
        }
        let extensions = self.wanted.lock().unwrap().clone();
        tracing::info!(sets = ?local.set_ids(), bytes = local.total_size(), "speech models download");
        self.set_marker(Some(Marker {
            force,
            paused: false,
            extensions: extensions.clone(),
        }));
        let keep = self.dl.lock().unwrap().clone();
        self.set_dl(Dl {
            state: "downloading",
            done: keep.done,
            total: local.total_size(),
            ..Default::default()
        });
        let me = self.clone();
        tokio::spawn(async move {
            let me2 = me.clone();
            let on_progress = move |p: Progress| {
                *me2.dl.lock().unwrap() = Dl {
                    state: "downloading",
                    done: p.done,
                    total: p.total,
                    ..Default::default()
                };
                me2.emit(false);
            };
            let r = local.download(force, &me.ctl, &on_progress).await;
            // N101: `running` goes false last (see `spawn`).
            match r {
                Ok(()) => {
                    tracing::info!(version = local.version(), "speech models installed");
                    me.set_marker(None);
                    me.set_dl(Dl::idle());
                    let hook = me.on_installed.lock().unwrap().clone();
                    if let Some(f) = hook {
                        f();
                    }
                }
                Err(SpeechError::Paused) => {
                    me.set_marker(Some(Marker {
                        force: false,
                        paused: true,
                        extensions,
                    }));
                    let d = me.dl.lock().unwrap().clone();
                    me.set_dl(Dl {
                        state: "paused",
                        done: d.done,
                        total: d.total,
                        ..Default::default()
                    });
                }
                Err(e) => {
                    tracing::warn!(err = %e, "speech models download failed");
                    let kind = error_kind(&e);
                    let d = me.dl.lock().unwrap().clone();
                    me.set_dl(Dl {
                        state: "error",
                        done: d.done,
                        total: d.total,
                        error: Some(error_text(kind).into()),
                        error_kind: Some(kind),
                    });
                }
            }
            me.running.store(false, Ordering::SeqCst);
        });
    }

    pub fn pause(&self) -> Value {
        self.ctl.pause();
        if let Some(m) = self.marker() {
            self.set_marker(Some(Marker {
                force: m.force,
                paused: true,
                extensions: m.extensions,
            }));
            let d = self.dl.lock().unwrap().clone();
            if d.state != "installing" {
                self.set_dl(Dl {
                    state: "paused",
                    done: d.done,
                    total: d.total,
                    ..Default::default()
                });
            }
        }
        self.status()
    }

    pub fn resume(self: &Arc<Self>) -> Result<Value, AppError> {
        let force = self.marker().is_some_and(|m| m.force);
        self.download(force)
    }

    pub async fn check(&self) -> Result<Value, AppError> {
        let Some(setup) = &self.setup else {
            // The pinned models have no newer release to look for (an app update brings new pins): no request.
            if self.local.is_some() {
                return Ok(self.status());
            }
            return Err(Self::unconfigured());
        };
        let dl = Downloader::new(&setup.base_url);
        let health = ModuleHealth {
            opts: setup.opts.clone(),
        };
        let now = wall;
        let up = Updater {
            store: &setup.store,
            dl: &dl,
            health: &health,
            now_ms: &now,
        };
        if let Err(e) = up.refresh_trust().await {
            tracing::warn!(err = %e, "speech trust refresh");
        }
        match up.latest().await {
            Ok((m, _, _)) => {
                *self.latest.lock().unwrap() = Some((m.version.clone(), m.total_size()));
                self.emit(true);
                Ok(self.status())
            }
            Err(e) => Err(AppError::new(ErrorCode::Unavailable, error_text(error_kind(&e))).retryable(true)),
        }
    }

    pub fn delete(&self) -> Result<Value, AppError> {
        if self.running.load(Ordering::SeqCst) && (self.setup.is_some() || self.local.is_some()) {
            return Err(AppError::new(ErrorCode::Unavailable, "pause the download first").retryable(true));
        }
        let Some(setup) = &self.setup else {
            let Some(local) = &self.local else {
                return Err(Self::unconfigured());
            };
            if let Err(e) = local.delete() {
                return Err(AppError::new(ErrorCode::Internal, e.to_string()));
            }
            self.set_marker(None);
            self.ctl.resume();
            self.set_dl(Dl::idle());
            return Ok(self.status());
        };
        match setup.store.delete_all() {
            Ok(()) => {}
            Err(SpeechError::Busy) => {
                return Err(AppError::new(ErrorCode::Unavailable, error_text("busy")).retryable(true))
            }
            Err(e) => return Err(AppError::new(ErrorCode::Internal, e.to_string())),
        }
        self.set_marker(None);
        self.ctl.resume();
        self.set_dl(Dl::idle());
        Ok(self.status())
    }

    // ---- ADR 0175 (N181): calls scoped to one module's own models (a module view names exactly itself) ----

    fn scope_error(what: &str) -> AppError {
        AppError::new(ErrorCode::Validation, what.to_owned()).retryable(false)
    }

    /// The extensions the current (or paused) download was asked for.
    fn download_scope(&self) -> Option<Vec<String>> {
        self.wanted
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.marker().and_then(|m| m.extensions))
    }

    /// `speech:pause`: `Some(e)` pauses only a download that was asked for exactly `e`.
    pub fn pause_for(&self, extensions: Option<&[String]>) -> Result<Value, AppError> {
        let Some(e) = extensions else {
            return Ok(self.pause());
        };
        let busy = self.running.load(Ordering::SeqCst) || self.marker().is_some();
        if busy && self.download_scope().as_deref() != Some(e) {
            return Err(Self::scope_error("this download isn't for these models"));
        }
        if busy {
            self.pause();
        }
        Ok(self.status_for(Some(e)))
    }

    /// `speech:resume`: `Some(e)` resumes only a download that was asked for exactly `e`.
    pub fn resume_for(self: &Arc<Self>, extensions: Option<&[String]>) -> Result<Value, AppError> {
        let Some(e) = extensions else {
            return self.resume();
        };
        let Some(m) = self.marker() else {
            return Ok(self.status_for(Some(e)));
        };
        if m.extensions.as_deref() != Some(e) {
            return Err(Self::scope_error("this download isn't for these models"));
        }
        self.download_for(m.force, Some(e.to_vec()))?;
        Ok(self.status_for(Some(e)))
    }

    /// `speech:check`: `Some(e)` answers with the status of only those models.
    pub async fn check_for(&self, extensions: Option<&[String]>) -> Result<Value, AppError> {
        let all = self.check().await?;
        Ok(match extensions {
            Some(e) => self.status_for(Some(e)),
            None => all,
        })
    }

    /// `speech:delete`: `Some(e)` removes only the pinned sets `e` needs that no other switched-on module needs. The
    /// speech module source is one shared bundle: only settings deletes it.
    pub fn delete_for(&self, extensions: Option<&[String]>) -> Result<Value, AppError> {
        let Some(e) = extensions else {
            return self.delete();
        };
        if self.setup.is_some() {
            return Err(Self::scope_error(
                "the speech module is shared: remove it in settings",
            ));
        }
        let Some(full) = &self.local else {
            return Err(Self::unconfigured());
        };
        if self.running.load(Ordering::SeqCst) {
            return Err(AppError::new(ErrorCode::Unavailable, "pause the download first").retryable(true));
        }
        let others: Vec<String> = self
            .selector
            .lock()
            .unwrap()
            .clone()
            .map(|f| f())
            .unwrap_or_default()
            .into_iter()
            .filter(|x| !e.contains(x))
            .collect();
        let kept: Vec<String> = others
            .iter()
            .flat_map(|o| crate::local_models::sets_needed_by(o))
            .collect();
        let view = full.for_extensions(e).without_sets(&kept);
        if let Err(err) = view.delete() {
            return Err(AppError::new(ErrorCode::Internal, err.to_string()));
        }
        if self.marker().is_some_and(|m| m.extensions.as_deref() == Some(e)) {
            self.set_marker(None);
            *self.wanted.lock().unwrap() = None;
            self.ctl.resume();
            self.set_dl(Dl::idle());
        } else {
            self.emit(true);
        }
        Ok(self.status_for(Some(e)))
    }
}

// ------------------------------------------------------------------------------------------------ port

pub struct Setup {
    pub speech: Arc<SpeechManager>,
}

/// A speech request's `extensions` (None = not named).
fn scope(p: &Value) -> Option<Vec<String>> {
    p["extensions"].as_array().map(|a| {
        a.iter()
            .filter_map(|e| e.as_str().map(str::to_owned))
            .collect::<Vec<_>>()
    })
}

#[async_trait]
impl SetupPort for Setup {
    async fn call(&self, channel: &str, p: Value) -> Result<Value, AppError> {
        match channel {
            "speech:status" => Ok(self.speech.status_for(scope(&p).as_deref())),
            "speech:check" => self.speech.check_for(scope(&p).as_deref()).await,
            "speech:download" => self
                .speech
                .download_for(p["force"].as_bool().unwrap_or(false), scope(&p)),
            "speech:pause" => self.speech.pause_for(scope(&p).as_deref()),
            "speech:resume" => self.speech.resume_for(scope(&p).as_deref()),
            "speech:delete" => self.speech.delete_for(scope(&p).as_deref()),
            _ => Err(AppError::new(ErrorCode::UnknownChannel, channel.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    /// N100: the speech bundle download allows plain http only to a real loopback host (qrtz-llm's copy went with
    /// the cloud client, ADR 0076).
    #[test]
    fn the_loopback_check_is_strict() {
        for (url, loopback) in [
            ("http://127.0.0.1:8080", true),
            ("http://127.1", true),
            ("http://localhost/x", true),
            ("http://[::1]:9/", true),
            ("http://127.0.0.1.example.com", false),
            ("http://localhost.example.com", false),
            ("http://user@127.0.0.1", false),
            ("https://huggingface.co", false),
            ("ftp://127.0.0.1", false),
            ("not a url", false),
        ] {
            assert_eq!(qrtz_speech::download::is_loopback_url(url), loopback, "{url}");
        }
    }
}
