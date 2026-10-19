//! qrtz's package manager (ADR 0157, on ADR 0153's signed index): one installer for extensions, services and models,
//! used the same way by the app (IPC, the extensions screen) and the `qrtz` CLI.
//!
//! Every change is a [`Plan`] computed first and applied second:
//!
//! 1. **Plan** ([`Installer::plan`]). The next `qrtz.lock` for an [`Op`] (`install`, `remove`, `update`, an engine
//!    swap, a frozen reinstall, or app-pinned models), what changes, and which blobs are missing from the store.
//!    Ranges are resolved only here and only online: against the cached signed index from the last consented
//!    refresh. Without one, only lock-only ops plan (`remove`, frozen, pinned).
//! 2. **Consent.** Nothing is fetched without an [`InstallConsent`] for exactly that plan (its hash and size).
//! 3. **Rules** (Bozniak's, ADR 0144 §6 / 0146 §5). A plan that resolved online, or fetches code, needs a fresh
//!    revocation list (rule 1); nothing revoked is installed (rule 2); `.qrtzpkg` sideloads follow rules 3-6.
//! 4. **Fetch** ([`crate::fetch`]): missing blobs only, parallel ranges, resumable, from the hosts then each blob's
//!    signed mirror URLs, each verified into the store.
//! 5. **Stage**: each new package is linked from the store (re-verified, cloned on APFS) into a staging dir, its
//!    `qrtz.json` checked against the signed facts.
//! 6. **Code checks** ([`CodeChecks`], rule 7): every staged extension and service, both paths.
//! 7. **Swap**: staged dirs are renamed into `packages/<name>/<version>`, made read-only, and `qrtz.lock` is written
//!    last (temp + rename): the whole resolved set changes at once. Old dirs stay (a running helper keeps its files)
//!    until `prune`, which removes dirs and store blobs the lock no longer references.
//!
//! Nothing a package contains is run during any of this: there are no install scripts (ADR 0157 §3).
use crate::fetch::{fetch_blobs, Source, Transport};
use crate::fsutil;
use crate::index::{
    layout, BlobRef, Index, IndexError, PackageEntry, PackageKind, Tree, MANIFEST_FILE, MAX_META_BYTES,
};
use crate::pkg::Qrtzpkg;
use crate::qrtz_json::PackageManifest;
use crate::qrtz_lock::{key, package_dir, split_key, Locked, QrtzLock, Source as LockSource, LOCK_FILE};
use crate::resolve::{resolve, ResolveError, Update};
use crate::sideload::{
    adopt_snapshot, Confirmation, Notice, Prompt, Refusal, RevocationSnapshot, SideloadOrigin,
    VerifiedSnapshot,
};
use crate::store::Store;
use crate::trust::{Mode, Trust, Verified, TRUST_FILE};
use crate::ModuleError;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Where everything lives: the app's data dir (`~/Library/Application Support/ai.qrtz.desktop` on macOS), or
/// `QRTZ_USER_DATA` (tests, side-by-side runs). The app and the CLI resolve it the same way.
pub fn default_root() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("QRTZ_USER_DATA") {
        return Some(PathBuf::from(d));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/ai.qrtz.desktop"))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("the consent doesn't match this plan (asked again)")]
    ConsentMismatch,
    #[error("the lock or the index changed since this plan; plan again")]
    StalePlan,
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error("every host failed: {0}")]
    Network(String),
    #[error("{0}")]
    Refused(Refusal),
    #[error("no signed index yet: this needs an online refresh first (offline installs use qrtz.lock only)")]
    NoIndex,
    #[error("{0}: not in qrtz.lock (offline, only what the lock names can be installed)")]
    NotLocked(String),
    #[error("{0}: not installed")]
    NotInstalled(String),
    #[error("{key}: the package's qrtz.json doesn't match its signed entry ({why})")]
    ManifestMismatch { key: String, why: String },
    #[error("{0}: tree not available (offline, and not in the store)")]
    NoTree(String),
    #[error("{0}: missing from the file (a sideload brings everything it needs)")]
    MissingFromFile(String),
    #[error("{0}: only models can be pinned by the app")]
    PinnedCode(String),
    #[error("another install is running")]
    Busy,
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

/// Rule 7's Apple half, on staged bytes: an extension's helper or a service's `.xpc` against its exact designated
/// requirement and team, plus notarization. qrtz-app implements it with `codesign` and `spctl` on macOS.
pub trait CodeChecks: Send + Sync {
    /// `code` is the helper executable (extension) or the `.xpc` bundle (service), inside `dir`.
    fn check(&self, kind: PackageKind, code: &Path, team_id: &str, requirement: &str) -> Result<(), String>;
    /// ADR 0170, offline: `file` (the notarized executable, already matched to the record's sha256 by the
    /// installer) is the code Apple notarized: its code directory hash is `n.cdhash`. macOS reads it with
    /// `codesign`; a test double may accept.
    fn check_notarized(&self, _file: &Path, _n: &crate::index::Notarization) -> Result<(), String> {
        Ok(())
    }
}

/// The user said yes to checking for packages (onboarding, settings, or running `qrtz install|update`).
#[derive(Clone, Copy, Debug)]
pub struct RefreshConsent(());
impl RefreshConsent {
    /// Only a user action (an IPC handler, a CLI command the user typed) calls this.
    pub fn user_asked() -> Self {
        Self(())
    }
}

/// The user said yes to this plan, after seeing what changes and its download size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallConsent {
    pub plan_sha256: String,
    pub download_bytes: u64,
}

