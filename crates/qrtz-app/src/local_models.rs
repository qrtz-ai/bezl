//! ADR 0079 / ADR 0157: the Mac helper's own speech models (Nemotron ASR for recordings, Nemotron 3 diarization,
//! LocalVQE echo cancellation, Parakeet TDT v2 for echo dictation) for builds without the signed speech module. Each
//! set is a `model.*` package whose tree (every file's path, size and sha256, and its Hugging Face URL at a fixed
//! revision) is pinned in this binary (`local_models.json`). The download is the package manager's: the user's yes
//! (settings → speech, or switching echo or nano on in the module manager; ADR 0163) asks, `Op::Pinned` plans it, and the one fetcher takes each file from the package hosts
//! (`modules.qrtz.ai`, then its mirrors) or else the pinned Hugging Face URL, verified by the same sha256 into the
//! content-addressed store, then installed read-only under `<data>/packages/`. The helper's view is unchanged:
//! `<data>/models/<set>/` (0700) holds store links of the files, and a set is usable once its `.qrtz-verified` marker
//! is written; the helper reads only marked sets (`QRTZ_MODELS_DIR`) and never fetches anything itself.
//!
//! A model an installed extension declares it reads in its own process (`extension.reads`: ctrl's `model.laya`,
//! nano's Nemotron, diarizer and LocalVQE; ADR 0212) gets no view: it is loaded
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

#[derive(Clone)]
pub struct LocalModels {
    root: PathBuf,
    packages_root: PathBuf,
    manifest: Manifest,
    /// The Hugging Face host actually reached (tests: a loopback server); `None` = the pinned hosts.
    base: Option<String>,
    /// The root pin and package hosts for the signed index (tests: loopback); `None` = `Hosts::pinned()`.
    index: Option<(Option<String>, Vec<String>)>,
    /// How long a download that needs bytes waits for the package manager's install lock (N189, ADR 0213).
    busy_wait: std::time::Duration,
}

/// N189 (ADR 0213): how long a model download waits for another install, update or refresh to let go of
/// `.qrtz-install.lock` before it reports "another download or update is running", and how often it looks.
pub const BUSY_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
const BUSY_POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// The model set dirs extension `ext` needs: its bundled manifest's `model:<dir>` data dependencies (echo:
/// Parakeet; nano: Nemotron, the diarizer and LocalVQE). Unknown extensions need none (audit P0-3).
pub fn sets_needed_by(ext: &str) -> Vec<String> {
    crate::modules::bundled()
        .manifests
        .iter()
        .filter(|m| m.id == ext)
        .flat_map(|m| &m.dependencies)
        .filter_map(|d| d.id.strip_prefix("model:"))
        .map(str::to_owned)
        .collect()
}

