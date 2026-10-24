//! `qrtz.lock` (ADR 0157 §4): the text lockfile, like `bun.lock`. One file records the user's requests (the
//! workspace) and the exact resolved set; it is pretty-printed JSON with sorted keys and one package per key, so a
//! change diffs as the packages that changed.
//!
//! ```json
//! {
//!   "lockfileVersion": 1,
//!   "packages": {
//!     "echo@0.3.0": {"kind": "extension", "sha256": "…", "teamId": "…", "designatedRequirement": "…",
//!                    "dependencies": {"inference.voice": "1.0.0", "model.parakeet-tdt-v2-en": "1.0.0"}, …},
//!     …
//!   },
//!   "workspace": {"dependencies": {"echo": "^0.3.0"}, "resolved": {"echo": "0.3.0"}}
//! }
//! ```
//!
//! - Every package is keyed `name@exact-version`, with its tree sha256, size, team id, and (for code) identifier
//!   and designated requirement: everything an offline reinstall checks. Two versions of one package can both be
//!   here (side by side) when two ranges don't overlap.
//! - `dependencies` inside a package are exact versions: the resolution, not the ranges.
//! - Ranges are resolved only online against the signed index. Offline and sideload installs use this file as it
//!   is (`--frozen`), and nothing else.
//! - The file is written last, through temp + rename: that rename is the install (the atomic swap of the whole
//!   resolved set). Package dirs it doesn't name are garbage for `prune`.
use crate::index::PackageKind;
use crate::ModuleError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const LOCK_FILE: &str = "qrtz.lock";
pub const LOCKFILE_VERSION: u32 = 1;
pub const MAX_LOCK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// Resolved online against the signed index.
    Network,
    /// From a `.qrtzpkg` the user picked.
    Sideload,
    /// Pinned in the qrtz binary itself (today's model sets until the hosted index carries them, ADR 0157 §6).
    Pinned,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workspace {
    /// What the user asked for: name → range (`qrtz install echo` adds `"echo": "^<newest>"`).
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    /// The exact version each request resolved to.
    #[serde(default)]
    pub resolved: BTreeMap<String, String>,
    /// Engine swaps (ADR 0157 §5): extension → (dependency it declares → `replacement@range`). Switching echo from
    /// Parakeet to Nemotron is `{"echo": {"model.parakeet-tdt-v2-en": "model.nemotron@^1.0.0"}}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Locked {
    pub kind: PackageKind,
    /// The package tree's sha256 (ADR 0157 §4).
    pub sha256: String,
    pub size: u64,
    pub team_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designated_requirement: Option<String>,
    /// name → exact version.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Service: the models it may be handed. Extension: the models its own process reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    pub source: Source,
    /// Installed while the revocation list was expired or missing (sideload rule 6); rechecked on the next fresh one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale_revocation: bool,
    /// Set when a fresh list revokes it (or, for an extension, one of its dependencies): it doesn't run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<String>,
    /// Code only (ADR 0170): the signed index's notarization record, kept so a lock-only reinstall checks it too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notarization: Option<crate::index::Notarization>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QrtzLock {
    pub lockfile_version: u32,
    #[serde(default)]
    pub workspace: Workspace,
    /// `name@version` → package.
    #[serde(default)]
    pub packages: BTreeMap<String, Locked>,
}

impl Default for QrtzLock {
    fn default() -> Self {
        Self {
            lockfile_version: LOCKFILE_VERSION,
            workspace: Workspace::default(),
            packages: BTreeMap::new(),
        }
    }
}

pub fn key(name: &str, version: &str) -> String {
    format!("{name}@{version}")
}

/// `name@version` → (name, version). Names have no `@`.
pub fn split_key(k: &str) -> Option<(&str, &str)> {
    k.split_once('@')
}

/// Where a package's files are linked, relative to the root: `packages/<name>/<version>`.
pub fn package_dir(name: &str, version: &str) -> String {
    format!("packages/{name}/{version}")
}

/// An override value: `model.nemotron@^1.0.0` → (`model.nemotron`, `^1.0.0`).
pub fn parse_override(v: &str) -> Result<(String, semver::VersionReq), ModuleError> {
    let (n, r) = v
        .split_once('@')
        .ok_or_else(|| ModuleError::invalid("overrides", format!("{v:?}: <name>@<range>")))?;
    crate::qrtz_json::check_name(n)?;
    Ok((n.to_owned(), crate::qrtz_json::parse_range("overrides", r)?))
}