/// A model pinned in the qrtz binary (ADR 0157 §6): its tree (files, sizes, hashes, mirror URLs) and its
/// `qrtz.json` bytes. Installed through the same fetcher, store and swap as everything else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedPackage {
    pub name: String,
    pub version: String,
    pub team_id: String,
    pub tree: Tree,
    pub manifest: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// `qrtz install <name>[@range] …`: add requests (default range `^<newest>`), resolve online.
    Install(Vec<(String, Option<String>)>),
    /// `qrtz remove <name> …`: drop requests; whatever nothing else needs leaves the lock. No network.
    Remove(Vec<String>),
    /// `qrtz update [name …] [--latest]`: re-resolve, preferring the newest; `latest` also moves the ranges.
    Update { names: Vec<String>, latest: bool },
    /// Switch an extension's engine: replace the model dependency of the same task with `model` (`^<newest>`).
    Engine { extension: String, model: String },
    /// `qrtz install --frozen`: exactly `qrtz.lock`, nothing resolved; missing dirs relinked, missing blobs fetched.
    Frozen,
    /// App-pinned models, requested exactly (data only).
    Pinned(Vec<PinnedPackage>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Add { name: String, version: String },
    Remove { name: String, version: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub op: Op,
    /// The lock this plan writes.
    pub next: QrtzLock,
    pub changes: Vec<Change>,
    /// Blobs missing from the store.
    pub fetch: Vec<BlobRef>,
    pub download_bytes: u64,
    /// Size of everything new to link.
    pub link_bytes: u64,
    /// Resolved against the signed index (rule 1 applies).
    pub online: bool,
    pub index_sha256: Option<String>,
    base_lock_sha256: String,
    pub plan_sha256: String,
    wants: BTreeMap<String, Want>,
}

impl Plan {
    pub fn consent(&self) -> InstallConsent {
        InstallConsent {
            plan_sha256: self.plan_sha256.clone(),
            download_bytes: self.download_bytes,
        }
    }
    pub fn is_noop(&self) -> bool {
        self.changes.is_empty() && self.wants.is_empty()
    }
}

/// One package to stage: the signed facts and its tree.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Want {
    name: String,
    version: String,
    locked: Locked,
    /// From the index (online / sideload); `None` when only the lock's exact versions are known.
    ranges: Option<BTreeMap<String, String>>,
    manifest_sha256: Option<String>,
    tree: Tree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub changes: Vec<Change>,
    pub stale_revocation: bool,
    pub notices: Vec<Notice>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refreshed {
    pub timestamp_version: u64,
    pub index_version: u64,
    pub notices: Vec<Notice>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub dirs: Vec<String>,
    pub blobs: usize,
    pub bytes: u64,
}

/// One row of `qrtz ls` / the extensions screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub version: String,
    pub kind: PackageKind,
    pub requested: Option<String>,
    pub required_by: Vec<String>,
    /// Who uses it, by name ([`QrtzLock::users`]): what the module manager's "used by" label shows.
    pub used_by: Vec<String>,
    pub disabled: Option<String>,
    pub size: u64,
}

pub struct Installer {
    root: PathBuf,
    pinned_root_sha256: Option<String>,
    checks: Box<dyn CodeChecks>,
    store: Store,
}

const CACHE: &str = "index-cache";
const PACKAGES: &str = "packages";
const LEGACY_LOCK: &str = "modules/modules.lock.json";

