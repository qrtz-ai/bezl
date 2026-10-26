//! echo's experimental Phonon-2.5 English recogniser on disk (QRT-367; gated: nothing calls this until release
//! clearance). An opt-in model, laid out like ctrl's Laya model (ADR 0116, `ctrl_model`):
//!
//! - It is not in echo's required set: `model:phonon-2.5-en` is an `optional` dependency in echo's module.json and its
//!   set lives in its own pinned manifest (`echo_phonon_model.json`), not `local_models.json`, so
//!   `speech:download {extensions:["echo"]}` and onboarding never fetch it.
//! - Nothing is fetched until the user turns "Use Phonon-2.5 (English) when possible" on (the consent, persisted at
//!   `<data>/echo/phonon.json`, 0600). There is no implied yes. Turning it off deletes the model.
//! - Before any network call the manifest is checked ([`validate`]): every path relative with no `..`, `.`,
//!   absolute or backslash part, every sha256 64 lowercase hex, every size non-zero, the whole set at most
//!   [`MAX_TOTAL_BYTES`].
//! - The download is `local_models`' (the package manager's): resumable, each file size- and sha256-checked into the
//!   content-addressed store and installed read-only as the `model.phonon-2.5-en` package, under a fresh signed
//!   revocation list like Laya.
//! - Hash before use: [`EchoPhononModel::ready_path`] hands the helper a path only after every file in the store-linked
//!   package dir re-hashed to its pinned sha256 (once per process and dir; modes and sizes at every call).
//! - echo's process is granted that one package read-only (`extension.reads`, scoped by
//!   [`qrtz_modules::bundled_read_allowed`]); it gets no `config.write` for it and never fetches anything itself.
use crate::local_models::{DownloadControl, LocalModels, Manifest, Progress};
use qrtz_speech::{fsx, SpeechError};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

pub const MANIFEST: &str = include_str!("echo_phonon_model.json");
pub const PACKAGE: &str = "model.phonon-2.5-en";
/// The pinned set is ~424 MB; anything over this is refused before any network call.
pub const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// Why a manifest is refused (a short code, never content).
pub fn validate(m: &Manifest) -> Result<(), String> {
    let [set] = m.sets.as_slice() else {
        return Err("phonon.manifest_sets".into());
    };
    if set.package != PACKAGE {
        return Err("phonon.manifest_package".into());
    }
    if !safe_rel(&set.dir) || set.dir.contains('/') {
        return Err("phonon.manifest_path".into());
    }
    if set.files.is_empty() {
        return Err("phonon.manifest_empty".into());
    }
    let mut total: u64 = 0;
    let mut seen = std::collections::BTreeSet::new();
    for f in &set.files {
        if !safe_rel(&f.path) || !safe_rel(&f.remote) {
            return Err("phonon.manifest_path".into());
        }
        if !seen.insert(f.path.as_str()) {
            return Err("phonon.manifest_duplicate".into());
        }
        if f.sha256.len() != 64
            || !f
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("phonon.manifest_sha256".into());
        }
        if f.size == 0 {
            return Err("phonon.manifest_size".into());
        }
        total = total.checked_add(f.size).ok_or("phonon.too_large")?;
    }
    if total > MAX_TOTAL_BYTES {
        return Err("phonon.too_large".into());
    }
    Ok(())
}

/// A relative path made only of normal parts: no `..`, `.`, root, empty part or backslash.
fn safe_rel(p: &str) -> bool {
    !p.is_empty()
        && !p.contains('\\')
        && !p.contains('\0')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
        && Path::new(p)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Consent {
    consented: bool,
    at: u64,
}

/// What echo's settings row shows next to the toggle (codes; lowercase on screen).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhononModelState {
    pub enabled: bool,
    /// `off`, `not_installed`, `installed`, `refused` (the manifest failed [`validate`]).
    pub state: &'static str,
    pub size_bytes: u64,
    pub bytes_on_disk: u64,
}

pub struct EchoPhononModel {
    models: LocalModels,
    refused: Option<String>,
    consent_file: PathBuf,
    set: String,
    entry: PathBuf,
    verified: Mutex<Option<PathBuf>>,
}

