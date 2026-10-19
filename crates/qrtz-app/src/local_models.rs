//! ADR 0079 / ADR 0157: the Mac helper's own speech models (Nemotron ASR for recordings, Nemotron 3 diarization,
//! LocalVQE echo cancellation, Parakeet TDT v2 for echo dictation) for builds without the signed speech module. Each
//! set is a `model.*` package whose tree (every file's path, size and sha256, and its Hugging Face URL at a fixed
//! revision) is pinned in this binary (`local_models.json`). The download is the package manager's: onboarding or
//! settings asks, `Op::Pinned` plans it, and the one fetcher takes each file from the package hosts
//! (`modules.qrtz.ai`, then its mirrors) or else the pinned Hugging Face URL, verified by the same sha256 into the
//! content-addressed store, then installed read-only under `<data>/packages/`. The helper's view is unchanged:
//! `<data>/models/<set>/` (0700) holds store links of the files, and a set is usable once its `.qrtz-verified` marker
//! is written; the helper reads only marked sets (`QRTZ_MODELS_DIR`) and never fetches anything itself.
//!
//! A model an extension reads in its own process (ctrl's `model.laya`, ADR 0157 §9) gets no view: it is loaded
//! read-only straight from its store-linked package dir ([`LocalModels::read_dir`]), and, like code, it installs
//! only under a fresh signed revocation list (§6), so its download refreshes the index first.
use crate::module_hosting::{AppleCodeChecks, Hosts, HttpTransport, InstallError, Op};
use qrtz_modules::install::{Installer, PinnedPackage};
use qrtz_modules::models::{ModelFile as PinnedFile, ModelSource};
/// The download's control and progress types, for the model sets that go through here (ctrl's Laya model, ADR
/// 0116).
pub use qrtz_speech::download::{DownloadControl, Progress};
use qrtz_speech::{fsx, SpeechError};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = include_str!("local_models.json");
/// Written last in a set's directory, holding the manifest version: the helper loads only marked sets.
pub const MARKER: &str = ".qrtz-verified";
/// The env var the helper reads the models root from.
pub const ENV: &str = "QRTZ_MODELS_DIR";

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub base: String,
    pub sets: Vec<ModelSet>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSet {
    pub id: String,
    /// The `model.*` package this set is.
    pub package: String,
    pub task: String,
    /// The package version when it isn't the manifest's (ctrl's Laya manifest carries a model build name).
    #[serde(default)]
    pub package_version: Option<String>,
    #[serde(default)]
    pub license: String,
    pub dir: String,
    pub repo: String,
    pub revision: String,
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    pub remote: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

impl Manifest {
    pub fn pinned() -> Manifest {
        serde_json::from_str(MANIFEST).expect("local_models.json")
    }
    pub fn total_size(&self) -> u64 {
        self.sets.iter().flat_map(|s| &s.files).map(|f| f.size).sum()
    }
}

pub struct LocalModels {
    root: PathBuf,
    packages_root: PathBuf,
    manifest: Manifest,
    /// The Hugging Face host actually reached (tests: a loopback server); `None` = the pinned hosts.
    base: Option<String>,
    /// The root pin and package hosts for the signed index (tests: loopback); `None` = `Hosts::pinned()`.
    index: Option<(Option<String>, Vec<String>)>,
}

/// The extension whose own process reads `package` ([`qrtz_modules::qrtz_json::EXTENSION_READS`]).
fn reader_of(package: &str) -> Option<&'static str> {
    qrtz_modules::qrtz_json::EXTENSION_READS
        .iter()
        .find(|(_, m)| *m == package)
        .map(|(e, _)| *e)
}

