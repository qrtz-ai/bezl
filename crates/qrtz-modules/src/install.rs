//! The shell's module installer (ADR 0153): verified, consented, resumable, atomic.
//!
//! The installer itself has no network code. Bytes come through a [`Transport`]: the shell's HTTPS transport
//! (qrtz-app `module_hosting`, the primary host then its mirrors), or a `.qrtzpkg` file ([`PkgTransport`]). Both
//! paths run the same checks in the same order:
//!
//! 1. **Trust.** Roots chain from the pinned root; the timestamp and the index it names are signed and not older
//!    than stored (freeze and rollback). A network install refuses an expired timestamp; a sideload reports it.
//! 2. **Plan.** The module's index entry and every dependency it pins (`requires`, which must equal its
//!    `module.json`), each matched against the index *and* the lockfile: same kind, sha256, team id, bundle id and
//!    requirement, and an installed version never gets other bytes.
//! 3. **Consent.** Nothing is fetched without an [`InstallConsent`] for exactly this plan (module, version, index,
//!    bytes); a refresh needs a [`RefreshConsent`]. There is no other network use.
//! 4. **Fetch.** Missing blobs only, resuming partial ones; each is size-capped and sha256-checked by the transport
//!    and again here. A failed host is skipped for the next one (mirrors are untrusted transport).
//! 5. **Stage.** Files are assembled from their blobs in `staging/` and checked again, whole.
//! 6. **Code checks** ([`CodeChecks`]: Apple signature, team id, designated requirement, notarization) on the
//!    staged module and each staged xpc-service, then [`crate::sideload::decide`] (Bozniak's seven rules).
//! 7. **Commit.** Staged dirs are renamed into `modules/<id>/<version>` and `packages/<kind>/<name>/<version>`,
//!    made read-only, and the lockfile is written last (temp + rename): that rename is the install. A crash before
//!    it leaves only dirs the lockfile doesn't name, which [`Installer::sweep`] removes.
use crate::index::{
    layout, Chunk, Index, IndexError, PackageEntry, PackageKind, MAX_META_BYTES, MODULE_MANIFEST_FILE,
};
use crate::lock::{package_dir, LockedPackage, Lockfile, PinnedRef, LOCK_FILE};
use crate::manifest::{DependencyKind, ModuleManifest};
use crate::pkg::Qrtzpkg;
use crate::sideload::{
    decide, recheck_after_fresh_list, Candidate, Confirmation, Notice, PackageChecks, Refusal, Request,
    SideloadOrigin, VerifiedSnapshot,
};
use crate::trust::{Mode, Trust, Verified, TRUST_FILE};
use crate::{fsutil, ModuleError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where everything lives: `~/Library/Application Support/qrtz/modules` on macOS.
pub fn default_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/qrtz/modules"))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("the consent doesn't match this download (asked again)")]
    ConsentMismatch,
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error("every host failed: {0}")]
    Network(String),
    #[error("{id}@{version} is not in the signed index")]
    NotInIndex { id: String, version: String },
    #[error("{id}@{version}: {field} differs between the index and the module's pin")]
    PinMismatch {
        id: String,
        version: String,
        field: &'static str,
    },
    #[error("module.json doesn't match its index entry: {0}")]
    ManifestMismatch(String),
    #[error("{0}: missing from the file (a sideload brings every dependency it needs)")]
    MissingFromFile(String),
    #[error("{0}")]
    Refused(Refusal),
    #[error("no index yet; refresh first")]
    NoIndex,
    #[error(transparent)]
    Module(#[from] ModuleError),
    #[error("io: {0}")]
    Io(String),
}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<Refusal> for InstallError {
    fn from(r: Refusal) -> Self {
        Self::Refused(r)
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A source of layout paths: the HTTPS hosts (primary first, then mirrors) or a `.qrtzpkg`.
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    fn hosts(&self) -> usize;
    /// A small document; `Ok(None)` when the host doesn't have it.
    async fn get(&self, host: usize, path: &str, max: usize) -> Result<Option<Vec<u8>>, String>;
    /// A document whose size and sha256 are already known from a signed one (the index, named by the
    /// timestamp): the transport may fetch it resumably and must not return other bytes.
    async fn get_pinned(&self, host: usize, path: &str, size: u64, sha256: &str) -> Result<Vec<u8>, String> {
        let b = self
            .get(host, path, size as usize)
            .await?
            .ok_or_else(|| format!("{path}: not found"))?;
        if b.len() as u64 != size || crate::sha256_hex(&b) != sha256 {
            return Err(format!("{path}: doesn't match the timestamp"));
        }
        Ok(b)
    }
    /// Every blob into `dir/<sha256>`, resuming `dir/<sha256>.part`, each capped at its size and checked against
    /// its sha256 before it is renamed into place.
    async fn get_blobs(
        &self,
        host: usize,
        blobs: &[Chunk],
        dir: &Path,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), String>;
}

/// Rule 7's Apple half, on the staged bytes: the module `.app` (or the helper) against its manifest's code
/// identity, and each xpc-service against its pinned identity. qrtz-app implements it with `codesign` /
/// `SecStaticCodeCheckValidity` and the notarization ticket on macOS.
pub trait CodeChecks: Send + Sync {
    fn check_module(&self, dir: &Path, m: &ModuleManifest) -> Result<(), String>;
    fn check_service(
        &self,
        dir: &Path,
        bundle_id: &str,
        team_id: &str,
        requirement: &str,
    ) -> Result<(), String>;
}

/// The user said yes to checking for modules (onboarding or settings). Required for any network refresh.
#[derive(Clone, Copy, Debug)]
pub struct RefreshConsent(());

impl RefreshConsent {
    /// Only the IPC handler of a user action calls this.
    pub fn user_asked() -> Self {
        Self(())
    }
}

/// The user said yes to this download, after seeing its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallConsent {
    pub module: String,
    pub version: String,
    pub index_sha256: String,
    pub download_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedPackage {
    pub id: String,
    pub version: String,
    pub entry: PackageEntry,
    /// Already installed with the same content: nothing to fetch.
    pub installed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub module: String,
    pub version: String,
    pub index_sha256: String,
    pub packages: Vec<PlannedPackage>,
    /// Blobs still to fetch (not installed, not already in the download cache).
    pub fetch: Vec<Chunk>,
    pub download_bytes: u64,
    pub total_bytes: u64,
}

impl Plan {
    /// What the consent prompt shows and the user's yes carries back.
    pub fn consent(&self) -> InstallConsent {
        InstallConsent {
            module: self.module.clone(),
            version: self.version.clone(),
            index_sha256: self.index_sha256.clone(),
            download_bytes: self.download_bytes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub module: String,
    pub version: String,
    pub stale_revocation: bool,
    /// Modules a fresh list disabled in the same step (rule 6).
    pub notices: Vec<Notice>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refreshed {
    pub timestamp_version: u64,
    pub index_version: u64,
    pub notices: Vec<Notice>,
}

pub struct Installer {
    root: PathBuf,
    pinned_root_sha256: Option<String>,
    checks: Box<dyn CodeChecks>,
}

const CACHE: &str = "cache";
const BLOBS: &str = "cache/blobs";
const STAGING: &str = "staging";

fn module_dir(id: &str, version: &str) -> String {
    format!("modules/{id}/{version}")
}

/// Where a package lands: modules under `modules/`, everything else under `packages/` (the lockfile's paths).
fn install_dir(id: &str, version: &str, kind: PackageKind) -> String {
    match kind {
        PackageKind::Module => module_dir(id, version),
        _ => package_dir(id, version),
    }
}

fn dep_kind(k: PackageKind) -> Option<DependencyKind> {
    match k {
        PackageKind::Data => Some(DependencyKind::Data),
        PackageKind::XpcService => Some(DependencyKind::XpcService),
        _ => None,
    }
}

impl Installer {
    pub fn new(
        root: impl Into<PathBuf>,
        pinned_root_sha256: Option<String>,
        checks: Box<dyn CodeChecks>,
    ) -> Self {
        Self {
            root: root.into(),
            pinned_root_sha256,
            checks,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }
    pub fn trust_path(&self) -> PathBuf {
        self.root.join(TRUST_FILE)
    }
    pub fn lock(&self) -> Result<Lockfile, InstallError> {
        Ok(Lockfile::load(&self.lock_path())?)
    }
    pub fn trust(&self) -> Result<Option<Trust>, InstallError> {
        Ok(Trust::load(&self.trust_path())?)
    }

    /// Bootstrap from (or walk) the roots `src` has, stopping at the first it doesn't.
    async fn roots(
        &self,
        src: &dyn Transport,
        host: usize,
        trust: Option<Trust>,
    ) -> Result<Trust, InstallError> {
        let mut t = match trust {
            Some(t) => t,
            None => {
                self.pinned_root_sha256.as_ref().ok_or(IndexError::NoPinnedRoot)?;
                let p = layout::root(1);
                let (Some(b), Some(s)) = (
                    src.get(host, &p, MAX_META_BYTES)
                        .await
                        .map_err(InstallError::Network)?,
                    src.get(host, &layout::sig(&p), MAX_META_BYTES)
                        .await
                        .map_err(InstallError::Network)?,
                ) else {
                    return Err(InstallError::Network(format!("{p}: not found")));
                };
                Trust::bootstrap(self.pinned_root_sha256.as_deref(), &b, &s)?
            }
        };
        for _ in 0..1000 {
            let p = t.next_root_path();
            let Some(b) = src
                .get(host, &p, MAX_META_BYTES)
                .await
                .map_err(InstallError::Network)?
            else {
                break;
            };
            let s = src
                .get(host, &layout::sig(&p), MAX_META_BYTES)
                .await
                .map_err(InstallError::Network)?
                .ok_or_else(|| InstallError::Network(format!("{p}.sig: not found")))?;
            t.update_root(&b, &s)?;
        }
        Ok(t)
    }

    async fn pair(
        &self,
        src: &dyn Transport,
        host: usize,
        t: &Trust,
        mode: Mode,
        now_ms: u64,
    ) -> Result<(Verified, Vec<u8>, Vec<u8>), InstallError> {
        let tp = layout::timestamp();
        let get = |p: String| async move {
            src.get(host, &p, crate::index::MAX_INDEX_BYTES)
                .await
                .map_err(InstallError::Network)?
                .ok_or(InstallError::Network(format!("{p}: not found")))
        };
        let ts = get(tp.clone()).await?;
        let ts_sig = get(layout::sig(&tp)).await?;
        let stamp = t.verify_timestamp(mode, &ts, &ts_sig, now_ms)?;
        let ip = layout::index(&stamp.index.sha256);
        let ix = src
            .get_pinned(host, &ip, stamp.index.size, &stamp.index.sha256)
            .await
            .map_err(InstallError::Network)?;
        let ix_sig = get(layout::sig(&ip)).await?;
        let v = t.verify_pair(mode, &ts, &ts_sig, &ix, &ix_sig, now_ms)?;
        Ok((v, ts, ts_sig))
    }

    fn save_index(&self, v: &Verified, ts: &[u8], ts_sig: &[u8]) -> Result<(), InstallError> {
        let c = self.root.join(CACHE);
        fsutil::write_private_atomic(&c.join("timestamp.json"), ts)?;
        fsutil::write_private_atomic(&c.join("timestamp.json.sig"), ts_sig)?;
        fsutil::write_private_atomic(&c.join("index.json"), &v.index_bytes)?;
        fsutil::write_private_atomic(&c.join("index.json.sig"), &v.index_sig)?;
        Ok(())
    }

    /// The cached index, re-verified against the stored trust (signatures and the stored hash; not freshness).
    pub fn cached_index(&self) -> Result<(Trust, Verified), InstallError> {
        let t = self.trust()?.ok_or(InstallError::NoIndex)?;
        let c = self.root.join(CACHE);
        let r = |n: &str| std::fs::read(c.join(n)).map_err(|_| InstallError::NoIndex);
        let v = t.verify_pair(
            Mode::Sideload,
            &r("timestamp.json")?,
            &r("timestamp.json.sig")?,
            &r("index.json")?,
            &r("index.json.sig")?,
            0,
        )?;
        if t.state.index.as_ref().map(|s| &s.sha256) != Some(&v.timestamp.index.sha256) {
            return Err(InstallError::NoIndex);
        }
        Ok((t, v))
    }

    /// Network refresh: roots, then the newest fresh timestamp and its index from the first host that has one.
    /// A host that serves something stale, expired or badly signed is skipped for the next (the freeze attack
    /// costs the attacker every host); when all fail, the trust state is unchanged. Then every installed module
    /// is rechecked against the fresh list (rule 6).
    pub async fn refresh(
        &self,
        src: &dyn Transport,
        _consent: &RefreshConsent,
        now_ms: u64,
    ) -> Result<Refreshed, InstallError> {
        let stored = self.trust()?;
        let mut errs = vec![];
        for host in 0..src.hosts() {
            let attempt = async {
                let mut t = self.roots(src, host, stored.clone()).await?;
                t.check_root_fresh(now_ms)?;
                let (v, ts, ts_sig) = self.pair(src, host, &t, Mode::Network, now_ms).await?;
                t.commit(&v, now_ms)?;
                Ok::<_, InstallError>((t, v, ts, ts_sig))
            };
            match attempt.await {
                Ok((t, v, ts, ts_sig)) => {
                    self.save_index(&v, &ts, &ts_sig)?;
                    t.save(&self.trust_path())?;
                    let notices = self.recheck(&t, &v, now_ms)?;
                    return Ok(Refreshed {
                        timestamp_version: v.timestamp.version,
                        index_version: v.index.version,
                        notices,
                    });
                }
                Err(e) => errs.push(e),
            }
        }
        // The most telling refusal wins over plain network errors (a freeze looks like "expired" or "rollback").
        let pick = errs
            .iter()
            .find(|e| matches!(e, InstallError::Index(_)))
            .or(errs.first())
            .cloned();
        Err(pick.unwrap_or(InstallError::Network("no hosts".into())))
    }

    fn recheck(&self, t: &Trust, v: &Verified, now_ms: u64) -> Result<Vec<Notice>, InstallError> {
        let snap = VerifiedSnapshot::verify(
            &t.carried(&v.timestamp, v.timestamp_sha256.clone()),
            &v.index_bytes,
            &v.index_sig,
        )?;
        let lock = self.lock()?;
        let (next, notices) = recheck_after_fresh_list(&lock, &snap, t.effective_now(now_ms));
        if next != lock {
            next.save(&self.lock_path())?;
        }
        Ok(notices)
    }

    /// What installing `module` (at `version`, or the newest unrevoked) would fetch, from the cached index.
    pub fn plan(&self, module: &str, version: Option<&str>) -> Result<Plan, InstallError> {
        let (_, v) = self.cached_index()?;
        self.plan_from(&v.index, &v.timestamp.index.sha256, module, version)
    }

    fn plan_from(
        &self,
        index: &Index,
        index_sha256: &str,
        module: &str,
        version: Option<&str>,
    ) -> Result<Plan, InstallError> {
        let (ver, entry) = match version {
            Some(v) => (v, index.entry(module, v)),
            None => match index.latest(module) {
                Some((v, e)) => (v, Some(e)),
                None => ("latest", None),
            },
        };
        let Some(entry) = entry.filter(|e| e.kind == PackageKind::Module) else {
            return Err(InstallError::NotInIndex {
                id: module.into(),
                version: ver.into(),
            });
        };
        let lock = self.lock()?;
        let module_installed = lock
            .modules
            .get(module)
            .is_some_and(|m| m.version == ver && Some(&m.manifest_sha256) == entry.manifest_sha256.as_ref());
        let mut packages = vec![PlannedPackage {
            id: module.into(),
            version: ver.into(),
            entry: entry.clone(),
            installed: module_installed,
        }];
        for r in &entry.requires {
            let Some(e) = index.entry(&r.id, &r.version) else {
                return Err(InstallError::NotInIndex {
                    id: r.id.clone(),
                    version: r.version.clone(),
                });
            };
            let installed = match lock.package(&r.id, &r.version) {
                Some(have) if have.sha256 == e.sha256 => true,
                Some(_) => {
                    return Err(ModuleError::ContentChanged {
                        id: r.id.clone(),
                        version: r.version.clone(),
                    }
                    .into())
                }
                None => false,
            };
            packages.push(PlannedPackage {
                id: r.id.clone(),
                version: r.version.clone(),
                entry: e.clone(),
                installed,
            });
        }
        let cache = self.root.join(BLOBS);
        let mut seen = std::collections::BTreeSet::new();
        let mut fetch = vec![];
        let mut total = 0;
        for p in packages.iter().filter(|p| !p.installed) {
            total += p.entry.size;
            for c in p.entry.blobs() {
                if seen.insert(c.sha256.clone())
                    && !fsutil::sha256_file(&cache.join(&c.sha256))
                        .is_ok_and(|(n, h)| n == c.size && h == c.sha256)
                {
                    fetch.push(c);
                }
            }
        }
        let download_bytes = fetch.iter().map(|c| c.size).sum::<u64>();
        Ok(Plan {
            module: module.into(),
            version: ver.into(),
            index_sha256: index_sha256.into(),
            packages,
            fetch,
            download_bytes,
            total_bytes: total,
        })
    }

    async fn fetch(
        &self,
        src: &dyn Transport,
        plan: &Plan,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), InstallError> {
        let cache = self.root.join(BLOBS);
        fsutil::mkdir_private(&cache)?;
        let mut errs = vec![];
        for host in 0..src.hosts() {
            let missing: Vec<Chunk> = plan
                .fetch
                .iter()
                .filter(|c| {
                    !fsutil::sha256_file(&cache.join(&c.sha256))
                        .is_ok_and(|(n, h)| n == c.size && h == c.sha256)
                })
                .cloned()
                .collect();
            if missing.is_empty() {
                return Ok(());
            }
            match src.get_blobs(host, &missing, &cache, progress).await {
                Ok(()) => {}
                Err(e) => errs.push(format!("host {host}: {e}")),
            }
        }
        let still = plan.fetch.iter().any(|c| {
            !fsutil::sha256_file(&cache.join(&c.sha256)).is_ok_and(|(n, h)| n == c.size && h == c.sha256)
        });
        if still {
            return Err(InstallError::Network(errs.join("; ")));
        }
        Ok(())
    }

    /// Assemble every not-yet-installed package of `plan` from the cache into a fresh staging dir.
    fn stage(&self, plan: &Plan) -> Result<PathBuf, InstallError> {
        let staging =
            self.root
                .join(STAGING)
                .join(format!("{}-{}-{}", plan.module, plan.version, std::process::id()));
        fsutil::remove_tree(&staging)?;
        fsutil::mkdir_private(&staging)?;
        let cache = self.root.join(BLOBS);
        for p in plan.packages.iter().filter(|p| !p.installed) {
            let dir = staging.join(install_dir(&p.id, &p.version, p.entry.kind));
            fsutil::mkdir_private(&dir)?;
            for f in &p.entry.files {
                let dest = dir.join(&f.path);
                if let Some(parent) = dest.parent() {
                    fsutil::mkdir_private(parent)?;
                }
                {
                    let mut out = std::fs::OpenOptions::new();
                    out.create_new(true).write(true);
                    #[cfg(unix)]
                    std::os::unix::fs::OpenOptionsExt::mode(&mut out, 0o600);
                    let mut out = out.open(&dest)?;
                    for c in f.blobs() {
                        std::io::copy(&mut std::fs::File::open(cache.join(&c.sha256))?, &mut out)?;
                    }
                    out.sync_all()?;
                }
                let (n, h) = fsutil::sha256_file(&dest)?;
                if n != f.size || h != f.sha256 {
                    return Err(InstallError::Io(format!(
                        "{}: assembled file doesn't match the index",
                        f.path
                    )));
                }
            }
        }
        Ok(staging)
    }

    /// The staged module.json, checked against its entry; and each dependency's pin checked against the index.
    fn candidate(
        &self,
        plan: &Plan,
        staging: &Path,
        package_sha256: String,
    ) -> Result<Candidate, InstallError> {
        let me = &plan.packages[0];
        let mdir = if me.installed {
            self.root.join(module_dir(&me.id, &me.version))
        } else {
            staging.join(module_dir(&me.id, &me.version))
        };
        let bytes = std::fs::read(mdir.join(MODULE_MANIFEST_FILE))?;
        let sha = crate::sha256_hex(&bytes);
        if Some(&sha) != me.entry.manifest_sha256.as_ref() {
            return Err(InstallError::ManifestMismatch("sha256".into()));
        }
        let m = ModuleManifest::parse(&bytes)?;
        if m.id != plan.module || m.version != plan.version || m.code.team_id != me.entry.team_id {
            return Err(InstallError::ManifestMismatch("id, version or team".into()));
        }
        let mut pins: Vec<PinnedRef> = m
            .dependencies
            .iter()
            .map(|d| PinnedRef {
                id: d.id.clone(),
                version: d.version.clone(),
            })
            .collect();
        let mut req = me.entry.requires.clone();
        pins.sort();
        req.sort();
        if pins != req {
            return Err(InstallError::ManifestMismatch(
                "requires differs from the manifest's dependencies".into(),
            ));
        }
        let mut brings = vec![];
        for d in &m.dependencies {
            let p = plan
                .packages
                .iter()
                .find(|p| p.id == d.id && p.version == d.version)
                .expect("planned from requires");
            let e = &p.entry;
            let field = if dep_kind(e.kind) != Some(d.kind) {
                Some("kind")
            } else if e.sha256 != d.sha256 {
                Some("sha256")
            } else if e.team_id != d.team_id {
                Some("teamId")
            } else if e.bundle_id != d.bundle_id {
                Some("bundleId")
            } else if e.designated_requirement != d.designated_requirement {
                Some("designatedRequirement")
            } else {
                None
            };
            if let Some(field) = field {
                return Err(InstallError::PinMismatch {
                    id: d.id.clone(),
                    version: d.version.clone(),
                    field,
                });
            }
            if !p.installed {
                brings.push((
                    PinnedRef {
                        id: d.id.clone(),
                        version: d.version.clone(),
                    },
                    LockedPackage::for_dependency(d),
                ));
            }
        }
        Ok(Candidate {
            manifest: m,
            manifest_sha256: sha,
            package_sha256,
            packages: brings,
        })
    }

    /// Rename the staged dirs into place, make them read-only, then write the lockfile (the commit point).
    fn commit(&self, plan: &Plan, staging: &Path, next: &Lockfile) -> Result<(), InstallError> {
        for p in plan.packages.iter().filter(|p| !p.installed) {
            let rel = install_dir(&p.id, &p.version, p.entry.kind);
            let from = staging.join(&rel);
            let to = self.root.join(&rel);
            if let Some(parent) = to.parent() {
                fsutil::mkdir_private(parent)?;
            }
            // An orphan from a crash before a lockfile write (nothing names it): replace it. A dir the lockfile
            // names is never replaced (an installed version never changes; ADR 0146 §3).
            if self
                .lock()?
                .modules
                .iter()
                .any(|(id, m)| module_dir(id, &m.version) == rel)
                || self
                    .lock()?
                    .packages
                    .values()
                    .any(|v| v.values().any(|p| p.path == rel))
            {
                return Err(ModuleError::ContentChanged {
                    id: p.id.clone(),
                    version: p.version.clone(),
                }
                .into());
            }
            if to.exists() {
                fsutil::remove_tree(&to)?;
            }
            std::fs::rename(&from, &to)?;
            make_read_only(&to, &p.entry)?;
        }
        next.save(&self.lock_path())?;
        fsutil::remove_tree(staging)?;
        let cache = self.root.join(BLOBS);
        for p in &plan.packages {
            for c in p.entry.blobs() {
                let _ = std::fs::remove_file(cache.join(&c.sha256));
            }
        }
        Ok(())
    }

    /// A network install of a planned module. Refuses unless `consent` is for exactly this plan, and (rule 1) the
    /// stored timestamp is fresh.
    pub async fn install(
        &self,
        src: &dyn Transport,
        consent: &InstallConsent,
        now_ms: u64,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<Installed, InstallError> {
        let (trust, v) = self.cached_index()?;
        let plan = self.plan_from(
            &v.index,
            &v.timestamp.index.sha256,
            &consent.module,
            Some(&consent.version),
        )?;
        if consent.index_sha256 != plan.index_sha256
            || consent.module != plan.module
            || consent.version != plan.version
            || consent.download_bytes < plan.download_bytes
        {
            return Err(InstallError::ConsentMismatch);
        }
        // Rule 1 before any byte moves: an expired list refuses here, not after the download.
        if !trust
            .revocation()
            .is_some_and(|s| s.is_fresh(trust.effective_now(now_ms)))
        {
            return Err(Refusal::RevocationListExpired.into());
        }
        self.fetch(src, &plan, progress).await?;
        let staging = self.stage(&plan)?;
        let r = self.finish(
            &plan,
            &staging,
            &trust,
            Request::Network,
            plan.index_sha256.clone(),
            now_ms,
        );
        if r.is_err() {
            let _ = fsutil::remove_tree(&staging);
        }
        r.map(|(i, _)| i)
    }

    fn finish(
        &self,
        plan: &Plan,
        staging: &Path,
        trust: &Trust,
        req: Request<'_>,
        package_sha256: String,
        now_ms: u64,
    ) -> Result<(Installed, Option<crate::sideload::RevocationSnapshot>), InstallError> {
        let c = self.candidate(plan, staging, package_sha256)?;
        let checks = StagedChecks {
            checks: self.checks.as_ref(),
            plan,
            staging,
            root: &self.root,
        };
        let lock = self.lock()?;
        let d = decide(
            req,
            &c,
            &checks,
            trust.revocation(),
            &lock,
            trust.effective_now(now_ms),
        )?;
        self.commit(plan, staging, &d.next_lock)?;
        Ok((
            Installed {
                module: plan.module.clone(),
                version: plan.version.clone(),
                stale_revocation: d.stale_revocation,
                notices: vec![],
            },
            d.adopted,
        ))
    }

    /// A user-picked `.qrtzpkg` (Bozniak's rules; ADR 0144 §6). Without `confirmation` this returns
    /// `Refused(NeedsConfirmation(prompt))` with the list's age, having changed nothing.
    pub async fn sideload(
        &self,
        file: &Path,
        origin: SideloadOrigin,
        confirmation: Option<&Confirmation>,
        now_ms: u64,
    ) -> Result<Installed, InstallError> {
        if origin != SideloadOrigin::UserPickedFile {
            return Err(Refusal::NotUserInitiated.into());
        }
        let pkg = Qrtzpkg::open(file)?;
        let package_sha256 = pkg.sha256()?;
        let src = PkgTransport(&pkg);
        let mut trust = self.roots(&src, 0, self.trust()?).await?;
        let (carried, ts, ts_sig) = self.pair(&src, 0, &trust, Mode::Sideload, now_ms).await?;
        let snap = VerifiedSnapshot::verify(
            &trust.carried(&carried.timestamp, carried.timestamp_sha256.clone()),
            &carried.index_bytes,
            &carried.index_sig,
        )?;
        // Rule 3: a newer carried list is adopted as soon as it verifies, before the prompt and whatever the
        // outcome of this install: it is signed and only adds knowledge. If it is fresh, every installed module is
        // rechecked against it now (rule 6), so a revoked one is disabled even when this file is refused.
        let mut notices = vec![];
        let (_, newer) = crate::sideload::adopt_snapshot(trust.revocation(), Some(&snap));
        if newer {
            trust.commit(&carried, now_ms)?;
            self.save_index(&carried, &ts, &ts_sig)?;
            trust.save(&self.trust_path())?;
            if snap.get().is_fresh(trust.effective_now(now_ms)) {
                notices = self.recheck(&trust, &carried, now_ms)?;
            }
        }
        let mut plan = self.plan_from(
            &carried.index,
            &carried.timestamp.index.sha256,
            &pkg.toc.module,
            Some(&pkg.toc.version),
        )?;
        // Rule 5: every blob not already installed comes from the file; nothing is fetched.
        let cache = self.root.join(BLOBS);
        fsutil::mkdir_private(&cache)?;
        for c in &plan.fetch {
            let dest = cache.join(&c.sha256);
            if !pkg.copy_to(&layout::blob(&c.sha256), &dest)? {
                return Err(InstallError::MissingFromFile(c.sha256.clone()));
            }
            let (n, h) = fsutil::sha256_file(&dest)?;
            if n != c.size || h != c.sha256 {
                let _ = std::fs::remove_file(&dest);
                return Err(InstallError::Io(format!(
                    "{}: blob doesn't match the index",
                    c.sha256
                )));
            }
        }
        plan.fetch.clear();
        let staging = self.stage(&plan)?;
        let req = Request::Sideload {
            origin,
            confirmation,
            carried: Some(snap.clone()),
        };
        let r = self.finish(&plan, &staging, &trust, req, package_sha256, now_ms);
        let mut installed = match r {
            Ok((i, _)) => i,
            Err(e) => {
                let _ = fsutil::remove_tree(&staging);
                return Err(e);
            }
        };
        // Roots learned from the file are kept (they chained to ours).
        trust.note_clock(now_ms);
        trust.save(&self.trust_path())?;
        installed.notices = notices;
        Ok(installed)
    }

    /// Remove dirs under `modules/` and `packages/` that the lockfile doesn't name (a crash before the lockfile
    /// write), and the staging area.
    pub fn sweep(&self) -> Result<Vec<String>, InstallError> {
        let lock = self.lock()?;
        let mut keep: Vec<String> = lock
            .modules
            .iter()
            .map(|(id, m)| module_dir(id, &m.version))
            .collect();
        keep.extend(
            lock.packages
                .values()
                .flat_map(|v| v.values().map(|p| p.path.clone())),
        );
        let mut gone = vec![];
        for top in ["modules", "packages"] {
            let base = self.root.join(top);
            let Ok(a) = std::fs::read_dir(&base) else { continue };
            for a in a.flatten() {
                let Ok(b) = std::fs::read_dir(a.path()) else {
                    continue;
                };
                for b in b.flatten() {
                    let rels: Vec<PathBuf> = if top == "modules" {
                        vec![b.path()]
                    } else {
                        std::fs::read_dir(b.path())
                            .map(|c| c.flatten().map(|c| c.path()).collect())
                            .unwrap_or_default()
                    };
                    for p in rels {
                        let rel = p
                            .strip_prefix(&self.root)
                            .unwrap_or(&p)
                            .to_string_lossy()
                            .replace('\\', "/");
                        if !keep.contains(&rel) {
                            fsutil::remove_tree(&p)?;
                            gone.push(rel);
                        }
                    }
                }
            }
        }
        fsutil::remove_tree(&self.root.join(STAGING))?;
        Ok(gone)
    }
}

fn make_read_only(dir: &Path, e: &PackageEntry) -> std::io::Result<()> {
    let exec: BTreeMap<&str, bool> = e.files.iter().map(|f| (f.path.as_str(), f.executable)).collect();
    fn walk(base: &Path, d: &Path, exec: &BTreeMap<&str, bool>) -> std::io::Result<()> {
        for x in std::fs::read_dir(d)? {
            let x = x?;
            let p = x.path();
            if x.file_type()?.is_dir() {
                walk(base, &p, exec)?;
            } else {
                let rel = p
                    .strip_prefix(base)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                fsutil::set_mode(
                    &p,
                    if exec.get(rel.as_str()).copied().unwrap_or(false) {
                        0o500
                    } else {
                        0o400
                    },
                )?;
            }
        }
        fsutil::set_mode(d, 0o500)
    }
    walk(dir, dir, &exec)
}

/// Runs [`CodeChecks`] on the staged (or already installed) code of a plan; `decide` calls it first (rule 7).
struct StagedChecks<'a> {
    checks: &'a dyn CodeChecks,
    plan: &'a Plan,
    staging: &'a Path,
    root: &'a Path,
}

impl PackageChecks for StagedChecks<'_> {
    fn check(&self, c: &Candidate) -> Result<(), String> {
        for p in &self.plan.packages {
            let rel = install_dir(&p.id, &p.version, p.entry.kind);
            let dir = if p.installed {
                self.root.join(&rel)
            } else {
                self.staging.join(&rel)
            };
            match p.entry.kind {
                PackageKind::Module => self.checks.check_module(&dir, &c.manifest)?,
                PackageKind::XpcService => self.checks.check_service(
                    &dir,
                    p.entry.bundle_id.as_deref().unwrap_or_default(),
                    &p.entry.team_id,
                    p.entry.designated_requirement.as_deref().unwrap_or_default(),
                )?,
                _ => {}
            }
        }
        Ok(())
    }
}

/// A `.qrtzpkg` as a single host.
pub struct PkgTransport<'a>(pub &'a Qrtzpkg);

#[async_trait::async_trait]
impl Transport for PkgTransport<'_> {
    fn hosts(&self) -> usize {
        1
    }
    async fn get(&self, _host: usize, path: &str, max: usize) -> Result<Option<Vec<u8>>, String> {
        match self.0.get(path).map_err(|e| e.to_string())? {
            Some(b) if b.len() > max => Err(format!("{path}: too large")),
            x => Ok(x),
        }
    }
    async fn get_blobs(
        &self,
        _host: usize,
        blobs: &[Chunk],
        dir: &Path,
        _progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), String> {
        for c in blobs {
            if !self
                .0
                .copy_to(&layout::blob(&c.sha256), &dir.join(&c.sha256))
                .map_err(|e| e.to_string())?
            {
                return Err(format!("{}: not in the file", c.sha256));
            }
        }
        Ok(())
    }
}