impl EchoPhononModel {
    /// `models_root` is `<data>/models/echo-phonon`, `echo_dir` is `<data>/echo`; `base` overrides the host in tests.
    pub fn new(models_root: impl Into<PathBuf>, echo_dir: &Path, base: Option<String>) -> Self {
        let m: Manifest = serde_json::from_str(MANIFEST).expect("echo_phonon_model.json");
        Self::with_manifest(models_root, echo_dir, m, base)
    }

    pub fn with_manifest(
        models_root: impl Into<PathBuf>,
        echo_dir: &Path,
        mut m: Manifest,
        base: Option<String>,
    ) -> Self {
        let refused = validate(&m).err();
        let set = m.sets.first().map(|s| s.id.clone()).unwrap_or_default();
        let entry = m
            .sets
            .first()
            .and_then(|s| s.files.first())
            .and_then(|f| Path::new(&f.path).components().next())
            .map(|c| PathBuf::from(c.as_os_str()))
            .unwrap_or_default();
        if refused.is_some() {
            // Never handed to the downloader (which also refuses unsafe paths, by panicking).
            m.sets.clear();
        }
        EchoPhononModel {
            models: LocalModels::with_manifest(models_root, m, base)
                .with_packages_root(echo_dir.parent().unwrap_or(echo_dir)),
            refused,
            consent_file: echo_dir.join("phonon.json"),
            set,
            entry,
            verified: Mutex::new(None),
        }
    }

    /// Tests: the signed index's root pin and hosts (loopback).
    pub fn with_index(mut self, root_pin: Option<String>, hosts: Vec<String>) -> Self {
        self.models = self.models.with_index(root_pin, hosts);
        self
    }

    /// Whether the user turned the setting on. No answer is no (never implied, unlike Laya's ADR 0169 default).
    pub fn enabled(&self) -> bool {
        std::fs::read(&self.consent_file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Consent>(&b).ok())
            .is_some_and(|c| c.consented)
    }

    /// The toggle. Off also deletes the model and any partial download.
    pub fn set_enabled(&self, yes: bool, now_ms: u64) -> std::io::Result<()> {
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
            *self.verified.lock().unwrap_or_else(crate::poison::recover) = None;
        }
        Ok(())
    }

    pub fn installed(&self) -> bool {
        self.refused.is_none() && self.models.installed()
    }

    /// The `.mlpackage` the helper may load: only when on, installed, every file read-only at its pinned size, and
    /// (once per process and package dir) every file at its pinned sha256.
    pub fn ready_path(&self) -> Option<PathBuf> {
        if self.refused.is_some() || !self.enabled() {
            return None;
        }
        let dir = self.models.read_dir(&self.set, false)?;
        let mut v = self.verified.lock().unwrap_or_else(crate::poison::recover);
        if v.as_ref() != Some(&dir) {
            if self.models.read_dir(&self.set, true).as_ref() != Some(&dir) {
                return None;
            }
            *v = Some(dir.clone());
        }
        Some(dir.join(&self.entry))
    }

    pub fn state(&self) -> PhononModelState {
        let enabled = self.enabled();
        PhononModelState {
            enabled,
            state: match (enabled, self.refused.is_some(), self.installed()) {
                (_, true, _) => "refused",
                (false, _, _) => "off",
                (true, _, false) => "not_installed",
                (true, _, true) => "installed",
            },
            size_bytes: self.models.total_size(),
            bytes_on_disk: self.models.bytes_on_disk(),
        }
    }

    /// Fetch the model: refused before any network call without the user's yes or on a manifest [`validate`] refuses.
    pub async fn download(
        &self,
        ctl: &DownloadControl,
        on_progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<(), SpeechError> {
        if let Some(code) = &self.refused {
            return Err(SpeechError::Denied(code.clone()));
        }
        if !self.enabled() {
            return Err(SpeechError::Denied("phonon.off".into()));
        }
        self.models.download(false, ctl, on_progress).await
    }
}