fn safe_rel(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && Path::new(p)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

impl LocalModels {
    /// The pinned manifest. `root` is `<data>/models`; the packages live in its parent (`<data>`). `base` replaces
    /// the Hugging Face host and drops the package hosts (tests serve the files from loopback).
    pub fn new(root: impl Into<PathBuf>, base: Option<String>) -> Self {
        Self::with_manifest(root, Manifest::pinned(), base)
    }

    pub fn with_manifest(root: impl Into<PathBuf>, manifest: Manifest, base: Option<String>) -> Self {
        let root: PathBuf = root.into();
        let packages_root = root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.clone());
        let m = Self {
            root,
            packages_root,
            manifest,
            base,
            index: None,
        };
        for s in &m.manifest.sets {
            assert!(safe_rel(&s.dir), "model set dir {}", s.dir);
            for f in &s.files {
                assert!(safe_rel(&f.path) && safe_rel(&f.remote), "model file {}", f.path);
            }
            let src = m.source(s);
            src.manifest().validate().expect("pinned model qrtz.json");
            src.tree().validate().expect("pinned model tree");
        }
        m
    }

    /// Where the packages live (`<data>`), when `root` isn't `<data>/models`.
    pub fn with_packages_root(mut self, p: impl Into<PathBuf>) -> Self {
        self.packages_root = p.into();
        self
    }

    /// The signed index's root pin and package hosts (tests serve a published index from loopback).
    pub fn with_index(mut self, root_pin: Option<String>, hosts: Vec<String>) -> Self {
        self.index = Some((root_pin, hosts));
        self
    }

    /// Every set is read by an extension's own process: no view dir, loaded from the package (ADR 0157 §9).
    fn extension_read(&self) -> bool {
        !self.manifest.sets.is_empty() && self.manifest.sets.iter().all(|s| reader_of(&s.package).is_some())
    }

    /// ADR 0157 §9: the store-linked package dir an extension loads `set` from, read-only, never a writable copy.
    /// `verify_sha256` re-hashes every file (the install already did, right after linking); without it the check is
    /// modes and sizes.
    pub fn read_dir(&self, set: &str, verify_sha256: bool) -> Option<PathBuf> {
        let s = self.manifest.sets.iter().find(|s| s.id == set)?;
        let ext = reader_of(&s.package)?;
        let dir = self
            .installer()
            .extension_read_path(ext, &s.package, verify_sha256)
            .ok()?;
        let want = self.source(s).version;
        (dir == self.installer().package_path(&s.package, &want)).then_some(dir)
    }

    fn source(&self, s: &ModelSet) -> ModelSource {
        ModelSource {
            package: s.package.clone(),
            version: s
                .package_version
                .clone()
                .unwrap_or_else(|| self.manifest.version.clone()),
            task: s.task.clone(),
            repo: s.repo.clone(),
            revision: s.revision.clone(),
            license: s.license.clone(),
            files: s
                .files
                .iter()
                .map(|f| PinnedFile {
                    remote: f.remote.clone(),
                    path: f.path.clone(),
                    size: f.size,
                    sha256: f.sha256.clone(),
                })
                .collect(),
        }
    }

    /// The `model.*` packages this manifest pins, for `Op::Pinned`.
    pub fn packages(&self) -> Vec<PinnedPackage> {
        // Data carries no signature: the placeholder team stands until the signed index lists the same tree (then
        // the index's team is recorded).
        let team = qrtz_modules::manifest::PLACEHOLDER_TEAM_ID;
        self.manifest
            .sets
            .iter()
            .map(|s| self.source(s).pinned(team))
            .collect()
    }

    fn installer(&self) -> Installer {
        let pin = match &self.index {
            Some((pin, _)) => pin.clone(),
            None => Hosts::pinned().root1_sha256,
        };
        Installer::new(&self.packages_root, pin, Box::new(AppleCodeChecks))
    }

    fn transport(&self, ctl: &DownloadControl) -> HttpTransport {
        let t = match &self.base {
            None => HttpTransport::pinned(),
            // The signed trees name Hugging Face; a test serves those paths from loopback.
            Some(b) => {
                let hosts = self.index.as_ref().map(|(_, h)| h.clone()).unwrap_or_default();
                HttpTransport::new(
                    &hosts,
                    &[(qrtz_modules::models::HUGGING_FACE.to_owned(), b.clone())],
                )
            }
        };
        t.with_control(ctl.clone())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn version(&self) -> &str {
        &self.manifest.version
    }
    pub fn total_size(&self) -> u64 {
        self.manifest.total_size()
    }

    fn set_ok(&self, s: &ModelSet) -> bool {
        if reader_of(&s.package).is_some() {
            return self.read_dir(&s.id, false).is_some();
        }
        let dir = self.root.join(&s.dir);
        std::fs::read_to_string(dir.join(MARKER)).is_ok_and(|v| v.trim() == self.manifest.version)
            && s.files
                .iter()
                .all(|f| std::fs::metadata(dir.join(&f.path)).is_ok_and(|m| m.is_file() && m.len() == f.size))
    }

    /// Every set downloaded, verified and marked for this manifest version (a cheap check: marker + sizes).
    pub fn installed(&self) -> bool {
        self.manifest.sets.iter().all(|s| self.set_ok(s))
    }

    /// Bytes under the models root (finished files), or in the installed packages an extension reads.
    pub fn bytes_on_disk(&self) -> u64 {
        fn walk(d: &Path) -> u64 {
            std::fs::read_dir(d)
                .map(|it| {
                    it.flatten()
                        .map(|e| match e.file_type() {
                            Ok(t) if t.is_dir() => walk(&e.path()),
                            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                            _ => 0,
                        })
                        .sum()
                })
                .unwrap_or(0)
        }
        if self.extension_read() {
            return self
                .manifest
                .sets
                .iter()
                .filter_map(|s| self.read_dir(&s.id, false))
                .map(|d| walk(&d))
                .sum();
        }
        walk(&self.root)
    }

    /// Installs whatever is missing through the package manager (resuming partial blobs; every file checked by
    /// size and sha256 before it enters the store), then links each set's files into its view dir and marks it.
    /// `force` deletes the view dirs and re-checks every stored blob. The caller holds the user's consent.
    pub async fn download(
        &self,
        force: bool,
        ctl: &DownloadControl,
        on_progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<(), SpeechError> {
        if force {
            self.delete()?;
        }
        let installer = self.installer();
        let t = self.transport(ctl);
        if self.manifest.sets.iter().any(|s| reader_of(&s.package).is_some()) {
            // ADR 0157 §6: a model an extension reads needs a fresh signed list, like code. The user's yes to this
            // download is the consent to refresh; if it can't (no pinned root yet, offline), apply refuses below.
            if let Err(e) = installer
                .refresh(&t, &qrtz_modules::install::RefreshConsent::user_asked(), now_ms())
                .await
            {
                if ctl.is_paused() {
                    return Err(SpeechError::Paused);
                }
                tracing::info!(error = %e, "model index refresh failed");
            }
        }
        if !self.extension_read() {
            fsx::mkdir_private(&self.root)?;
        }
        let plan = installer
            .plan(Op::Pinned(self.packages()))
            .map_err(|e| pm_error(e, ctl))?;
        let total = self.total_size();
        let have = total.saturating_sub(plan.download_bytes);
        installer
            .apply(&t, &plan, &plan.consent(), now_ms(), &|done, _| {
                on_progress(Progress {
                    done: (have + done).min(total),
                    total,
                })
            })
            .await
            .map_err(|e| pm_error(e, ctl))?;
        for s in &self.manifest.sets {
            if reader_of(&s.package).is_none() && self.set_ok(s) {
                continue;
            }
            if reader_of(&s.package).is_some() {
                // An older build's writable view of it goes; ctrl never loads from there.
                match qrtz_modules::fsutil::remove_tree(&self.root.join(&s.dir)) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                    _ => {}
                }
                if self.set_ok(s) {
                    continue;
                }
                // Installed and re-verified by `apply`; a failing load check here means it changed since.
                return Err(SpeechError::Checksum(format!(
                    "{}: not read-only or changed",
                    s.package
                )));
            }
            let src = self.source(s);
            let dir = self.root.join(&s.dir);
            let _ = std::fs::remove_file(dir.join(MARKER));
            fsx::mkdir_private(&dir)?;
            for f in &s.files {
                let dest = dir.join(&f.path);
                if let Some(p) = dest.parent() {
                    fsx::mkdir_private(p)?;
                }
                let _ = std::fs::remove_file(&dest);
                installer
                    .store()
                    .link(&f.sha256, f.size, &dest, false)
                    .map_err(|e| SpeechError::Checksum(format!("{}: {e}", f.path)))?;
            }
            debug_assert_eq!(src.files.len(), s.files.len());
            fsx::write_private_atomic(&dir.join(MARKER), self.manifest.version.as_bytes())?;
        }
        on_progress(Progress { done: total, total });
        Ok(())
    }

    /// Removes every set's view dir, drops the pinned requests and prunes what nothing else needs (a model an
    /// installed extension depends on stays). A recording that already loaded a model keeps it until it ends.
    pub fn delete(&self) -> std::io::Result<()> {
        for s in &self.manifest.sets {
            match qrtz_modules::fsutil::remove_tree(&self.root.join(&s.dir)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        let installer = self.installer();
        let names: Vec<String> = self.manifest.sets.iter().map(|s| s.package.clone()).collect();
        let io = |e: InstallError| std::io::Error::other(e.to_string());
        let requested = installer.lock().map_err(io)?.workspace.dependencies;
        let names: Vec<String> = names.into_iter().filter(|n| requested.contains_key(n)).collect();
        if !names.is_empty() {
            let plan = installer.plan(Op::Remove(names)).map_err(io)?;
            let t = HttpTransport::new(&[], &[]);
            futures_lite_block(installer.apply(&t, &plan, &plan.consent(), now_ms(), &|_, _| {}))
                .map_err(io)?;
            installer.prune().map_err(io)?;
        }
        Ok(())
    }
}

/// A remove plan fetches nothing, so its apply completes without awaiting I/O; run it in place (`delete` is called
/// from sync code and inside the runtime alike).
fn futures_lite_block<F: std::future::Future>(f: F) -> F::Output {
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn pm_error(e: InstallError, ctl: &DownloadControl) -> SpeechError {
    if ctl.is_paused() {
        return SpeechError::Paused;
    }
    match e {
        InstallError::Network(m) if m.contains(qrtz_modules::fetch::MISMATCH) => SpeechError::Checksum(m),
        InstallError::Network(m) => SpeechError::Network(m),
        InstallError::Refused(qrtz_modules::sideload::Refusal::RevocationListExpired) => {
            SpeechError::RevocationExpired
        }
        e => SpeechError::Invalid(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_manifest_is_complete_and_safe() {
        let m = Manifest::pinned();
        let ids: Vec<&str> = m.sets.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["asr", "diarizer", "echo", "echo-asr"]);
        // The dirs the helper reads (mac/Models.swift `LocalModels`).
        let dirs: Vec<&str> = m.sets.iter().map(|s| s.dir.as_str()).collect();
        assert_eq!(
            dirs,
            [
                "nemotron-560ms",
                "nemotron3-diarizer",
                "localvqe",
                "parakeet-tdt-v2"
            ]
        );
        assert!(m.base.starts_with("https://"));
        for s in &m.sets {
            assert_eq!(s.revision.len(), 40, "{}: a pinned commit, not a branch", s.id);
            for f in &s.files {
                assert!(safe_rel(&f.path) && safe_rel(&f.remote), "{}", f.path);
                assert_eq!(f.sha256.len(), 64, "{}", f.path);
                assert!(f.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            }
        }
        // What the helper loads must be there.
        let has = |set: &str, p: &str| {
            m.sets
                .iter()
                .find(|s| s.id == set)
                .unwrap()
                .files
                .iter()
                .any(|f| f.path == p)
        };
        assert!(has("asr", "encoder/encoder_int8.mlmodelc/weights/weight.bin"));
        assert!(has("asr", "decoder.mlmodelc/coremldata.bin"));
        assert!(has("asr", "joint.mlmodelc/coremldata.bin"));
        assert!(has("asr", "tokenizer.json"));
        assert!(has("asr", "metadata.json"));
        assert!(has(
            "diarizer",
            "Nemotron3Diarizer_fast32.mlmodelc/coremldata.bin"
        ));
        assert!(has("diarizer", "learnable_sil_emb.bin"));
        assert!(has("echo", "localvqe-v1.3-4.8M-16ms.mlmodelc/coremldata.bin"));
        // Echo's recogniser (Parakeet TDT v2, FluidAudio `AsrModels.loadLocal(version: .v2)`): its four models and
        // the vocabulary.
        for model in ["Preprocessor", "Encoder", "Decoder", "JointDecision"] {
            assert!(
                has("echo-asr", &format!("{model}.mlmodelc/coremldata.bin")),
                "{model}"
            );
            assert!(
                has("echo-asr", &format!("{model}.mlmodelc/weights/weight.bin")),
                "{model}"
            );
        }
        assert!(has("echo-asr", "parakeet_vocab.json"));
        let total = m.total_size();
        assert!((1_250_000_000..1_350_000_000).contains(&total), "{total}");
    }

    #[test]
    fn unsafe_paths_are_refused() {
        for p in ["../x", "/etc/passwd", "a/../../b", ""] {
            assert!(!safe_rel(p), "{p}");
        }
        assert!(safe_rel("encoder/encoder_int8.mlmodelc/weights/weight.bin"));
    }
}
