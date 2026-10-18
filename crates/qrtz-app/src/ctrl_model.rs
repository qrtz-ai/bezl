//! ADR 0116: ctrl's Laya model on disk. ctrl is offline: the model is fetched only after the user says yes in
//! onboarding or ctrl's settings, through the same pinned-manifest, resumable, size- and sha256-checked path as the
//! speech models (ADR 0079, `local_models`), into `<data>/models/ctrl/` (0700). The consent is persisted next to
//! ctrl's store (`<data>/ctrl/laya.json`, 0600); withdrawing it deletes the model. The host hands the installed
//! model's path to the helper with every `ctrl.laya` question ([`qrtz_ctrl::laya::HelperLaya::path`]); without
//! consent, without the files, on a placeholder manifest or when the helper can't run it, the deterministic heuristic
//! scorer answers.
//!
//! The pinned manifest (`ctrl_laya_model.json`) is a **placeholder** until a Core ML build of Laya with a known url,
//! size and sha256 exists: `download` refuses it with `laya.placeholder` before any network call.
use crate::local_models::{LocalModels, Manifest};
use qrtz_speech::download::{DownloadControl, Progress};
use qrtz_speech::{fsx, SpeechError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const MANIFEST: &str = include_str!("ctrl_laya_model.json");

#[derive(Debug, Clone, Deserialize)]
pub struct CtrlManifest {
    /// True until the real model is pinned: the download is refused, nothing goes on the network.
    #[serde(default)]
    pub placeholder: bool,
    #[serde(flatten)]
    pub manifest: Manifest,
}

impl CtrlManifest {
    pub fn pinned() -> CtrlManifest {
        serde_json::from_str(MANIFEST).expect("ctrl_laya_model.json")
    }
}

/// What the ctrl settings show (all lowercase on screen; the strings here are codes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayaModelState {
    pub consented: bool,
    /// `off` (no consent), `unavailable` (consented, placeholder manifest), `not_installed`, `installed`.
    pub state: &'static str,
    pub size_bytes: u64,
    pub bytes_on_disk: u64,
    /// `laya` only when the model is consented and installed (and the build has the runtime); else `heuristic`.
    pub scorer: &'static str,
    /// A short code when the last download failed, never content.
    pub error: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Consent {
    consented: bool,
    at: u64,
}

pub struct CtrlLayaModel {
    models: LocalModels,
    placeholder: bool,
    consent_file: PathBuf,
    /// `<set dir>/<first path component>`, e.g. `laya/laya.mlpackage`.
    entry: PathBuf,
}

impl CtrlLayaModel {
    /// `models_root` is `<data>/models/ctrl`, `ctrl_dir` is `<data>/ctrl`; `base` overrides the host in tests.
    pub fn new(models_root: impl Into<PathBuf>, ctrl_dir: &Path, base: Option<String>) -> Self {
        Self::with_manifest(models_root, ctrl_dir, CtrlManifest::pinned(), base)
    }

    pub fn with_manifest(
        models_root: impl Into<PathBuf>,
        ctrl_dir: &Path,
        m: CtrlManifest,
        base: Option<String>,
    ) -> Self {
        let set = m.manifest.sets.first().expect("one model set");
        let first = set
            .files
            .first()
            .and_then(|f| Path::new(&f.path).components().next())
            .expect("one model file");
        let entry = Path::new(&set.dir).join(first);
        CtrlLayaModel {
            entry,
            placeholder: m.placeholder,
            models: LocalModels::with_manifest(models_root, m.manifest, base),
            consent_file: ctrl_dir.join("laya.json"),
        }
    }

    pub fn consented(&self) -> bool {
        std::fs::read(&self.consent_file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Consent>(&b).ok())
            .is_some_and(|c| c.consented)
    }

    /// Record the user's answer. `false` also deletes the model and any partial download.
    pub fn set_consent(&self, yes: bool, now_ms: u64) -> std::io::Result<()> {
        if let Some(d) = self.consent_file.parent() {
            fsx::mkdir_private(d)?;
        }
        let c = Consent {
            consented: yes,
            at: now_ms,
        };
        fsx::write_private_atomic(&self.consent_file, &serde_json::to_vec(&c)?)?;
        if !yes {
            self.models.delete()?;
        }
        Ok(())
    }

    pub fn installed(&self) -> bool {
        !self.placeholder && self.models.installed()
    }

    /// The model the helper should run: the set's first top-level entry (`laya.mlpackage` / `laya.mlmodelc`).
    pub fn model_path(&self) -> PathBuf {
        self.models.root().join(&self.entry)
    }

    /// What `select_scorer` reads: consent, and the path only when installed.
    pub fn consent(&self) -> qrtz_ctrl::laya::ModelConsent {
        let consented = self.consented();
        qrtz_ctrl::laya::ModelConsent {
            consented,
            installed: (consented && self.installed()).then(|| self.model_path()),
        }
    }

    pub fn state(&self, runtime: bool) -> LayaModelState {
        let consented = self.consented();
        let installed = self.installed();
        let state = match (consented, self.placeholder, installed) {
            (false, _, _) => "off",
            (true, true, _) => "unavailable",
            (true, false, false) => "not_installed",
            (true, false, true) => "installed",
        };
        LayaModelState {
            consented,
            state,
            size_bytes: self.models.total_size(),
            bytes_on_disk: self.models.bytes_on_disk(),
            scorer: if runtime && state == "installed" {
                "laya"
            } else {
                "heuristic"
            },
            error: None,
        }
    }

    /// Fetch the model: only with consent and a real (non-placeholder) manifest; otherwise refused before any
    /// network call.
    pub async fn download(
        &self,
        ctl: &DownloadControl,
        on_progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<(), SpeechError> {
        if !self.consented() {
            return Err(SpeechError::Denied("laya.no_consent".into()));
        }
        if self.placeholder {
            return Err(SpeechError::Denied("laya.placeholder".into()));
        }
        self.models.download(false, ctl, on_progress).await
    }
}

/// The `ctrl:model` / `ctrl:model-consent` channels (ADR 0116): the model plus the one download in flight.
pub struct CtrlModelSlot {
    model: Arc<CtrlLayaModel>,
    busy: Arc<Mutex<Option<DownloadControl>>>,
    error: Arc<Mutex<Option<String>>>,
}

fn code(e: &SpeechError) -> String {
    let s = match e {
        SpeechError::Denied(c) => c.as_str(),
        SpeechError::Checksum(_) => "checksum",
        _ => "download.failed",
    };
    s.chars().take(64).collect()
}

impl CtrlModelSlot {
    pub fn new(model: CtrlLayaModel) -> Self {
        CtrlModelSlot {
            model: Arc::new(model),
            busy: Arc::default(),
            error: Arc::default(),
        }
    }

    pub fn state(&self) -> LayaModelState {
        let mut s = self.model.state(qrtz_ctrl::laya::RUNTIME_BUILD);
        if self.busy.lock().unwrap().is_some() && s.consented {
            s.state = "downloading";
        }
        let err = self.error.lock().unwrap().clone();
        if err.is_some() && s.state == "not_installed" {
            s.state = "failed";
        }
        s.error = err;
        s
    }

    /// Record the answer; yes starts the download on `rt` (refused at once on the placeholder manifest), no cancels
    /// a download in flight and deletes the model. A changed consent applies to the scorer at the next app start.
    pub async fn set_consent(&self, yes: bool, now_ms: u64) -> std::io::Result<LayaModelState> {
        if !yes {
            if let Some(ctl) = self.busy.lock().unwrap().take() {
                ctl.pause(); // stops the transfer; the task ends and the files go below
            }
        }
        self.model.set_consent(yes, now_ms)?;
        *self.error.lock().unwrap() = None;
        if yes && !self.model.installed() && self.busy.lock().unwrap().is_none() {
            let ctl = DownloadControl::default();
            *self.busy.lock().unwrap() = Some(ctl.clone());
            let (model, busy, error) = (self.model.clone(), self.busy.clone(), self.error.clone());
            let run = async move {
                let r = model.download(&ctl, &|_| {}).await;
                if !ctl.is_paused() {
                    *error.lock().unwrap() = r.err().map(|e| code(&e));
                }
                *busy.lock().unwrap() = None;
            };
            match tokio::runtime::Handle::try_current() {
                Ok(h) => {
                    // The placeholder refusal (and a no-consent refusal) is immediate: wait a moment so the answer
                    // already says so; a real download keeps going in the background.
                    let t = h.spawn(run);
                    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), t).await;
                }
                Err(_) => *self.busy.lock().unwrap() = None,
            }
        }
        Ok(self.state())
    }
}