impl QrtzLock {
    pub fn parse(bytes: &[u8]) -> Result<Self, ModuleError> {
        if bytes.len() > MAX_LOCK_BYTES {
            return Err(ModuleError::Lock("too large".into()));
        }
        let l: Self = serde_json::from_slice(bytes).map_err(|e| ModuleError::Lock(e.to_string()))?;
        l.validate()?;
        Ok(l)
    }

    /// Pretty, sorted, newline-terminated: the same state always writes the same bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let v = serde_json::to_value(self).expect("plain json");
        let mut b = serde_json::to_vec_pretty(&v).expect("plain json");
        b.push(b'\n');
        b
    }

    pub fn get(&self, name: &str, version: &str) -> Option<&Locked> {
        self.packages.get(&key(name, version))
    }

    /// Every version of `name` in the lock.
    pub fn versions(&self, name: &str) -> Vec<(&str, &Locked)> {
        self.packages
            .iter()
            .filter_map(|(k, l)| split_key(k).filter(|(n, _)| *n == name).map(|(_, v)| (v, l)))
            .collect()
    }

    pub fn validate(&self) -> Result<(), ModuleError> {
        let bad = |s: String| Err(ModuleError::Lock(s));
        if self.lockfile_version != LOCKFILE_VERSION {
            return bad(format!("unsupported lockfileVersion {}", self.lockfile_version));
        }
        for (k, p) in &self.packages {
            let Some((name, v)) = split_key(k) else {
                return bad(format!("{k}: name@version"));
            };
            let kind = crate::qrtz_json::kind_of(name)?;
            crate::manifest::exact_version(k, v)?;
            if kind != p.kind {
                return bad(format!("{k}: kind {:?} doesn't match the name", p.kind));
            }
            if !crate::manifest::is_sha256(&p.sha256) || !crate::manifest::is_team_id(&p.team_id) {
                return bad(format!("{k}: sha256 or teamId malformed"));
            }
            let code = kind.has_code();
            if code != p.identifier.is_some() || code != p.designated_requirement.is_some() {
                return bad(format!(
                    "{k}: identifier and designatedRequirement only (and always) on code"
                ));
            }
            if let Some(id) = &p.identifier {
                if Some(id.clone()) != crate::qrtz_json::identifier_for(name, kind)
                    || p.designated_requirement.as_deref()
                        != Some(&crate::manifest::designated_requirement(id, &p.team_id))
                {
                    return bad(format!(
                        "{k}: designated requirement isn't the Developer ID one for {id}"
                    ));
                }
            }
            for (d, dv) in &p.dependencies {
                if self.get(d, dv).is_none() {
                    return bad(format!("{k}: depends on {d}@{dv}, which isn't in the lock"));
                }
            }
        }
        for (n, r) in &self.workspace.dependencies {
            crate::qrtz_json::check_name(n)?;
            let req = crate::qrtz_json::parse_range(&format!("workspace.dependencies.{n}"), r)?;
            match self.workspace.resolved.get(n) {
                Some(v) if self.get(n, v).is_some() => {
                    if !semver::Version::parse(v).is_ok_and(|s| req.matches(&s)) {
                        return bad(format!("workspace: {n}@{v} doesn't satisfy {r}"));
                    }
                }
                _ => return bad(format!("workspace: {n} isn't resolved")),
            }
        }
        if self.workspace.resolved.len() != self.workspace.dependencies.len() {
            return bad("workspace: resolved names something not requested".into());
        }
        for (ext, o) in &self.workspace.overrides {
            for (from, to) in o {
                crate::qrtz_json::check_name(ext)?;
                crate::qrtz_json::check_name(from)?;
                parse_override(to)?;
            }
        }
        let reach = self.reachable();
        if let Some(k) = self.packages.keys().find(|k| !reach.contains(*k)) {
            return bad(format!(
                "{k}: nothing requires it (a removed package leaves the lock)"
            ));
        }
        Ok(())
    }

    /// Every `name@version` reachable from the workspace.
    pub fn reachable(&self) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut todo: Vec<String> = self.workspace.resolved.iter().map(|(n, v)| key(n, v)).collect();
        while let Some(k) = todo.pop() {
            if !seen.insert(k.clone()) {
                continue;
            }
            if let Some(p) = self.packages.get(&k) {
                todo.extend(p.dependencies.iter().map(|(n, v)| key(n, v)));
            }
        }
        seen
    }

    /// Who depends on `name@version` (the workspace counts as `""`).
    pub fn required_by(&self, name: &str, version: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .packages
            .iter()
            .filter(|(_, p)| p.dependencies.get(name).is_some_and(|x| x == version))
            .map(|(k, _)| k.clone())
            .collect();
        if self.workspace.resolved.get(name).is_some_and(|x| x == version) {
            v.push(String::new());
        }
        v
    }

    /// Who uses `name@version`, by name: the one answer the module manager's "used by" label, `qrtz ls` and
    /// `prune` share. It walks up through every locked package that depends on it to the extensions at the top
    /// (`echo`, not `echo@0.3.0`; a service in between isn't a user, the extension above it is). Where the walk
    /// reaches a request of the workspace itself (`qrtz install model.x`, or a model the app pinned for one of its
    /// modules, ADR 0163, including ones adopted from before the package manager), `pinned_for(name)` names the
    /// modules that request is for. With no extension above it, what depends on it directly. Sorted, each once.
    /// Empty only for a bare request nothing is known to use (`requested` says so) or a package nothing reaches.
    pub fn users(&self, name: &str, version: &str, pinned_for: &dyn Fn(&str) -> Vec<String>) -> Vec<String> {
        let mut out = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut todo = vec![(name.to_owned(), version.to_owned())];
        while let Some((n, v)) = todo.pop() {
            if !seen.insert(key(&n, &v)) {
                continue;
            }
            for d in self.required_by(&n, &v) {
                if d.is_empty() {
                    out.extend(pinned_for(&n));
                    continue;
                }
                let Some((dn, dv)) = split_key(&d) else {
                    continue;
                };
                if self
                    .packages
                    .get(&d)
                    .is_some_and(|p| p.kind == PackageKind::Extension)
                {
                    out.insert(dn.to_owned());
                }
                todo.push((dn.to_owned(), dv.to_owned()));
            }
        }
        if out.is_empty() {
            // Nothing above it is an extension (a service the user asked for by name): name what depends on it.
            out.extend(
                self.required_by(name, version)
                    .iter()
                    .filter_map(|d| split_key(d).map(|(n, _)| n.to_owned())),
            );
        }
        out.into_iter().collect()
    }

    /// The lock without packages nothing reaches any more.
    pub fn without_unreachable(mut self) -> Self {
        let reach = self.reachable();
        self.packages.retain(|k, _| reach.contains(k));
        self
    }

    /// A missing file is an empty lock; a corrupt one is an error (never silently reset).
    pub fn load(path: &Path) -> Result<Self, ModuleError> {
        match std::fs::read(path) {
            Ok(b) => Self::parse(&b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Validate, then temp file (0600) + fsync + rename (+ dir fsync): readers see the old set or the new one.
    pub fn save(&self, path: &Path) -> Result<(), ModuleError> {
        self.validate()?;
        crate::fsutil::write_private_atomic(path, &self.to_bytes())
    }
}

/// ADR 0146's ids → ADR 0157's names.
pub fn legacy_name(id: &str) -> String {
    match id {
        "model:parakeet-tdt-v2" => "model.parakeet-tdt-v2-en".into(),
        "model:nemotron-560ms" => "model.nemotron".into(),
        "model:nemotron3-diarizer" => "model.nemotron-diarizer".into(),
        "model:localvqe" => "model.localvqe".into(),
        "model:laya" => "model.laya".into(),
        other => match other.split_once(':') {
            Some(("xpc", n)) => format!("inference.{n}"),
            Some(("model", n)) => format!("model.{n}"),
            _ => other.into(),
        },
    }
}

/// Migrate ADR 0146's `modules.lock.json`: each installed module becomes a request `^<its version>` for the next
/// online install. Its exact pins can't be carried (they hashed content differently and named no tree), so the
/// lock starts empty and nothing is fetched here. Returns name → range.
pub fn migrate_legacy(old: &crate::lock::Lockfile) -> BTreeMap<String, String> {
    old.modules
        .iter()
        .filter(|(id, _)| crate::qrtz_json::kind_of(id).ok() == Some(PackageKind::Extension))
        // ADR 0237: a module built into qrtz.app is never requested as a package again.
        .filter(|(id, _)| !crate::is_bundled_extension(id))
        .map(|(id, m)| (id.clone(), format!("^{}", m.version)))
        .collect()
}
