//! ADR 0116: ctrl's Laya model on disk. ctrl is offline: the model is fetched only after the user says yes in
//! ctrl's settings, through the same pinned-manifest, resumable, size- and sha256-checked path as the
//! speech models (ADR 0079, `local_models`), as the `model.laya` package.
//!
//! ADR 0212 (superseding ADR 0157 §9's allowlist): ctrl declares `model.laya` in its `extension.reads`, and the
//! resolver grants its process that model and nothing else; the planned exit is still a sandboxed `inference.laya`
//! XPC service. Until then the model is loaded read-only straight from its store-linked
//! package dir (`<data>/packages/model.laya/<version>/`, files 0444/0400, every sha256 re-checked right after
//! linking, modes and sizes checked at every load), never from a writable copy; and it installs only under a fresh
//! signed revocation list, like code (§6), so the download refreshes the index first and is refused
//! (`laya.needs_signed_index`) until `modules.qrtz.ai` is live and its root is pinned. The consent is persisted next to
//! ctrl's store (`<data>/ctrl/laya.json`, 0600); withdrawing it deletes the model. The host hands the installed
//! model's path to the helper with every `ctrl.laya` question ([`qrtz_ctrl::laya::HelperLaya::path`]). QRT-293: the
//! model is required. Without consent, without the files, mid-download, on a sha256 mismatch, on a placeholder
//! manifest or when the helper can't load it, ctrl decides nothing and takes no action; [`CtrlModelSlot::readiness`]
//! is the one status the scorer and `ctrl:model` (`status`) both read.
//!
//! The pinned manifest (`ctrl_laya_model.json`, ADR 0129) is ctrl's default decision model: FluidInference/laya-coreml
//! @ 7b8d7a2, the e8 512-token bucket (`laya_multilingual_e8_L512_options32.mlmodelc`, 4 files) plus `tokenizer.json`
//! beside it, about 484 MB. A manifest marked `placeholder` is still refused with `laya.placeholder` before any
//! network call.
// ADR 0078/0079: the download runs only through `local_models` (the pinned, sha256-checked path).
use crate::local_models::{DownloadControl, LocalModels, Manifest, Progress};
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
    /// Legacy: `laya` only when `status` is `ready`. `heuristic` now means no model is deciding, so ctrl takes no
    /// action (QRT-293 removed the rules scorer); read `status` instead.
    pub scorer: &'static str,
    /// QRT-293: whether ctrl can decide (`ready`), else why not (`not_downloaded`, `downloading`, `hash_mismatch`,
    /// `load_failed`, `unavailable`): what the "download to start" screen shows.
    pub status: &'static str,
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
    set: String,
    /// The first path component in the package, e.g. `laya.mlpackage`.
    entry: PathBuf,
    /// The package dir whose every file re-hashed to its pinned sha256 in this process (QRT-293: checked once
    /// before the first decision, then modes and sizes at every question).
    verified: Mutex<Option<PathBuf>>,
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
        let entry = PathBuf::from(first.as_os_str());
        CtrlLayaModel {
            set: set.id.clone(),
            entry,
            placeholder: m.placeholder,
            // The packages live in `<data>`, the parent of `ctrl_dir`.
            models: LocalModels::with_manifest(models_root, m.manifest, base)
                .with_packages_root(ctrl_dir.parent().unwrap_or(ctrl_dir)),
            consent_file: ctrl_dir.join("laya.json"),
            verified: Mutex::new(None),
        }
    }

    /// Tests: the signed index's root pin and hosts (loopback).
    pub fn with_index(mut self, root_pin: Option<String>, hosts: Vec<String>) -> Self {
        self.models = self.models.with_index(root_pin, hosts);
        self
    }

    /// The user's answer in ctrl's settings; with none, ADR 0169: `model.laya` installed with ctrl (its dependency,
    /// listed in the install's plan and covered by that one consent) is the yes. A recorded "no" still wins.
    pub fn consented(&self) -> bool {
        match std::fs::read(&self.consent_file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Consent>(&b).ok())
        {
            Some(c) => c.consented,
            None => self.installed(),
        }
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

    /// The model the helper should run, when installed: the package's first top-level entry (`laya.mlpackage` /
    /// `laya.mlmodelc`) inside the read-only, store-linked package dir. Checked at every call: every file read-only
    /// and of its pinned size.
    pub fn model_path(&self) -> Option<PathBuf> {
        if self.placeholder {
            return None;
        }
        self.models
            .read_dir(&self.set, false)
            .map(|d| d.join(&self.entry))
    }

    /// What `select_scorer` reads: consent, and the path only when installed.
    pub fn consent(&self) -> qrtz_ctrl::laya::ModelConsent {
        let consented = self.consented();
        qrtz_ctrl::laya::ModelConsent {
            consented,
            installed: consented.then(|| self.model_path()).flatten(),
        }
    }

    /// QRT-293: whether the files can decide. `Ready` (with the path) only when consented, installed, every file at
    /// its pinned size and read-only mode, and (once per process and package dir) every file at its pinned sha256.
    /// Download state (in flight, a failed checksum) is the slot's; see [`CtrlModelSlot::readiness`].
    pub fn readiness(&self) -> qrtz_ctrl::laya::Readiness {
        use qrtz_ctrl::decide::ModelStatus as S;
        use qrtz_ctrl::laya::Readiness;
        if self.placeholder {
            return Readiness::not(S::Unavailable);
        }
        if !self.consented() || !self.installed() {
            return Readiness::not(S::NotDownloaded);
        }
        let Some(dir) = self.models.read_dir(&self.set, false) else {
            return Readiness::not(S::HashMismatch);
        };
        let mut v = self.verified.lock().unwrap_or_else(crate::poison::recover);
        if v.as_ref() != Some(&dir) {
            if self.models.read_dir(&self.set, true).as_ref() != Some(&dir) {
                return Readiness::not(S::HashMismatch);
            }
            *v = Some(dir.clone());
        }
        Readiness {
            status: S::Ready,
            path: Some(dir.join(&self.entry)),
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
        let mut s = LayaModelState {
            consented,
            state,
            size_bytes: self.models.total_size(),
            bytes_on_disk: self.models.bytes_on_disk(),
            scorer: "heuristic",
            status: if runtime {
                self.readiness().status.code()
            } else {
                qrtz_ctrl::decide::ModelStatus::Unavailable.code()
            },
            error: None,
        };
        if s.status == "ready" {
            s.scorer = "laya";
        }
        s
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
        SpeechError::RevocationExpired => "laya.needs_signed_index",
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

    /// QRT-293: the one status ctrl's scorer and `ctrl:model` read: the files' readiness, with the download in
    /// flight (`downloading`) and a download that failed its sha256 (`hash_mismatch`) on top.
    pub fn readiness(&self) -> qrtz_ctrl::laya::Readiness {
        use qrtz_ctrl::decide::ModelStatus as S;
        let r = self.model.readiness();
        if r.status == S::Ready {
            return r;
        }
        if self.busy.lock().unwrap_or_else(crate::poison::recover).is_some() && self.model.consented() {
            return qrtz_ctrl::laya::Readiness::not(S::Downloading);
        }
        if self
            .error
            .lock()
            .unwrap_or_else(crate::poison::recover)
            .as_deref()
            == Some("checksum")
        {
            return qrtz_ctrl::laya::Readiness::not(S::HashMismatch);
        }
        r
    }

    pub fn state(&self) -> LayaModelState {
        let mut s = self.model.state(qrtz_ctrl::laya::RUNTIME_BUILD);
        if qrtz_ctrl::laya::RUNTIME_BUILD {
            s.status = self.readiness().status.code();
            s.scorer = if s.status == "ready" { "laya" } else { "heuristic" };
        }
        if self.busy.lock().unwrap_or_else(crate::poison::recover).is_some() && s.consented {
            s.state = "downloading";
        }
        let err = self.error.lock().unwrap_or_else(crate::poison::recover).clone();
        if err.is_some() && s.state == "not_installed" {
            s.state = "failed";
        }
        s.error = err;
        s
    }

    /// Record the answer; yes starts the download on `rt` (refused at once on the placeholder manifest), no cancels
    /// a download in flight and deletes the model. A changed consent (or a finished download) applies to the scorer at the next question (`LiveScorer`, run 19).
    pub async fn set_consent(&self, yes: bool, now_ms: u64) -> std::io::Result<LayaModelState> {
        if !yes {
            if let Some(ctl) = self.busy.lock().unwrap_or_else(crate::poison::recover).take() {
                ctl.pause(); // stops the transfer; the task ends and the files go below
            }
        }
        self.model.set_consent(yes, now_ms)?;
        *self.error.lock().unwrap_or_else(crate::poison::recover) = None;
        if yes && !self.model.installed() && self.busy.lock().unwrap_or_else(crate::poison::recover).is_none()
        {
            let ctl = DownloadControl::default();
            *self.busy.lock().unwrap_or_else(crate::poison::recover) = Some(ctl.clone());
            let (model, busy, error) = (self.model.clone(), self.busy.clone(), self.error.clone());
            let run = async move {
                let r = model.download(&ctl, &|_| {}).await;
                if !ctl.is_paused() {
                    *error.lock().unwrap_or_else(crate::poison::recover) = r.err().map(|e| code(&e));
                }
                *busy.lock().unwrap_or_else(crate::poison::recover) = None;
            };
            match tokio::runtime::Handle::try_current() {
                Ok(h) => {
                    // The placeholder refusal (and a no-consent refusal) is immediate: wait a moment so the answer
                    // already says so; a real download keeps going in the background.
                    let t = h.spawn(run);
                    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), t).await;
                }
                Err(_) => *self.busy.lock().unwrap_or_else(crate::poison::recover) = None,
            }
        }
        Ok(self.state())
    }
}
