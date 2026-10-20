//! `modules.lock.json` (ADR 0146): the one local record of installed modules and the exact packages they use.
//!
//! - Resolution is a lookup: a manifest's dependency `(id, version)` must be in the lockfile with the same sha256,
//!   team id, bundle id and designated requirement. There is no range matching and no index or network lookup,
//!   so an offline install gives the same tree every time.
//! - Packages are keyed by id and exact version, so two modules that pin different versions get both side by side
//!   (`packages/<kind>/<name>/<version>`). A module's pin is never moved to another version by someone else's
//!   install, and an installed `(id, version)` never changes content ([`ModuleError::ContentChanged`]).
//! - [`Lockfile::plan_install`] returns the whole next lockfile; the caller writes it with [`Lockfile::save`]
//!   (temp file + rename), so an install applies completely or not at all.
use crate::manifest::{
    exact_version, is_sha256, is_team_id, CodeIdentity, Dependency, DependencyKind, ModuleManifest,
};
use crate::ModuleError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const LOCK_VERSION: u32 = 1;
pub const LOCK_FILE: &str = "modules.lock.json";
pub const MAX_LOCK_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// Shipped inside qrtz.app (M0-M7).
    Bundled,
    /// Fetched through the signed index on consent (M7).
    Network,
    /// Installed from a `.qrtzpkg` file the user picked ([`crate::sideload`]).
    Sideload,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedRef {
    pub id: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockedModule {
    pub version: String,
    /// sha256 of the exact manifest bytes that were installed.
    pub manifest_sha256: String,
    pub code: CodeIdentity,
    pub source: Source,
    /// Sideload rule 6: installed while the revocation list was expired or missing; rechecked on the next fresh one.
    #[serde(default)]
    pub stale_revocation: bool,
    /// Set when a fresh revocation list revokes the module or one of its packages: it doesn't launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<String>,
    pub dependencies: Vec<PinnedRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockedPackage {
    pub kind: DependencyKind,
    pub sha256: String,
    pub team_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designated_requirement: Option<String>,
    /// Where the files are, relative to the app data dir. `packages/<kind>/<name>/<version>` for new installs;
    /// today's model sets keep their `models/<dir>` (ADR 0079) until M2 moves them.
    pub path: String,
}

impl LockedPackage {
    /// The package a dependency pin describes, at its default path.
    pub fn for_dependency(d: &Dependency) -> Self {
        Self {
            kind: d.kind,
            sha256: d.sha256.clone(),
            team_id: d.team_id.clone(),
            bundle_id: d.bundle_id.clone(),
            designated_requirement: d.designated_requirement.clone(),
            path: package_dir(&d.id, &d.version),
        }
    }

    fn mismatch(&self, d: &Dependency) -> Option<&'static str> {
        if self.kind != d.kind {
            Some("kind")
        } else if self.sha256 != d.sha256 {
            Some("sha256")
        } else if self.team_id != d.team_id {
            Some("teamId")
        } else if self.bundle_id != d.bundle_id {
            Some("bundleId")
        } else if self.designated_requirement != d.designated_requirement {
            Some("designatedRequirement")
        } else {
            None
        }
    }
}

/// `packages/model/parakeet-tdt-v2/1.0.0`, one dir per exact version.
pub fn package_dir(id: &str, version: &str) -> String {
    let (kind, name) = id.split_once(':').unwrap_or(("pkg", id));
    format!("packages/{kind}/{name}/{version}")
}

#[derive(Serialize)]
struct DigestEntry<'a> {
    path: &'a str,
    sha256: &'a str,
    size: u64,
}

/// The content hash of a package: sha256 over the canonical JSON (keys sorted, no whitespace, files sorted by
/// path) of `[{path, sha256, size}]`. The same value the signed index will carry (M7), so a model set already
/// pinned file by file (local_models.json) gets one stable hash without re-reading 500 MB.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "serde_json on a plain derived type with string keys cannot fail"
)]
pub fn content_digest<'a>(files: impl IntoIterator<Item = (&'a str, u64, &'a str)>) -> String {
    let mut v: Vec<DigestEntry> = files
        .into_iter()
        .map(|(path, size, sha256)| DigestEntry { path, sha256, size })
        .collect();
    v.sort_by(|a, b| a.path.cmp(b.path));
    crate::sha256_hex(&serde_json::to_vec(&v).expect("plain json"))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lockfile {
    pub lock_version: u32,
    #[serde(default)]
    pub modules: BTreeMap<String, LockedModule>,
    /// id → exact version → package.
    #[serde(default)]
    pub packages: BTreeMap<String, BTreeMap<String, LockedPackage>>,
}

