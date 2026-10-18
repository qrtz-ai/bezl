//! Range resolution (ADR 0157 §5), online only, against the signed index.
//!
//! - **Dedupe**: when ranges on one package overlap, everyone gets one version, the highest that satisfies all of
//!   them. When they don't, the versions sit side by side (`packages/<name>/<version>`), each requirer pinned to its
//!   own.
//! - **Stable**: a version already in `qrtz.lock` that still satisfies its ranges is kept unless the package is being
//!   updated (`qrtz update [name]`), like bun.
//! - **Revoked** versions are never picked.
//! - **Engines**: a workspace override swaps one model dependency of one extension for another model of the same
//!   task (`echo: model.parakeet-tdt-v2-en → model.nemotron@^1.0.0`). Switching an engine is a dependency change,
//!   resolved like any other.
//! - **Readers**: every model an extension depends on must be readable by one of its services (`models` in the
//!   service's `qrtz.json`) or by the extension itself (`extension.reads`); otherwise nothing could use it, and the
//!   resolution is refused.
use crate::index::{Index, PackageEntry, PackageKind};
use crate::qrtz_lock::{parse_override, QrtzLock, Workspace};
use semver::{Version, VersionReq};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    #[error("{name}@{range}: no unrevoked version in the signed index (required by {by})")]
    NotFound { name: String, range: String, by: String },
    #[error("{0}")]
    Invalid(String),
    #[error("{extension}: an override can only swap a model for a model of the same task ({from} → {to})")]
    BadOverride {
        extension: String,
        from: String,
        to: String,
    },
    #[error("{extension}@{version}: nothing it runs can read {model} (no service of it lists it, and it doesn't read it itself)")]
    NoReader {
        extension: String,
        version: String,
        model: String,
    },
    #[error("resolution didn't settle")]
    NoFixpoint,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Update {
    /// Keep locked versions that still fit.
    #[default]
    None,
    All,
    Names(BTreeSet<String>),
}

