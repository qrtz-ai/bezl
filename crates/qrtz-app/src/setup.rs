//! Speech setup (ADR 0046 §7, ADR 0061): what onboarding and the settings "speech" section show
//! and do. The speech module downloads in the background with pause, resume and retry, survives a quit (a
//! marker file in the speech store resumes it on the next launch, from the `.part` files), checks for the newest
//! release, re-downloads and deletes. No engine or model names leave this module: a version, a size and a state.
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

/// Lowercase, no engine detail (N72): what the settings row and onboarding say.
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
    events: Events,
    dl: Mutex<Dl>,
    latest: Mutex<Option<(String, u64)>>,
    ctl: DownloadControl,
    running: AtomicBool,
    last_emit: Mutex<Option<Instant>>,
}

impl SpeechManager {
    pub fn new(setup: Option<SpeechSetup>, events: Events) -> Arc<Self> {
        Arc::new(Self {
            setup,
            events,
            dl: Mutex::new(Dl::idle()),
            latest: Mutex::default(),
            ctl: DownloadControl::default(),
            running: AtomicBool::new(false),
            last_emit: Mutex::default(),
        })
    }

    fn marker_path(&self) -> Option<PathBuf> {
        self.setup.as_ref().map(|s| s.store.root().join("download.json"))
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
        let latest = self.latest.lock().unwrap().clone();
        let dl = self.dl.lock().unwrap().clone();
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
        if self.setup.is_none() {
            return Err(Self::unconfigured());
        }
        self.ctl.resume();
        if !self.running.load(Ordering::SeqCst) {
            self.spawn(force);
        }
        Ok(self.status())
    }

    fn spawn(self: &Arc<Self>, force: bool) {
        let Some(setup) = self.setup.clone() else { return };
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        self.set_marker(Some(Marker { force, paused: false }));
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
                    me.set_marker(Some(Marker { force, paused: true }));
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

    pub fn pause(&self) -> Value {
        self.ctl.pause();
        if self.marker().is_some() {
            self.set_marker(Some(Marker {
                force: self.marker().is_some_and(|m| m.force),
                paused: true,
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
        let Some(setup) = &self.setup else {
            return Err(Self::unconfigured());
        };
        if self.running.load(Ordering::SeqCst) {
            return Err(AppError::new(ErrorCode::Unavailable, "pause the download first").retryable(true));
        }
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
}

// ------------------------------------------------------------------------------------------------ port

pub struct Setup {
    pub speech: Arc<SpeechManager>,
}

#[async_trait]
impl SetupPort for Setup {
    async fn call(&self, channel: &str, p: Value) -> Result<Value, AppError> {
        match channel {
            "speech:status" => Ok(self.speech.status()),
            "speech:check" => self.speech.check().await,
            "speech:download" => self.speech.download(p["force"].as_bool().unwrap_or(false)),
            "speech:pause" => Ok(self.speech.pause()),
            "speech:resume" => self.speech.resume(),
            "speech:delete" => self.speech.delete(),
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