/// The `model.*` packages a bundled extension runs (echo: `model.parakeet-tdt-v2-en`): its manifest's `model:<dir>`
/// sets, as the pinned manifest names them. A bundled extension's engine is these and only these (ADR 0174/0175).
pub fn packages_needed_by(ext: &str) -> Vec<String> {
    let dirs = sets_needed_by(ext);
    let mut out: Vec<String> = Manifest::pinned()
        .sets
        .into_iter()
        .filter(|s| dirs.contains(&s.dir))
        .map(|s| s.package)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Which of this build's modules a `model.*` package the app pinned is for (ADR 0163: switching echo or nano on
/// pins their models as workspace requests, and a Mac upgrading from before the package manager adopts its
/// downloads the same way): every bundled module whose manifest declares a `model:<dir>` dependency on one of the
/// pinned sets that is that package (`model.parakeet-tdt-v2-en` → echo; `model.nemotron`, `model.nemotron-diarizer`,
/// `model.localvqe` → nano; `model.laya` → ctrl), plus an extension that reads it in its own process. This is
/// what [`crate::module_hosting::Packages::list`] passes to [`qrtz_modules::qrtz_lock::QrtzLock::users`].
pub fn pinned_users(package: &str) -> Vec<String> {
    static SETS: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    let sets = SETS.get_or_init(|| {
        let ctrl = crate::ctrl_model::CtrlManifest::pinned().manifest;
        Manifest::pinned()
            .sets
            .into_iter()
            .chain(ctrl.sets)
            .map(|s| (s.package, s.dir))
            .collect()
    });
    let mut out: Vec<String> = crate::modules::bundled()
        .manifests
        .iter()
        .filter(|m| {
            m.dependencies.iter().any(|d| {
                d.id.strip_prefix("model:")
                    .is_some_and(|dir| sets.iter().any(|(p, sd)| p == package && sd == dir))
            })
        })
        .map(|m| m.id.clone())
        .collect();
    out.sort();
    out.dedup();
    out
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
            busy_wait: BUSY_WAIT,
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

    /// Only the sets `extensions` need ([`sets_needed_by`]), with the same roots and hosts: what onboarding or
    /// settings downloads for the installed/selected extensions, instead of every pinned set (audit P0-3).
    pub fn for_extensions<S: AsRef<str>>(&self, extensions: &[S]) -> LocalModels {
        let dirs: Vec<String> = extensions
            .iter()
            .flat_map(|e| sets_needed_by(e.as_ref()))
            .collect();
        let mut manifest = self.manifest.clone();
        manifest.sets.retain(|s| dirs.contains(&s.dir));
        LocalModels {
            root: self.root.clone(),
            packages_root: self.packages_root.clone(),
            manifest,
            base: self.base.clone(),
            index: self.index.clone(),
            busy_wait: self.busy_wait,
        }
    }

    /// This view without the sets in `dirs` (ADR 0175: a module's delete keeps what another switched-on module needs).
    pub fn without_sets(mut self, dirs: &[String]) -> LocalModels {
        self.manifest.sets.retain(|s| !dirs.contains(&s.dir));
        self
    }

    /// The ids of the sets this view covers (`asr`, `diarizer`, `echo`, `echo-asr`).
    pub fn set_ids(&self) -> Vec<&str> {
        self.manifest.sets.iter().map(|s| s.id.as_str()).collect()
    }

    /// The signed index's root pin and package hosts (tests serve a published index from loopback).
    pub fn with_index(mut self, root_pin: Option<String>, hosts: Vec<String>) -> Self {
        self.index = Some((root_pin, hosts));
        self
    }

    /// Tests: how long a download waits for the install lock ([`BUSY_WAIT`] otherwise).
    pub fn with_busy_wait(mut self, wait: std::time::Duration) -> Self {
        self.busy_wait = wait;
        self
    }

    /// The installed extension whose own process declares it reads `package` (`extension.reads` in `qrtz.lock`, ADR
    /// 0212): its set gets no view dir and is loaded read-only from the store-linked package.
    /// Before that extension is installed, this build's manifest of a downloadable module that declares it
    /// ([`qrtz_modules::bundled::declared_reads`]; a module bundled in qrtz.app keeps its view for now).
    fn reader_of(&self, package: &str) -> Option<String> {
        let installed = self.installer().lock().ok().and_then(|lock| {
            lock.workspace
                .resolved
                .keys()
                .find(|e| {
                    qrtz_modules::resolve::grants_in_lock(&lock, e)
                        .is_ok_and(|g| g.iter().any(|g| g.own_process && g.model == package))
                })
                .cloned()
        });
        installed.or_else(|| {
            qrtz_modules::bundled::declared_reads()
                .into_iter()
                .find(|(_, r)| r.iter().any(|m| m == package))
                .map(|(e, _)| e)
        })
    }

    /// Every set is read by an extension's own process: no view dir, loaded from the package (ADR 0212).
    fn extension_read(&self) -> bool {
        !self.manifest.sets.is_empty()
            && self
                .manifest
                .sets
                .iter()
                .all(|s| self.reader_of(&s.package).is_some())
    }

    /// ADR 0157 §9: the store-linked package dir an extension loads `set` from, read-only, never a writable copy.
    /// `verify_sha256` re-hashes every file (the install already did, right after linking); without it the check is
    /// modes and sizes.
    pub fn read_dir(&self, set: &str, verify_sha256: bool) -> Option<PathBuf> {
        let s = self.manifest.sets.iter().find(|s| s.id == set)?;
        let ext = self.reader_of(&s.package)?;
        let dir = self
            .installer()
            .extension_read_path(&ext, &s.package, verify_sha256)
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
        // Data carries no signature; the publisher team is the release team, the one the signed index lists for
        // every model (scripts/modules/publish.sh). An already-installed pin keeps its recorded entry.
        let team = qrtz_modules::manifest::RELEASE_TEAM_ID;
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
        if self.reader_of(&s.package).is_some() {
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
        if !force && self.up_to_date(&installer) {
            // Backlog P3 (ADR 0210): already installed and verified, nothing to fetch, link or record. No transport,
            // no index refresh and no install lock: before, the empty install took `.qrtz-install.lock` and failed
            // "another install is running" whenever an index refresh stuck on a dead network held it.
            let total = self.total_size();
            on_progress(Progress { done: total, total });
            return Ok(());
        }
        let t = self.transport(ctl);
        if self
            .manifest
            .sets
            .iter()
            .any(|s| self.reader_of(&s.package).is_some())
        {
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
        // N189 (ADR 0213): another install, update or refresh holding `.qrtz-install.lock` is waited for (apply
        // takes the lock before it does anything), re-planning each time since that install may have changed
        // `qrtz.lock`; a pause ends the wait at once. Before, the first `Busy` failed the download as "the download
        // failed its signature check".
        let total = self.total_size();
        let deadline = std::time::Instant::now() + self.busy_wait;
        let mut waited = false;
        loop {
            let plan = installer
                .plan(Op::Pinned(self.packages()))
                .map_err(|e| pm_error(e, ctl))?;
            let have = total.saturating_sub(plan.download_bytes);
            let r = installer
                .apply(&t, &plan, &plan.consent(), now_ms(), &|done, _| {
                    on_progress(Progress {
                        done: (have + done).min(total),
                        total,
                    })
                })
                .await;
            // A plan made just before the other install committed is stale once it lets go: plan again.
            let busy = matches!(r, Err(InstallError::Busy));
            let stale_after_wait = waited && matches!(r, Err(InstallError::StalePlan));
            match r {
                Ok(_) => break,
                Err(_)
                    if (busy || stale_after_wait)
                        && std::time::Instant::now() < deadline
                        && !ctl.is_paused() =>
                {
                    waited = true;
                    let end = std::time::Instant::now() + BUSY_POLL;
                    while std::time::Instant::now() < end && !ctl.is_paused() {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                }
                Err(e) => return Err(pm_error(e, ctl)),
            }
        }
        for s in &self.manifest.sets {
            if self.reader_of(&s.package).is_none() && self.set_ok(s) {
                continue;
            }
            if self.reader_of(&s.package).is_some() {
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
            self.link_view(&installer, s)?;
        }
        on_progress(Progress { done: total, total });
        Ok(())
    }

    /// Backlog P3 (ADR 0210): every set is installed and verified (its view marked, sizes right) and `qrtz.lock`
    /// already holds exactly what the pinned plan would write, so a download has nothing to do. Disk only. A set an
    /// extension reads in its own process (ctrl's Laya, nano's models; ADR 0212) is never "up to date" here: its
    /// download keeps ADR 0157 §6's refresh first.
    fn up_to_date(&self, installer: &Installer) -> bool {
        if self.manifest.sets.is_empty()
            || self
                .manifest
                .sets
                .iter()
                .any(|s| self.reader_of(&s.package).is_some())
            || !self.installed()
        {
            return false;
        }
        let Ok(lock) = installer.lock() else { return false };
        installer
            .plan(Op::Pinned(self.packages()))
            .is_ok_and(|p| p.is_noop() && p.fetch.is_empty() && p.next == lock)
    }

    /// The helper's view of set `s`: store links of its files under `<models>/<dir>/`, then the marker.
    fn link_view(&self, installer: &Installer, s: &ModelSet) -> Result<(), SpeechError> {
        let dir = self.root.join(&s.dir);
        let _ = std::fs::remove_file(dir.join(MARKER));
        fsx::mkdir_private(&self.root)?;
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
        fsx::write_private_atomic(&dir.join(MARKER), self.manifest.version.as_bytes())?;
        Ok(())
    }

    /// ADR 0169: a module's models arrive with the module, in the same `packages:apply` (one consent, one
    /// progress, all or nothing), so the helper's views follow `qrtz.lock`: every set whose package the lock holds
    /// gets its view linked (no download: the files are already in the store), and a set the lock no longer holds
    /// (its module was removed and nothing else uses it) loses its view. Sets an extension reads in its own process
    /// (ctrl's Laya) have no view. A set that can't be linked is skipped and reported.
    ///
    /// First, on the upgrade from before ADR 0169 (Bobs and Bozniak, 902bcab review): a model the app itself pinned
    /// for a module (an `Op::Pinned` workspace request, ADR 0163, or one adopted from the old downloader) is dropped
    /// when none of the modules it is for is installed, so an empty qrtz doesn't keep (or link) echo's and nano's
    /// models for modules that aren't there. A model an installed module uses is never dropped, and a request the
    /// user made (`qrtz install model.x`, from the index) is never touched ([`orphaned_app_requests`]).
    pub fn sync_views(&self) -> Vec<String> {
        let installer = self.installer();
        let mut lock = match installer.lock() {
            Ok(l) => l,
            Err(e) => return vec![format!("qrtz.lock: {e}")],
        };
        let mut problems = vec![];
        let orphans = orphaned_app_requests(&lock);
        if !orphans.is_empty() {
            tracing::info!(
                ?orphans,
                "dropping the app's model requests for modules that aren't installed"
            );
            match remove_requests(&installer, orphans).and_then(|()| installer.lock()) {
                Ok(l) => lock = l,
                Err(e) => problems.push(format!("old model requests: {e}")),
            }
        }
        for s in &self.manifest.sets {
            if self.reader_of(&s.package).is_some() {
                // ADR 0212: its reader loads it from the store; a view made before that extension declared it goes.
                match qrtz_modules::fsutil::remove_tree(&self.root.join(&s.dir)) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        problems.push(format!("{}: {e}", s.dir))
                    }
                    _ => {}
                }
                continue;
            }
            let version = self.source(s).version;
            let dir = self.root.join(&s.dir);
            if lock.get(&s.package, &version).is_none() {
                match qrtz_modules::fsutil::remove_tree(&dir) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        problems.push(format!("{}: {e}", s.dir))
                    }
                    _ => {}
                }
                continue;
            }
            if self.set_ok(s) {
                continue;
            }
            if let Err(e) = self.link_view(&installer, s) {
                problems.push(format!("{}: {e}", s.package));
            }
        }
        problems
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
            remove_requests(&installer, names).map_err(io)?;
        }
        Ok(())
    }
}