/// One resolved dependency: the manifest's pin and the installed package it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub dependency: Dependency,
    /// `None` only for an optional dependency that isn't installed.
    pub package: Option<LockedPackage>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            lock_version: LOCK_VERSION,
            modules: BTreeMap::new(),
            packages: BTreeMap::new(),
        }
    }
}

impl Lockfile {
    pub fn parse(bytes: &[u8]) -> Result<Self, ModuleError> {
        if bytes.len() > MAX_LOCK_BYTES {
            return Err(ModuleError::Lock("too large".into()));
        }
        let l: Self = serde_json::from_slice(bytes).map_err(|e| ModuleError::Lock(e.to_string()))?;
        l.validate()?;
        Ok(l)
    }

    /// Deterministic bytes (maps are sorted), so the same install state always writes the same file.
    #[allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "serde_json on a plain derived type with string keys cannot fail"
    )]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = serde_json::to_vec_pretty(self).expect("plain json");
        b.push(b'\n');
        b
    }

    pub fn validate(&self) -> Result<(), ModuleError> {
        let bad = |s: String| Err(ModuleError::Lock(s));
        if self.lock_version != LOCK_VERSION {
            return bad(format!("unsupported lockVersion {}", self.lock_version));
        }
        for (id, versions) in &self.packages {
            for (v, p) in versions {
                exact_version(&format!("packages[{id}]"), v)?;
                if !id.starts_with(p.kind.id_prefix()) {
                    return bad(format!("{id}@{v}: kind {:?} doesn't match the id", p.kind));
                }
                if !is_sha256(&p.sha256) || !is_team_id(&p.team_id) {
                    return bad(format!("{id}@{v}: sha256 or teamId malformed"));
                }
                let code = p.kind == DependencyKind::XpcService;
                if code != p.designated_requirement.is_some() || code != p.bundle_id.is_some() {
                    return bad(format!(
                        "{id}@{v}: designated requirement only (and always) on xpc-service"
                    ));
                }
                if p.path.starts_with('/') || p.path.split('/').any(|c| c.is_empty() || c == "..") {
                    return bad(format!("{id}@{v}: path must stay inside the app data dir"));
                }
            }
        }
        for (m, lm) in &self.modules {
            if !crate::MODULE_IDS.contains(&m.as_str()) {
                return bad(format!("unknown module {m}"));
            }
            exact_version(&format!("modules[{m}].version"), &lm.version)?;
            let mut ids = BTreeSet::new();
            for r in &lm.dependencies {
                if !ids.insert(&r.id) {
                    return bad(format!("{m}: {} pinned twice", r.id));
                }
                if self.package(&r.id, &r.version).is_none() {
                    return bad(format!("{m}: {}@{} is not in packages", r.id, r.version));
                }
            }
        }
        Ok(())
    }

    pub fn package(&self, id: &str, version: &str) -> Option<&LockedPackage> {
        self.packages.get(id).and_then(|v| v.get(version))
    }

    /// Every dependency of `m`, looked up exactly. Never consults anything but this lockfile.
    pub fn resolve(&self, m: &ModuleManifest) -> Result<Vec<Resolved>, ModuleError> {
        let mut out = vec![];
        for d in &m.dependencies {
            match self.package(&d.id, &d.version) {
                Some(p) => {
                    if let Some(field) = p.mismatch(d) {
                        return Err(ModuleError::Mismatch {
                            id: d.id.clone(),
                            version: d.version.clone(),
                            field,
                        });
                    }
                    out.push(Resolved {
                        dependency: d.clone(),
                        package: Some(p.clone()),
                    });
                }
                None if d.optional => out.push(Resolved {
                    dependency: d.clone(),
                    package: None,
                }),
                None => {
                    return Err(ModuleError::Missing {
                        id: d.id.clone(),
                        version: d.version.clone(),
                    })
                }
            }
        }
        Ok(out)
    }

    /// The next lockfile after installing `m` (exact bytes hashed to `manifest_sha256`) with `new_packages`
    /// (what the install brings; each must be one of `m`'s pins). Nothing is written; `self` is unchanged.
    pub fn plan_install(
        &self,
        m: &ModuleManifest,
        manifest_sha256: &str,
        source: Source,
        new_packages: &[(PinnedRef, LockedPackage)],
    ) -> Result<Self, ModuleError> {
        m.validate()?;
        if !is_sha256(manifest_sha256) {
            return Err(ModuleError::invalid("manifestSha256", "64 lowercase hex"));
        }
        let mut next = self.clone();
        for (r, p) in new_packages {
            let Some(d) = m
                .dependencies
                .iter()
                .find(|d| d.id == r.id && d.version == r.version)
            else {
                return Err(ModuleError::Lock(format!(
                    "{}@{}: not a dependency of {}; an install brings only what it pins",
                    r.id, r.version, m.id
                )));
            };
            if let Some(field) = p.mismatch(d) {
                return Err(ModuleError::Mismatch {
                    id: r.id.clone(),
                    version: r.version.clone(),
                    field,
                });
            }
            match next.package(&r.id, &r.version) {
                Some(have) if have.sha256 != p.sha256 || have.kind != p.kind => {
                    return Err(ModuleError::ContentChanged {
                        id: r.id.clone(),
                        version: r.version.clone(),
                    })
                }
                Some(_) => {}
                None => {
                    next.packages
                        .entry(r.id.clone())
                        .or_default()
                        .insert(r.version.clone(), p.clone());
                }
            }
        }
        let resolved = next.resolve(m)?;
        next.modules.insert(
            m.id.clone(),
            LockedModule {
                version: m.version.clone(),
                manifest_sha256: manifest_sha256.to_owned(),
                code: m.code.clone(),
                source,
                stale_revocation: false,
                disabled: None,
                dependencies: resolved
                    .iter()
                    .filter(|r| r.package.is_some())
                    .map(|r| PinnedRef {
                        id: r.dependency.id.clone(),
                        version: r.dependency.version.clone(),
                    })
                    .collect(),
            },
        );
        next.validate()?;
        Ok(next)
    }

    /// The next lockfile without module `id`. Its packages stay until [`Lockfile::collect_garbage`].
    pub fn plan_remove(&self, id: &str) -> Self {
        let mut next = self.clone();
        next.modules.remove(id);
        next
    }

    /// Modules that pin `(id, version)`.
    pub fn required_by(&self, id: &str, version: &str) -> Vec<String> {
        self.modules
            .iter()
            .filter(|(_, m)| m.dependencies.iter().any(|r| r.id == id && r.version == version))
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Packages no installed module pins.
    pub fn unreferenced(&self) -> Vec<PinnedRef> {
        let used: BTreeSet<&PinnedRef> = self
            .modules
            .values()
            .flat_map(|m| m.dependencies.iter())
            .collect();
        self.packages
            .iter()
            .flat_map(|(id, vs)| {
                vs.keys().map(|v| PinnedRef {
                    id: id.clone(),
                    version: v.clone(),
                })
            })
            .filter(|r| !used.contains(r))
            .collect()
    }

    /// The next lockfile without unreferenced packages, and what was dropped (the caller deletes their dirs).
    pub fn collect_garbage(&self) -> (Self, Vec<PinnedRef>) {
        let gone = self.unreferenced();
        let mut next = self.clone();
        for r in &gone {
            if let Some(vs) = next.packages.get_mut(&r.id) {
                vs.remove(&r.version);
                if vs.is_empty() {
                    next.packages.remove(&r.id);
                }
            }
        }
        (next, gone)
    }

    /// A missing file is an empty lockfile; a corrupt one is an error (never silently reset).
    pub fn load(path: &Path) -> Result<Self, ModuleError> {
        match std::fs::read(path) {
            Ok(b) => Self::parse(&b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Temp file (0600) in the same dir, fsync, rename: readers see the old or the new lockfile, never half.
    pub fn save(&self, path: &Path) -> Result<(), ModuleError> {
        use std::io::Write;
        self.validate()?;
        let dir = path
            .parent()
            .ok_or_else(|| ModuleError::Io("no parent dir".into()))?;
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{LOCK_FILE}.tmp"));
        let _ = std::fs::remove_file(&tmp);
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        let mut f = o.open(&tmp)?;
        f.write_all(&self.to_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}