/// Holds `.qrtz-install.lock` for one change: the app and the CLI never write at once.
struct Guard(#[allow(dead_code)] std::fs::File);

impl Installer {
    pub fn new(
        root: impl Into<PathBuf>,
        pinned_root_sha256: Option<String>,
        checks: Box<dyn CodeChecks>,
    ) -> Self {
        let root = root.into();
        Self {
            store: Store::new(root.join("store")),
            root,
            pinned_root_sha256,
            checks,
        }
    }

    /// The pinned `1.root.json` sha256 (none until the offline root exists: no index can be refreshed).
    pub fn pinned_root(&self) -> Option<&str> {
        self.pinned_root_sha256.as_deref()
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }
    pub fn trust_path(&self) -> PathBuf {
        self.root.join(TRUST_FILE)
    }
    pub fn lock(&self) -> Result<QrtzLock, InstallError> {
        Ok(QrtzLock::load(&self.lock_path())?)
    }
    pub fn trust(&self) -> Result<Option<Trust>, InstallError> {
        Ok(Trust::load(&self.trust_path())?)
    }
    /// Where `name@version`'s files are, if installed.
    pub fn package_path(&self, name: &str, version: &str) -> PathBuf {
        self.root.join(package_dir(name, version))
    }

    fn guard(&self) -> Result<Guard, InstallError> {
        fsutil::mkdir_private(&self.root)?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join(".qrtz-install.lock"))?;
        match f.try_lock() {
            Ok(()) => Ok(Guard(f)),
            Err(std::fs::TryLockError::WouldBlock) => Err(InstallError::Busy),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }

    // ---- trust and the index (ADR 0153) ----

    async fn roots(
        &self,
        src: &dyn Transport,
        host: usize,
        trust: Option<Trust>,
    ) -> Result<Trust, InstallError> {
        let net = InstallError::Network;
        let mut t = match trust {
            Some(t) => t,
            None => {
                self.pinned_root_sha256.as_ref().ok_or(IndexError::NoPinnedRoot)?;
                let p = layout::root(1);
                let (Some(b), Some(s)) = (
                    src.get(host, &p, MAX_META_BYTES).await.map_err(net)?,
                    src.get(host, &layout::sig(&p), MAX_META_BYTES)
                        .await
                        .map_err(net)?,
                ) else {
                    return Err(InstallError::Network(format!("{p}: not found")));
                };
                Trust::bootstrap(self.pinned_root_sha256.as_deref(), &b, &s)?
            }
        };
        for _ in 0..1000 {
            let p = t.next_root_path();
            let Some(b) = src.get(host, &p, MAX_META_BYTES).await.map_err(net)? else {
                break;
            };
            let s = src
                .get(host, &layout::sig(&p), MAX_META_BYTES)
                .await
                .map_err(net)?
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
        let get = |p: String, max: usize| async move {
            src.get(host, &p, max)
                .await
                .map_err(InstallError::Network)?
                .ok_or(InstallError::Network(format!("{p}: not found")))
        };
        let tp = layout::timestamp();
        let ts = get(tp.clone(), MAX_META_BYTES).await?;
        let ts_sig = get(layout::sig(&tp), MAX_META_BYTES).await?;
        let stamp = t.verify_timestamp(mode, &ts, &ts_sig, now_ms)?;
        let ip = layout::index(&stamp.index.sha256);
        let ix = get(ip.clone(), stamp.index.size as usize).await?;
        let ix_sig = get(layout::sig(&ip), MAX_META_BYTES).await?;
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

    /// Network refresh: roots, then the newest fresh timestamp and its index from the first host that serves a good
    /// one (a stale, expired or badly signed host is skipped: the freeze attack has to win every host). Then every
    /// installed package is rechecked against the fresh list (rule 6).
    pub async fn refresh(
        &self,
        src: &dyn Transport,
        _consent: &RefreshConsent,
        now_ms: u64,
    ) -> Result<Refreshed, InstallError> {
        let _g = self.guard()?;
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
                    let notices = self.recheck(t.revocation(), t.effective_now(now_ms))?;
                    return Ok(Refreshed {
                        timestamp_version: v.timestamp.version,
                        index_version: v.index.version,
                        notices,
                    });
                }
                Err(e) => errs.push(e),
            }
        }
        let pick = errs
            .iter()
            .find(|e| matches!(e, InstallError::Index(_)))
            .or(errs.first())
            .cloned();
        Err(pick.unwrap_or(InstallError::Network("no hosts".into())))
    }

    /// Rule 6: a fresh list disables what it revokes (and every extension that depends on it), and clears stale marks.
    fn recheck(&self, list: Option<&RevocationSnapshot>, now_ms: u64) -> Result<Vec<Notice>, InstallError> {
        let Some(s) = list.filter(|s| s.is_fresh(now_ms)) else {
            return Ok(vec![]);
        };
        let lock = self.lock()?;
        let (next, notices) = recheck_lock(&lock, s);
        if next != lock {
            next.save(&self.lock_path())?;
        }
        Ok(notices)
    }

    // ---- planning ----

    /// Plan `op`. Pure apart from reading the lock, the cached index and the store (pinned trees are added to the
    /// store so they can be planned like any other).
    pub fn plan(&self, op: Op) -> Result<Plan, InstallError> {
        let lock = self.lock()?;
        let base_lock_sha256 = crate::sha256_hex(&lock.to_bytes());
        let mut ws = lock.workspace.clone();
        let index = || self.cached_index().map(|(_, v)| v);
        let (next, online, index_sha, index_v): (QrtzLock, bool, Option<String>, Option<Verified>) = match &op
        {
            Op::Remove(names) => {
                for n in names {
                    if ws.dependencies.remove(n).is_none() {
                        return Err(InstallError::NotInstalled(n.clone()));
                    }
                    ws.resolved.remove(n);
                    ws.overrides.remove(n);
                }
                let next = QrtzLock {
                    workspace: ws,
                    ..lock.clone()
                }
                .without_unreachable();
                (next, false, None, None)
            }
            Op::Frozen => (lock.clone(), false, None, None),
            Op::Pinned(pkgs) => {
                let mut next = lock.clone();
                for p in pkgs {
                    let locked = self.pinned_locked(p)?;
                    if let Some(have) = next.get(&p.name, &p.version) {
                        if have.sha256 != locked.sha256 {
                            return Err(ModuleError::ContentChanged {
                                id: p.name.clone(),
                                version: p.version.clone(),
                            }
                            .into());
                        }
                    } else {
                        next.packages.insert(key(&p.name, &p.version), locked);
                    }
                    if !next.workspace.dependencies.contains_key(&p.name) {
                        next.workspace
                            .dependencies
                            .insert(p.name.clone(), format!("={}", p.version));
                        next.workspace.resolved.insert(p.name.clone(), p.version.clone());
                    }
                }
                (next, false, None, None)
            }
            Op::Install(_) | Op::Update { .. } | Op::Engine { .. } => {
                let v = index()?;
                let ix = &v.index;
                let mut update = Update::None;
                match &op {
                    Op::Install(reqs) => {
                        for (n, r) in reqs {
                            crate::qrtz_json::kind_of(n)?;
                            let range = match r {
                                Some(r) => {
                                    crate::qrtz_json::parse_range(n, r)?;
                                    r.clone()
                                }
                                None => match ws.dependencies.get(n) {
                                    Some(have) => have.clone(),
                                    None => format!(
                                        "^{}",
                                        ix.latest(n)
                                            .ok_or_else(|| ResolveError::NotFound {
                                                name: n.clone(),
                                                range: "latest".into(),
                                                by: "the workspace".into(),
                                            })?
                                            .0
                                    ),
                                },
                            };
                            ws.dependencies.insert(n.clone(), range);
                        }
                    }
                    Op::Update { names, latest } => {
                        update = if names.is_empty() {
                            Update::All
                        } else {
                            Update::Names(names.iter().cloned().collect())
                        };
                        if *latest {
                            for (n, r) in ws.dependencies.iter_mut() {
                                if (names.is_empty() || names.contains(n)) && !r.starts_with('=') {
                                    if let Some((v, _)) = ix.latest(n) {
                                        *r = format!("^{v}");
                                    }
                                }
                            }
                        }
                    }
                    Op::Engine { extension, model } => {
                        let ver = ws
                            .resolved
                            .get(extension)
                            .ok_or_else(|| InstallError::NotInstalled(extension.clone()))?;
                        let e = ix
                            .entry(extension, ver)
                            .ok_or_else(|| InstallError::NotLocked(key(extension, ver)))?;
                        let task_of = |n: &str| {
                            ix.packages
                                .get(n)
                                .and_then(|vs| vs.values().find_map(|e| e.task.clone()))
                        };
                        let want = task_of(model);
                        let from = e
                            .dependencies
                            .keys()
                            .find(|d| want.is_some() && task_of(d) == want)
                            .cloned()
                            .ok_or_else(|| ResolveError::BadOverride {
                                extension: extension.clone(),
                                from: "?".into(),
                                to: model.clone(),
                            })?;
                        let o = ws.overrides.entry(extension.clone()).or_default();
                        if from == *model {
                            o.remove(&from);
                        } else {
                            let newest = ix.latest(model).map(|(v, _)| v.to_owned()).ok_or_else(|| {
                                ResolveError::NotFound {
                                    name: model.clone(),
                                    range: "latest".into(),
                                    by: extension.clone(),
                                }
                            })?;
                            o.insert(from, format!("{model}@^{newest}"));
                        }
                        if o.is_empty() {
                            ws.overrides.remove(extension);
                        }
                    }
                    _ => unreachable!(),
                }
                // App-pinned requests the index doesn't carry (yet) stay exactly as they are; the rest resolve.
                let carried: Vec<(String, String, String)> = ws
                    .dependencies
                    .iter()
                    .filter_map(|(n, r)| {
                        let v = ws.resolved.get(n)?;
                        let l = lock.get(n, v)?;
                        (l.source == LockSource::Pinned && ix.entry(n, v).is_none())
                            .then(|| (n.clone(), r.clone(), v.clone()))
                    })
                    .collect();
                let mut rws = ws.clone();
                for (n, _, _) in &carried {
                    rws.dependencies.remove(n);
                    rws.resolved.remove(n);
                }
                let r = resolve(ix, &rws, &lock, &update)?;
                let mut next = QrtzLock {
                    workspace: crate::qrtz_lock::Workspace {
                        resolved: r.resolved.clone(),
                        ..rws.clone()
                    },
                    ..QrtzLock::default()
                };
                for (n, range, v) in &carried {
                    next.workspace.dependencies.insert(n.clone(), range.clone());
                    next.workspace.resolved.insert(n.clone(), v.clone());
                    let k = key(n, v);
                    if !r.packages.contains_key(&k) {
                        next.packages.insert(k.clone(), lock.packages[&k].clone());
                    }
                }
                for (k, p) in &r.packages {
                    let mut l = locked_from_entry(&p.entry, &p.dependencies, LockSource::Network);
                    if let Some(have) = lock.packages.get(k) {
                        if have.sha256 != p.entry.sha256 {
                            return Err(ModuleError::ContentChanged {
                                id: p.name.clone(),
                                version: p.version.clone(),
                            }
                            .into());
                        }
                        // An installed package keeps its source and flags; only its resolution can move.
                        l.source = have.source;
                        l.stale_revocation = have.stale_revocation;
                        l.disabled = have.disabled.clone();
                    }
                    next.packages.insert(k.clone(), l);
                }
                (next, true, Some(v.timestamp.index.sha256.clone()), Some(v))
            }
        };
        next.validate()?;
        self.finish_plan(
            op,
            lock,
            next,
            online,
            index_sha,
            index_v.as_ref(),
            base_lock_sha256,
        )
    }

    fn pinned_locked(&self, p: &PinnedPackage) -> Result<Locked, InstallError> {
        let kind = crate::qrtz_json::kind_of(&p.name)?;
        if kind != PackageKind::Model {
            return Err(InstallError::PinnedCode(p.name.clone()));
        }
        p.tree
            .validate()
            .map_err(|why| IndexError::Invalid("tree", why))?;
        let m = PackageManifest::parse(&p.manifest)?;
        let msha = crate::sha256_hex(&p.manifest);
        if m.name != p.name
            || m.version != p.version
            || p.tree.file(MANIFEST_FILE).map(|f| &f.sha256) != Some(&msha)
        {
            return Err(InstallError::ManifestMismatch {
                key: key(&p.name, &p.version),
                why: "the pinned qrtz.json isn't the tree's".into(),
            });
        }
        // The tree and the manifest go into the store now, so the plan can treat them like downloaded ones.
        self.store.insert_bytes(&p.tree.bytes(), &p.tree.sha256())?;
        self.store.insert_bytes(&p.manifest, &msha)?;
        Ok(Locked {
            kind,
            sha256: p.tree.sha256(),
            size: p.tree.size(),
            team_id: p.team_id.clone(),
            identifier: None,
            designated_requirement: None,
            dependencies: BTreeMap::new(),
            task: m.task().map(str::to_owned),
            models: vec![],
            source: LockSource::Pinned,
            stale_revocation: false,
            disabled: None,
            notarization: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_plan(
        &self,
        op: Op,
        lock: QrtzLock,
        next: QrtzLock,
        online: bool,
        index_sha256: Option<String>,
        index: Option<&Verified>,
        base_lock_sha256: String,
    ) -> Result<Plan, InstallError> {
        let mut changes = vec![];
        for k in lock.packages.keys().filter(|k| !next.packages.contains_key(*k)) {
            let (n, v) = split_key(k).expect("valid");
            changes.push(Change::Remove {
                name: n.into(),
                version: v.into(),
            });
        }
        let mut wants = BTreeMap::new();
        for (k, l) in &next.packages {
            let (n, v) = split_key(k).expect("valid");
            let installed = lock.packages.get(k).is_some_and(|h| h.sha256 == l.sha256)
                && self.root.join(package_dir(n, v)).is_dir();
            if !lock.packages.contains_key(k) {
                changes.push(Change::Add {
                    name: n.into(),
                    version: v.into(),
                });
            }
            if installed {
                continue;
            }
            let entry = index
                .and_then(|ix| ix.index.entry(n, v))
                .filter(|e| e.sha256 == l.sha256);
            let tree = match entry {
                Some(e) => e.tree(),
                None => match self
                    .store
                    .read_verified(&l.sha256, crate::index::MAX_INDEX_BYTES)?
                {
                    Some(b) => Tree::parse(&b)?,
                    None => return Err(InstallError::NoTree(k.clone())),
                },
            };
            wants.insert(
                k.clone(),
                Want {
                    name: n.into(),
                    version: v.into(),
                    locked: l.clone(),
                    ranges: entry.map(|e| e.dependencies.clone()),
                    manifest_sha256: entry.map(|e| e.manifest_sha256.clone()),
                    tree,
                },
            );
        }
        let mut seen = BTreeSet::new();
        let mut fetch = vec![];
        let mut link_bytes = 0;
        for w in wants.values() {
            link_bytes += w.tree.size();
            let tree_bytes = w.tree.bytes();
            let tree_blob = BlobRef {
                sha256: w.locked.sha256.clone(),
                size: tree_bytes.len() as u64,
                mirrors: vec![],
            };
            for b in std::iter::once(tree_blob).chain(w.tree.blobs()) {
                if seen.insert(b.sha256.clone()) && !self.store.has(&b.sha256, b.size) {
                    fetch.push(b);
                }
            }
        }
        let download_bytes = fetch.iter().map(|b| b.size).sum();
        let mut h = next.to_bytes();
        h.extend_from_slice(base_lock_sha256.as_bytes());
        h.extend_from_slice(index_sha256.as_deref().unwrap_or("").as_bytes());
        Ok(Plan {
            op,
            next,
            changes,
            fetch,
            download_bytes,
            link_bytes,
            online,
            index_sha256,
            base_lock_sha256,
            plan_sha256: crate::sha256_hex(&h),
            wants,
        })
    }

    // ---- applying ----

    /// Apply a plan the user consented to. Refuses a consent for another plan, a plan made on another lock or index,
    /// and (rule 1) a plan that resolved online or fetches code while the stored revocation list isn't fresh.
    pub async fn apply(
        &self,
        src: &dyn Transport,
        plan: &Plan,
        consent: &InstallConsent,
        now_ms: u64,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<Applied, InstallError> {
        let _g = self.guard()?;
        if consent.plan_sha256 != plan.plan_sha256 || consent.download_bytes < plan.download_bytes {
            return Err(InstallError::ConsentMismatch);
        }
        let lock = self.lock()?;
        if crate::sha256_hex(&lock.to_bytes()) != plan.base_lock_sha256 {
            return Err(InstallError::StalePlan);
        }
        let trust = self.trust()?;
        if plan.online {
            let current = self.cached_index().ok().map(|(_, v)| v.timestamp.index.sha256);
            if current != plan.index_sha256 {
                return Err(InstallError::StalePlan);
            }
        }
        let now = trust.as_ref().map_or(now_ms, |t| t.effective_now(now_ms));
        let list = trust.as_ref().and_then(|t| t.revocation());
        let fresh = list.is_some_and(|s| s.is_fresh(now));
        let fetches_code = plan.wants.values().any(|w| {
            w.locked.kind.has_code() && !self.store.has(&w.locked.sha256, w.tree.bytes().len() as u64)
        }) || plan.wants.values().any(|w| {
            w.locked.kind.has_code() && w.tree.blobs().iter().any(|b| !self.store.has(&b.sha256, b.size))
        });
        // ADR 0157 §6: the data exemption is data-only AND no extension.reads consumer. A model an extension's own
        // process reads (model.laya) follows code's rule, whether or not its bytes are already in the store.
        let read_model = plan.wants.values().any(|w| {
            w.locked.kind == PackageKind::Model
                && (crate::qrtz_json::read_by_extension(&w.name)
                    || plan
                        .next
                        .packages
                        .values()
                        .any(|l| l.kind == PackageKind::Extension && l.models.contains(&w.name)))
        });
        // Rule 1, no override.
        if (plan.online || fetches_code || read_model) && !fresh {
            return Err(Refusal::RevocationListExpired.into());
        }
        // Rule 2 on every package the plan installs, both paths.
        revoked_in(
            list,
            plan.wants.values().map(|w| (w.name.as_str(), w.version.as_str())),
        )?;
        fetch_blobs(src, &self.store, &plan.fetch, progress)
            .await
            .map_err(InstallError::Network)?;
        let mut next = plan.next.clone();
        if !fresh {
            for k in plan.wants.keys() {
                if let Some(l) = next.packages.get_mut(k) {
                    l.stale_revocation = true;
                }
            }
        }
        self.swap(plan.wants.values().collect(), &next)?;
        Ok(Applied {
            changes: plan.changes.clone(),
            stale_revocation: !fresh,
            notices: vec![],
        })
    }

    /// Stage every wanted package from the store, check it, rename it into place, write the lock last.
    fn swap(&self, wants: Vec<&Want>, next: &QrtzLock) -> Result<(), InstallError> {
        let staging = self
            .root
            .join(PACKAGES)
            .join(format!(".staging-{}", std::process::id()));
        fsutil::remove_tree(&staging)?;
        fsutil::mkdir_private(&staging)?;
        let r = (|| {
            for w in &wants {
                let dir = staging.join(&w.name).join(&w.version);
                self.stage(w, &dir)?;
            }
            let current = self.lock()?;
            for w in &wants {
                let rel = package_dir(&w.name, &w.version);
                let to = self.root.join(&rel);
                if current
                    .get(&w.name, &w.version)
                    .is_some_and(|h| h.sha256 != w.locked.sha256)
                {
                    return Err(ModuleError::ContentChanged {
                        id: w.name.clone(),
                        version: w.version.clone(),
                    }
                    .into());
                }
                // A dir the current lock doesn't name (a crash before a lock write, or a damaged install) is replaced.
                if to.exists() {
                    fsutil::remove_tree(&to)?;
                }
                fsutil::mkdir_private(to.parent().expect("parent"))?;
                std::fs::rename(staging.join(&w.name).join(&w.version), &to)?;
                make_read_only(&to)?;
            }
            next.save(&self.lock_path())?;
            Ok(())
        })();
        let _ = fsutil::remove_tree(&staging);
        r
    }

    fn stage(&self, w: &Want, dir: &Path) -> Result<(), InstallError> {
        let k = key(&w.name, &w.version);
        let mismatch = |why: &str| InstallError::ManifestMismatch {
            key: k.clone(),
            why: why.into(),
        };
        if w.tree.sha256() != w.locked.sha256 || w.tree.size() != w.locked.size {
            return Err(mismatch("tree hash or size"));
        }
        fsutil::mkdir_private(dir)?;
        for f in &w.tree.files {
            self.store
                .link(&f.sha256, f.size, &dir.join(&f.path), f.executable)?;
        }
        let mbytes = std::fs::read(dir.join(MANIFEST_FILE)).map_err(|_| mismatch("no qrtz.json"))?;
        if w.manifest_sha256
            .as_ref()
            .is_some_and(|h| *h != crate::sha256_hex(&mbytes))
        {
            return Err(mismatch("qrtz.json sha256"));
        }
        // ADR 0157 §9: a model an extension reads in its own process is loaded straight from this dir, so every file
        // is re-hashed after the clone (or link) and must be read-only.
        if w.locked.kind == PackageKind::Model && crate::qrtz_json::read_by_extension(&w.name) {
            for f in &w.tree.files {
                let p = dir.join(&f.path);
                if !file_is_immutable_copy(&p, &f.sha256, f.size)? {
                    return Err(mismatch(&format!(
                        "{}: changed after linking, or writable",
                        f.path
                    )));
                }
            }
        }
        let m = PackageManifest::parse(&mbytes)?;
        if m.name != w.name || m.version != w.version || m.kind != w.locked.kind {
            return Err(mismatch("name, version or kind"));
        }
        if m.task().map(str::to_owned) != w.locked.task {
            return Err(mismatch("task"));
        }
        let models: Vec<String> = match m.kind {
            PackageKind::Service => m.service_models().to_vec(),
            PackageKind::Extension => m.extension.as_ref().map(|e| e.reads.clone()).unwrap_or_default(),
            _ => vec![],
        };
        if models != w.locked.models {
            return Err(mismatch("models"));
        }
        match &w.ranges {
            Some(r) if *r != m.dependencies => return Err(mismatch("dependency ranges")),
            _ => {}
        }
        // The resolution must satisfy the package's own ranges, overrides aside (they name a different package).
        for (d, v) in &w.locked.dependencies {
            if let Some(r) = m.dependencies.get(d) {
                let req = crate::qrtz_json::parse_range(d, r)?;
                if !semver::Version::parse(v).is_ok_and(|s| req.matches(&s)) {
                    return Err(mismatch("a locked dependency is outside its range"));
                }
            }
        }
        // Rule 7: the Apple checks on staged code.
        if w.locked.kind.has_code() {
            let rel = match (&m.extension, &m.service) {
                (Some(e), _) => e.entry.clone(),
                (_, Some(s)) => s.bundle.clone(),
                _ => return Err(mismatch("no code entry")),
            };
            // ADR 0170: the signed record of Apple's notarization, for exactly a file of this tree: the extension's
            // entry, or an executable inside the service's bundle. All offline.
            let n =
                w.locked.notarization.as_ref().ok_or_else(|| {
                    Refusal::ChecksFailed("no notarization record in the signed index".into())
                })?;
            n.check().map_err(Refusal::ChecksFailed)?;
            let notarized = w
                .tree
                .files
                .iter()
                .find(|f| {
                    f.sha256 == n.code_sha256
                        && f.executable
                        && (f.path == rel || f.path.starts_with(&format!("{rel}/Contents/MacOS/")))
                })
                .ok_or_else(|| {
                    Refusal::ChecksFailed("the notarized file isn't this package's code".into())
                })?;
            self.checks
                .check(
                    w.locked.kind,
                    &dir.join(&rel),
                    &w.locked.team_id,
                    w.locked.designated_requirement.as_deref().unwrap_or_default(),
                )
                .map_err(Refusal::ChecksFailed)?;
            self.checks
                .check_notarized(&dir.join(&notarized.path), n)
                .map_err(Refusal::ChecksFailed)?;
        }
        Ok(())
    }

    // ---- sideload (Bozniak's rules 3-6) ----

    /// A user-picked `.qrtzpkg`. Installs exactly the resolved set the file carries (rule 5: no resolution, no fetch),
    /// after the carried index verifies against our trust and the user confirmed this file (rule 4). Without
    /// `confirmation` this returns `Refused(NeedsConfirmation(prompt))`, having changed nothing but adopting a newer
    /// carried revocation list (rule 3), which also rechecks what is installed (rule 6).
    pub async fn sideload(
        &self,
        file: &Path,
        origin: SideloadOrigin,
        confirmation: Option<&Confirmation>,
        now_ms: u64,
    ) -> Result<Applied, InstallError> {
        if origin != SideloadOrigin::UserPickedFile {
            return Err(Refusal::NotUserInitiated.into());
        }
        let _g = self.guard()?;
        let pkg = Qrtzpkg::open(file)?;
        let file_sha = pkg.sha256()?;
        let src = PkgTransport(&pkg);
        let mut trust = self.roots(&src, 0, self.trust()?).await?;
        let (carried, ts, ts_sig) = self.pair(&src, 0, &trust, Mode::Sideload, now_ms).await?;
        let snap = VerifiedSnapshot::verify(
            &trust.carried(&carried.timestamp, carried.timestamp_sha256.clone()),
            &carried.index_bytes,
            &carried.index_sig,
        )?;
        let now = trust.effective_now(now_ms);
        if let Some(s) = trust.revocation() {
            let c = snap.get();
            if c.version < s.version || (c.version > s.version && c.issued_ms < s.issued_ms) {
                return Err(Refusal::RevocationRollback {
                    carried: c.version,
                    stored: s.version,
                }
                .into());
            }
        }
        let mut notices = vec![];
        let (_, newer) = adopt_snapshot(trust.revocation(), Some(&snap));
        if newer {
            trust.commit(&carried, now_ms)?;
            self.save_index(&carried, &ts, &ts_sig)?;
            trust.save(&self.trust_path())?;
            notices = self.recheck(trust.revocation(), trust.effective_now(now_ms))?;
        }
        let list = trust.revocation().cloned();
        let issued = list.as_ref().map(|s| s.issued_ms);
        let fresh = list.as_ref().is_some_and(|s| s.is_fresh(now));
        // Rule 2 before the prompt: a revoked file is refused outright.
        revoked_in(
            list.as_ref(),
            pkg.toc
                .packages
                .iter()
                .map(|p| (p.name.as_str(), p.version.as_str())),
        )?;
        let lock = self.lock()?;
        let target = &pkg.toc.target;
        if let Some(have) = lock.workspace.resolved.get(&target.name) {
            if semver::Version::parse(have).ok() > semver::Version::parse(&target.version).ok() {
                return Err(Refusal::Downgrade {
                    installed: have.clone(),
                }
                .into());
            }
        }
        let Some(yes) = confirmation else {
            return Err(Refusal::NeedsConfirmation(Prompt {
                package_sha256: file_sha,
                module: target.name.clone(),
                version: target.version.clone(),
                revocation_issued_ms: issued,
                revocation_age_ms: issued.map(|i| now.saturating_sub(i)),
                revocation_expired: !fresh,
            })
            .into());
        };
        if yes.package_sha256 != file_sha || yes.revocation_issued_ms != issued {
            return Err(Refusal::ConfirmationMismatch.into());
        }
        // Rule 5: the file's set, checked against the carried index and against what is installed.
        let ix = &carried.index;
        let set: BTreeMap<String, (&str, &str)> = pkg
            .toc
            .packages
            .iter()
            .map(|p| (p.name.clone(), (p.name.as_str(), p.version.as_str())))
            .collect();
        let mut resolved = BTreeMap::new();
        for p in &pkg.toc.packages {
            let e = ix
                .entry(&p.name, &p.version)
                .ok_or_else(|| InstallError::MissingFromFile(key(&p.name, &p.version)))?;
            let mut deps = BTreeMap::new();
            for (d, r) in &e.dependencies {
                let req = crate::qrtz_json::parse_range(d, r)?;
                let (_, dv) = set
                    .get(d)
                    .ok_or_else(|| InstallError::MissingFromFile(format!("{d}@{r}")))?;
                if !semver::Version::parse(dv).is_ok_and(|v| req.matches(&v)) {
                    return Err(InstallError::MissingFromFile(format!("{d}@{r}")));
                }
                deps.insert(d.clone(), dv.to_string());
            }
            resolved.insert(
                key(&p.name, &p.version),
                crate::resolve::ResolvedPackage {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    entry: e.clone(),
                    dependencies: deps,
                },
            );
        }
        crate::resolve::check_readers(&resolved)?;
        let mut next = lock.clone();
        for (k, p) in &resolved {
            let mut l = locked_from_entry(&p.entry, &p.dependencies, LockSource::Sideload);
            l.stale_revocation = !fresh;
            match next.packages.get(k) {
                Some(have) if have.sha256 != l.sha256 => {
                    return Err(ModuleError::ContentChanged {
                        id: p.name.clone(),
                        version: p.version.clone(),
                    }
                    .into())
                }
                Some(_) => {}
                None => {
                    next.packages.insert(k.clone(), l);
                }
            }
        }
        next.workspace
            .dependencies
            .insert(target.name.clone(), format!("^{}", target.version));
        next.workspace
            .resolved
            .insert(target.name.clone(), target.version.clone());
        let next = next.without_unreachable();
        next.validate()?;
        let base = crate::sha256_hex(&lock.to_bytes());
        let plan = self.finish_plan(
            Op::Frozen,
            lock,
            next,
            false,
            Some(carried.timestamp.index.sha256.clone()),
            Some(&carried),
            base,
        )?;
        // Every blob comes from the file.
        for b in &plan.fetch {
            let tmp = self.store.tmp();
            fsutil::mkdir_private(&tmp)?;
            let t = tmp.join(format!("{}.sideload", b.sha256));
            if !pkg.copy_to(&layout::blob(&b.sha256), &t)?
                || !self.store.insert_file(&t, &b.sha256, b.size)?
            {
                return Err(InstallError::MissingFromFile(b.sha256.clone()));
            }
        }
        self.swap(plan.wants.values().collect(), &plan.next)?;
        trust.note_clock(now_ms);
        trust.save(&self.trust_path())?;
        Ok(Applied {
            changes: plan.changes,
            stale_revocation: !fresh,
            notices,
        })
    }

    // ---- prune, sweep, list, migrate ----

    /// Remove package dirs `qrtz.lock` doesn't name, store blobs no locked package's tree references (refcounted
    /// from the lock: a blob shared by two packages stays while either is locked), and stale staging dirs. Every
    /// locked package is kept, so nothing [`Installer::list`] shows (each with its users) is ever deleted here.
    pub fn prune(&self) -> Result<Pruned, InstallError> {
        let _g = self.guard()?;
        self.sweep(true)
    }

    /// What [`Installer::prune`] would delete right now, deleting nothing: the module manager offers "clean up"
    /// only when this isn't empty, so the button and the prune can't disagree.
    pub fn reclaimable(&self) -> Result<Pruned, InstallError> {
        self.sweep(false)
    }

    fn sweep(&self, delete: bool) -> Result<Pruned, InstallError> {
        let lock = self.lock()?;
        let mut out = Pruned::default();
        let keep_dirs: BTreeSet<String> = lock
            .packages
            .keys()
            .filter_map(|k| split_key(k).map(|(n, v)| package_dir(n, v)))
            .collect();
        let base = self.root.join(PACKAGES);
        if let Ok(names) = std::fs::read_dir(&base) {
            for n in names.flatten() {
                let name = n.file_name().to_string_lossy().into_owned();
                if name.starts_with(".staging-") {
                    if delete {
                        fsutil::remove_tree(&n.path())?;
                    }
                    continue;
                }
                let Ok(vs) = std::fs::read_dir(n.path()) else {
                    continue;
                };
                for v in vs.flatten() {
                    let rel = format!("{PACKAGES}/{name}/{}", v.file_name().to_string_lossy());
                    if !keep_dirs.contains(&rel) {
                        if delete {
                            fsutil::remove_tree(&v.path())?;
                        }
                        out.dirs.push(rel);
                    }
                }
                if delete {
                    let _ = std::fs::remove_dir(n.path());
                }
            }
        }
        let mut refs: BTreeMap<String, usize> = BTreeMap::new();
        for l in lock.packages.values() {
            *refs.entry(l.sha256.clone()).or_default() += 1;
            let tree = self
                .store
                .read_verified(&l.sha256, crate::index::MAX_INDEX_BYTES)?
                .and_then(|b| Tree::parse(&b).ok());
            match tree {
                Some(t) => {
                    for f in &t.files {
                        *refs.entry(f.sha256.clone()).or_default() += 1;
                    }
                }
                // Without its tree we can't know its blobs: keep everything rather than guess.
                None => return Err(InstallError::NoTree(key("?", &l.sha256))),
            }
        }
        for (sha, size) in self.store.list() {
            if !refs.contains_key(&sha) {
                if delete {
                    self.store.remove(&sha)?;
                }
                out.blobs += 1;
                out.bytes += size;
            }
        }
        Ok(out)
    }

    /// Every locked package, for `qrtz ls` and the extensions screen; a workspace request's users are unknown here
    /// (see [`Installer::list_with`]).
    pub fn list(&self) -> Result<Vec<Listed>, InstallError> {
        self.list_with(&|_| vec![])
    }

    /// [`Installer::list`], naming who each workspace request is for with `pinned_for` (the app: the modules whose
    /// models it pinned).
    pub fn list_with(&self, pinned_for: &dyn Fn(&str) -> Vec<String>) -> Result<Vec<Listed>, InstallError> {
        let lock = self.lock()?;
        Ok(lock
            .packages
            .iter()
            .filter_map(|(k, l)| {
                let (n, v) = split_key(k)?;
                Some(Listed {
                    name: n.into(),
                    version: v.into(),
                    kind: l.kind,
                    requested: lock
                        .workspace
                        .resolved
                        .get(n)
                        .filter(|x| *x == v)
                        .and_then(|_| lock.workspace.dependencies.get(n).cloned()),
                    required_by: lock
                        .required_by(n, v)
                        .into_iter()
                        .filter(|x| !x.is_empty())
                        .collect(),
                    used_by: lock.users(n, v, pinned_for),
                    disabled: l.disabled.clone(),
                    size: l.size,
                })
            })
            .collect())
    }

    /// ADR 0146's `modules/modules.lock.json`, if this root has one: its modules become requests for the next
    /// online install (returned), and the old file is renamed `.migrated`. Never fetches.
    pub fn migrate_legacy(&self) -> Result<BTreeMap<String, String>, InstallError> {
        let p = self.root.join(LEGACY_LOCK);
        if !p.exists() {
            return Ok(BTreeMap::new());
        }
        let old = crate::lock::Lockfile::load(&p)?;
        let reqs = crate::qrtz_lock::migrate_legacy(&old);
        std::fs::rename(&p, p.with_extension("json.migrated"))?;
        Ok(reqs)
    }

    /// The models a service may be handed for an extension (ADR 0157 §7): each (service, model, dir) pair where the
    /// extension depends on both and the service's `qrtz.json` declares the model. The shell opens the dir and passes
    /// the handle (or a sandbox extension token) at connect; the service never opens a path itself.
    /// ADR 0157 §9: the dir an extension's own process may load `model` from (ctrl's `model.laya`), straight from
    /// the store-linked package: only a [`crate::qrtz_json::EXTENSION_READS`] model, installed and not disabled,
    /// every file read-only and of its locked size (cheap, at every load; the sha256 was re-checked after linking,
    /// and `verify_sha256` re-checks it now as well).
    pub fn extension_read_path(
        &self,
        extension: &str,
        model: &str,
        verify_sha256: bool,
    ) -> Result<PathBuf, InstallError> {
        if !crate::qrtz_json::EXTENSION_READS.contains(&(extension, model)) {
            return Err(InstallError::NotLocked(format!("{extension} reads {model}")));
        }
        let lock = self.lock()?;
        let v = lock
            .workspace
            .resolved
            .get(model)
            .cloned()
            .or_else(|| lock.versions(model).last().map(|(v, _)| v.to_string()))
            .ok_or_else(|| InstallError::NotInstalled(model.into()))?;
        let l = lock
            .get(model, &v)
            .ok_or_else(|| InstallError::NotInstalled(model.into()))?;
        if l.disabled.is_some() {
            return Err(InstallError::NotInstalled(model.into()));
        }
        let dir = self.package_path(model, &v);
        let tree = self
            .store
            .read_verified(&l.sha256, crate::index::MAX_INDEX_BYTES)?
            .map(|b| Tree::parse(&b))
            .transpose()?
            .ok_or_else(|| InstallError::NoTree(key(model, &v)))?;
        for f in &tree.files {
            let p = dir.join(&f.path);
            let ok = if verify_sha256 {
                file_is_immutable_copy(&p, &f.sha256, f.size)?
            } else {
                std::fs::symlink_metadata(&p)
                    .is_ok_and(|m| m.is_file() && m.len() == f.size && m.permissions().readonly())
            };
            if !ok {
                return Err(InstallError::ManifestMismatch {
                    key: key(model, &v),
                    why: format!("{}: missing, changed or writable", f.path),
                });
            }
        }
        Ok(dir)
    }

    pub fn model_grants(&self, extension: &str) -> Result<Vec<(String, String, PathBuf)>, InstallError> {
        let lock = self.lock()?;
        let v = lock
            .workspace
            .resolved
            .get(extension)
            .ok_or_else(|| InstallError::NotInstalled(extension.into()))?;
        let e = lock
            .get(extension, v)
            .ok_or_else(|| InstallError::NotInstalled(extension.into()))?;
        let mut out = vec![];
        for (s, sv) in &e.dependencies {
            let Some(sl) = lock.get(s, sv).filter(|l| l.kind == PackageKind::Service) else {
                continue;
            };
            for (m, mv) in &e.dependencies {
                if sl.models.contains(m) && lock.get(m, mv).is_some_and(|l| l.disabled.is_none()) {
                    out.push((key(s, sv), key(m, mv), self.package_path(m, mv)));
                }
            }
        }
        Ok(out)
    }
}

/// The lock entry an index entry and its resolution make.
pub fn locked_from_entry(e: &PackageEntry, deps: &BTreeMap<String, String>, source: LockSource) -> Locked {
    Locked {
        kind: e.kind,
        sha256: e.sha256.clone(),
        size: e.size,
        team_id: e.team_id.clone(),
        identifier: e.identifier.clone(),
        designated_requirement: e.designated_requirement.clone(),
        dependencies: deps.clone(),
        task: e.task.clone(),
        models: e.models.clone(),
        source,
        stale_revocation: false,
        disabled: None,
        notarization: e.notarization.clone(),
    }
}

fn revoked_in<'a>(
    list: Option<&RevocationSnapshot>,
    pkgs: impl Iterator<Item = (&'a str, &'a str)>,
) -> Result<(), Refusal> {
    let Some(s) = list else { return Ok(()) };
    for (n, v) in pkgs {
        if let Some(r) = s.revokes(n, v) {
            return Err(Refusal::Revoked {
                package: n.into(),
                version: v.into(),
                reason: r.reason.clone(),
            });
        }
    }
    Ok(())
}

/// Rule 6 on `qrtz.lock`: what a fresh list revokes is disabled (with a notice), an extension depending on a
/// disabled package is disabled too, and everything else loses its stale mark.
pub fn recheck_lock(lock: &QrtzLock, fresh: &RevocationSnapshot) -> (QrtzLock, Vec<Notice>) {
    let mut next = lock.clone();
    let mut notices = vec![];
    let mut dead: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (k, l) in next.packages.iter_mut() {
        let (n, v) = split_key(k).expect("valid");
        match fresh.revokes(n, v) {
            Some(r) => {
                if l.disabled.is_none() {
                    notices.push(Notice {
                        module: n.into(),
                        version: v.into(),
                        revoked: n.into(),
                        reason: r.reason.clone(),
                    });
                }
                l.disabled = Some(r.reason.clone());
                dead.insert(k.clone(), (n.into(), r.reason.clone()));
            }
            None => l.stale_revocation = false,
        }
    }
    let snapshot = next.packages.clone();
    for (k, l) in next.packages.iter_mut() {
        if l.kind != PackageKind::Extension || dead.contains_key(k) {
            continue;
        }
        let hit = l
            .dependencies
            .iter()
            .find_map(|(d, dv)| dead.get(&key(d, dv)).cloned());
        if let Some((pkg, reason)) = hit {
            let (n, v) = split_key(k).expect("valid");
            if snapshot.get(k).is_some_and(|x| x.disabled.is_none()) {
                notices.push(Notice {
                    module: n.into(),
                    version: v.into(),
                    revoked: pkg,
                    reason: reason.clone(),
                });
            }
            l.disabled = Some(reason);
        }
    }
    (next, notices)
}

/// The file has exactly this size and sha256 and no write bit.
fn file_is_immutable_copy(p: &Path, sha256: &str, size: u64) -> std::io::Result<bool> {
    let m = std::fs::symlink_metadata(p)?;
    if !m.is_file() || m.len() != size || !m.permissions().readonly() {
        return Ok(false);
    }
    let (n, h) = fsutil::sha256_file(p)?;
    Ok(n == size && h == sha256)
}

fn make_read_only(dir: &Path) -> std::io::Result<()> {
    for x in std::fs::read_dir(dir)? {
        let x = x?;
        if x.file_type()?.is_dir() {
            make_read_only(&x.path())?;
        }
    }
    fsutil::set_mode(dir, 0o500)
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
    async fn get_range(&self, src: &Source, sha256: &str, start: u64, len: u64) -> Result<Vec<u8>, String> {
        match src {
            Source::Host(0) => self
                .0
                .read_range(&layout::blob(sha256), start, len)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("{sha256}: not in the file")),
            _ => Err("a sideload fetches nothing".into()),
        }
    }
}

/// `latest` is a newer semver than `current` (an unparsable one is never newer): the app's update check.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (semver::Version::parse(latest), semver::Version::parse(current)) {
        (Ok(l), Ok(c)) => l > c,
        _ => false,
    }
}

/// `Index` lookups the CLI and the app share.
pub fn newest(index: &Index, name: &str) -> Option<String> {
    index.latest(name).map(|(v, _)| v.to_owned())
}