/// Drop workspace requests `names` (no network: a remove plan fetches nothing) and prune what nothing needs now.
fn remove_requests(installer: &Installer, names: Vec<String>) -> Result<(), InstallError> {
    let plan = installer.plan(Op::Remove(names))?;
    let t = HttpTransport::new(&[], &[]);
    futures_lite_block(installer.apply(&t, &plan, &plan.consent(), now_ms(), &|_, _| {}))?;
    installer.prune()?;
    Ok(())
}

/// The workspace requests the app made for modules that aren't installed (see [`LocalModels::sync_views`]): a
/// `model.*` request whose resolved package came from the binary's pins (`source: pinned`, never a user's index
/// install), that [`pinned_users`] says is for some module, when none of those modules is an installed extension
/// and no installed extension depends on it through the lock either (an engine swap, ADR 0157 §5).
pub fn orphaned_app_requests(lock: &qrtz_modules::qrtz_lock::QrtzLock) -> Vec<String> {
    use qrtz_modules::index::PackageKind;
    let installed = crate::installed::installed_extensions(lock);
    lock.workspace
        .dependencies
        .keys()
        .filter(|n| {
            let Some(v) = lock.workspace.resolved.get(*n) else {
                return false;
            };
            let Some(l) = lock.get(n, v) else {
                return false;
            };
            if l.kind != PackageKind::Model || l.source != qrtz_modules::qrtz_lock::Source::Pinned {
                return false;
            }
            let for_modules = pinned_users(n);
            !for_modules.is_empty()
                && !for_modules.iter().any(|m| installed.contains(m))
                && !lock
                    .users(n, v, &|_| vec![])
                    .iter()
                    .any(|u| installed.contains(u))
        })
        .cloned()
        .collect()
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
        InstallError::Busy => SpeechError::InstallBusy,
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

    /// P0-3: each extension's manifest names exactly the sets it loads, and together they cover every pinned set.
    #[test]
    fn each_extension_needs_only_its_own_sets() {
        assert_eq!(sets_needed_by("echo"), ["parakeet-tdt-v2"]);
        assert_eq!(
            sets_needed_by("nano"),
            ["nemotron-560ms", "nemotron3-diarizer", "localvqe"]
        );
        // ctrl's Laya is its own manifest (ctrl_model.rs), not one of these sets.
        assert!(sets_needed_by("nobody").is_empty());
        let tmp = tempfile::tempdir().unwrap();
        let all = LocalModels::new(tmp.path().join("models"), None);
        let echo = all.for_extensions(&["echo"]);
        assert_eq!(echo.set_ids(), ["echo-asr"]);
        let echo_bytes = echo.total_size();
        assert!((400_000_000..520_000_000).contains(&echo_bytes), "{echo_bytes}");
        let nano = all.for_extensions(&["nano"]);
        assert_eq!(nano.set_ids(), ["asr", "diarizer", "echo"]);
        assert_eq!(echo_bytes + nano.total_size(), all.total_size());
        assert_eq!(all.for_extensions(&["echo", "nano"]).set_ids(), all.set_ids());
        let none = all.for_extensions::<&str>(&[]);
        assert_eq!(none.total_size(), 0);
        assert!(none.installed(), "nothing selected: nothing to download");
    }

    /// Jared's "used by nothing": each pinned model names the module it is for.
    #[test]
    fn each_pinned_model_names_its_module() {
        assert_eq!(pinned_users("model.parakeet-tdt-v2-en"), ["echo"]);
        for m in ["model.nemotron", "model.nemotron-diarizer", "model.localvqe"] {
            assert_eq!(pinned_users(m), ["nano"], "{m}");
        }
        assert_eq!(pinned_users("model.laya"), ["ctrl"]);
        assert!(pinned_users("model.nobody").is_empty());
        // Every pinned set is some module's.
        for s in Manifest::pinned().sets {
            assert!(!pinned_users(&s.package).is_empty(), "{}", s.package);
        }
    }

    #[test]
    fn unsafe_paths_are_refused() {
        for p in ["../x", "/etc/passwd", "a/../../b", ""] {
            assert!(!safe_rel(p), "{p}");
        }
        assert!(safe_rel("encoder/encoder_int8.mlmodelc/weights/weight.bin"));
    }
}