impl Update {
    fn covers(&self, name: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Names(n) => n.contains(name),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: String,
    pub entry: PackageEntry,
    /// name → exact version, after overrides.
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// `name@version` → package.
    pub packages: BTreeMap<String, ResolvedPackage>,
    /// workspace name → exact version.
    pub resolved: BTreeMap<String, String>,
}

struct Req {
    by: String,
    name: String,
    req: VersionReq,
    range: String,
}

/// The effective dependencies of an extension after its overrides: (name, range text, range).
fn effective_deps(
    index: &Index,
    ws: &Workspace,
    ext: &str,
    e: &PackageEntry,
) -> Result<Vec<(String, String, VersionReq)>, ResolveError> {
    let mut out = vec![];
    let ov = ws.overrides.get(ext);
    for (d, r) in &e.dependencies {
        let req =
            |r: &str| crate::qrtz_json::parse_range(d, r).map_err(|e| ResolveError::Invalid(e.to_string()));
        match ov.and_then(|o| o.get(d)) {
            Some(to) => {
                let (m, mreq) = parse_override(to).map_err(|e| ResolveError::Invalid(e.to_string()))?;
                let task_of = |n: &str| {
                    index
                        .packages
                        .get(n)
                        .and_then(|vs| vs.values().find_map(|e| e.task.clone()))
                };
                let same = crate::qrtz_json::kind_of(d).ok() == Some(PackageKind::Model)
                    && crate::qrtz_json::kind_of(&m).ok() == Some(PackageKind::Model)
                    && task_of(d).is_some()
                    && task_of(d) == task_of(&m);
                if !same {
                    return Err(ResolveError::BadOverride {
                        extension: ext.into(),
                        from: d.clone(),
                        to: to.clone(),
                    });
                }
                let range = to.split_once('@').map(|x| x.1).unwrap_or_default().to_owned();
                out.push((m, range, mreq));
            }
            None => out.push((d.clone(), r.clone(), req(r)?)),
        }
    }
    Ok(out)
}

/// Resolve the workspace against the index. Pure: nothing is fetched or written.
pub fn resolve(
    index: &Index,
    ws: &Workspace,
    lock: &QrtzLock,
    update: &Update,
) -> Result<Resolution, ResolveError> {
    // Versions the dedupe pass decided each name should prefer, best first.
    let mut prefer: BTreeMap<String, Vec<Version>> = BTreeMap::new();
    for _round in 0..16 {
        let r = pass(index, ws, lock, update, &prefer)?;
        // For each name chosen at more than one version, see whether fewer versions could serve every range.
        let mut changed = false;
        for (name, reqs) in &r.reqs_by_name {
            let chosen: BTreeSet<&Version> = r
                .choice
                .iter()
                .filter(|((_, n), _)| n == name)
                .map(|(_, v)| v)
                .collect();
            if chosen.len() < 2 {
                continue;
            }
            let cands: Vec<Version> = index
                .packages
                .get(name)
                .into_iter()
                .flatten()
                .filter(|(v, _)| !index.is_revoked(name, v))
                .filter_map(|(v, _)| Version::parse(v).ok())
                .collect();
            // Greedy cover: the version satisfying the most ranges (highest on ties), then the rest.
            let mut left: Vec<&VersionReq> = reqs.iter().collect();
            let mut picks = vec![];
            while !left.is_empty() {
                let best = cands
                    .iter()
                    .map(|v| (left.iter().filter(|q| q.matches(v)).count(), v))
                    .filter(|(n, _)| *n > 0)
                    .max_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
                let Some((_, v)) = best else { break };
                picks.push(v.clone());
                left.retain(|q| !q.matches(v));
            }
            if picks.len() < chosen.len() && prefer.get(name) != Some(&picks) {
                prefer.insert(name.clone(), picks);
                changed = true;
            }
        }
        if !changed {
            return finish(index, ws, r);
        }
    }
    Err(ResolveError::NoFixpoint)
}

struct Pass {
    /// (requirer key, dep name) → chosen version.
    choice: BTreeMap<(String, String), Version>,
    reqs_by_name: BTreeMap<String, Vec<VersionReq>>,
    root: BTreeMap<String, Version>,
}

fn pass(
    index: &Index,
    ws: &Workspace,
    lock: &QrtzLock,
    update: &Update,
    prefer: &BTreeMap<String, Vec<Version>>,
) -> Result<Pass, ResolveError> {
    let mut selected: BTreeMap<String, BTreeSet<Version>> = BTreeMap::new();
    let mut choice = BTreeMap::new();
    let mut reqs_by_name: BTreeMap<String, Vec<VersionReq>> = BTreeMap::new();
    let mut root = BTreeMap::new();
    let mut todo: std::collections::VecDeque<Req> = ws
        .dependencies
        .iter()
        .map(|(n, r)| {
            Ok(Req {
                by: String::new(),
                name: n.clone(),
                req: crate::qrtz_json::parse_range(n, r).map_err(|e| ResolveError::Invalid(e.to_string()))?,
                range: r.clone(),
            })
        })
        .collect::<Result<_, ResolveError>>()?;
    let mut expanded = BTreeSet::new();
    while let Some(q) = todo.pop_front() {
        reqs_by_name
            .entry(q.name.clone())
            .or_default()
            .push(q.req.clone());
        let ok = |v: &Version| {
            q.req.matches(v)
                && index.entry(&q.name, &v.to_string()).is_some()
                && !index.is_revoked(&q.name, &v.to_string())
        };
        let sel = selected.entry(q.name.clone()).or_default();
        let pick = prefer
            .get(&q.name)
            .and_then(|p| p.iter().find(|v| ok(v)).cloned())
            .or_else(|| sel.iter().rev().find(|v| ok(v)).cloned())
            .or_else(|| {
                (!update.covers(&q.name))
                    .then(|| {
                        lock.versions(&q.name)
                            .into_iter()
                            .filter_map(|(v, _)| Version::parse(v).ok())
                            .filter(&ok)
                            .max()
                    })
                    .flatten()
            })
            .or_else(|| index.matching(&q.name, &q.req).first().map(|(v, _)| v.clone()));
        let Some(v) = pick else {
            return Err(ResolveError::NotFound {
                name: q.name,
                range: q.range,
                by: if q.by.is_empty() {
                    "the workspace".into()
                } else {
                    q.by
                },
            });
        };
        sel.insert(v.clone());
        if q.by.is_empty() {
            root.insert(q.name.clone(), v.clone());
        } else {
            choice.insert((q.by.clone(), q.name.clone()), v.clone());
        }
        let k = crate::qrtz_lock::key(&q.name, &v.to_string());
        if expanded.insert(k.clone()) {
            let e = index.entry(&q.name, &v.to_string()).expect("checked");
            for (d, range, req) in effective_deps(index, ws, &q.name, e)? {
                todo.push_back(Req {
                    by: k.clone(),
                    name: d,
                    req,
                    range,
                });
            }
        }
    }
    Ok(Pass {
        choice,
        reqs_by_name,
        root,
    })
}

fn finish(index: &Index, ws: &Workspace, p: Pass) -> Result<Resolution, ResolveError> {
    let mut packages = BTreeMap::new();
    let mut todo: Vec<(String, Version)> = p.root.iter().map(|(n, v)| (n.clone(), v.clone())).collect();
    while let Some((n, v)) = todo.pop() {
        let k = crate::qrtz_lock::key(&n, &v.to_string());
        if packages.contains_key(&k) {
            continue;
        }
        let e = index.entry(&n, &v.to_string()).expect("resolved").clone();
        let mut deps = BTreeMap::new();
        for (d, _, _) in effective_deps(index, ws, &n, &e)? {
            let dv = p.choice.get(&(k.clone(), d.clone())).expect("chosen").clone();
            deps.insert(d.clone(), dv.to_string());
            todo.push((d, dv));
        }
        packages.insert(
            k,
            ResolvedPackage {
                name: n,
                version: v.to_string(),
                entry: e,
                dependencies: deps,
            },
        );
    }
    check_readers(&packages)?;
    Ok(Resolution {
        packages,
        resolved: p.root.into_iter().map(|(n, v)| (n, v.to_string())).collect(),
    })
}

/// ADR 0157 §7: a model is reachable only through a service that declares it (or an extension that reads it).
pub fn check_readers(packages: &BTreeMap<String, ResolvedPackage>) -> Result<(), ResolveError> {
    for p in packages
        .values()
        .filter(|p| p.entry.kind == PackageKind::Extension)
    {
        for (d, dv) in &p.dependencies {
            if crate::qrtz_json::kind_of(d).ok() != Some(PackageKind::Model) {
                continue;
            }
            let by_service = p.dependencies.iter().any(|(s, sv)| {
                packages
                    .get(&crate::qrtz_lock::key(s, sv))
                    .is_some_and(|sp| sp.entry.kind == PackageKind::Service && sp.entry.models.contains(d))
            });
            if !by_service && !p.entry.models.contains(d) {
                let _ = dv;
                return Err(ResolveError::NoReader {
                    extension: p.name.clone(),
                    version: p.version.clone(),
                    model: d.clone(),
                });
            }
        }
    }
    Ok(())
}
