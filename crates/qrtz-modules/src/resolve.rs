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
//! - **Reads** (ADR 0212): every runnable unit (the extension's own process, and each service it depends on)
//!   declares the models it reads (`extension.reads`, `service.models`). [`extension_grants`] is the one place that
//!   turns those declarations into grants and checks them: every model dependency is read by at least one declared
//!   unit (no orphan), and every read names one of the extension's model dependencies (no undeclared read). It runs
//!   at publish (`qrtz-publish build`), at install ([`check_readers`]), at launch ([`grants_in_lock`]) and over every
//!   `modules/*/qrtz.json` in the repo. No package or model is named in this logic: grants come from data only.
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
    #[error("{extension}@{version}: nothing it runs can read {model} (no service of it lists it, and it doesn't read it itself; declare it in extension.reads or a service's service.models)")]
    NoReader {
        extension: String,
        version: String,
        model: String,
    },
    #[error(
        "{extension}@{version}: {unit} reads {model}, which isn't one of {extension}'s model dependencies"
    )]
    ReadNotDependency {
        extension: String,
        version: String,
        unit: String,
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

/// One read-only grant (ADR 0212): `unit` (the extension's own process when `own_process`, else a service it
/// depends on) may be given `model`, one of the extension's dependencies. Versions are whatever the caller's
/// dependency map holds: exact versions after resolution or in a lock, ranges in a manifest.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Grant {
    pub extension: String,
    pub unit: String,
    pub unit_version: String,
    pub own_process: bool,
    pub model: String,
    pub model_version: String,
}

/// Resolved version and declared models of a service dependency: what the `service` callback of
/// [`extension_grants`] returns inside `Some` (`version`, then `service.models`).
pub type ServiceModels = (String, Vec<String>);

/// The single source of truth for reads (ADR 0212): the grants of extension `extension@version`, derived only from
/// what its units declare. `deps` is its dependency map, `own_reads` its `extension.reads`, and `service(name,
/// version)` the [`ServiceModels`] of a service dependency (`None` when it isn't known: it reads nothing).
/// Refused: a read by the extension's own process of a model that isn't one of its model dependencies, and a model
/// dependency no unit reads. A service (shared between extensions) is granted only the models it declares that this
/// extension depends on; nothing it declares beyond those is ever granted for this extension.
pub fn extension_grants(
    extension: &str,
    version: &str,
    deps: &BTreeMap<String, String>,
    own_reads: &[String],
    service: &dyn Fn(&str, &str) -> Option<ServiceModels>,
) -> Result<Vec<Grant>, ResolveError> {
    let kind = |n: &str| crate::qrtz_json::kind_of(n).ok();
    let mut units: Vec<(String, String, bool, Vec<String>)> =
        vec![(extension.to_owned(), version.to_owned(), true, own_reads.to_vec())];
    for (s, sv) in deps {
        if kind(s) == Some(PackageKind::Service) {
            if let Some((v, reads)) = service(s, sv) {
                units.push((s.clone(), v, false, reads));
            }
        }
    }
    let mut out = BTreeSet::new();
    for (unit, uv, own, reads) in &units {
        for m in reads {
            let dep = deps.get(m).filter(|_| kind(m) == Some(PackageKind::Model));
            // A service can serve several extensions: of what it declares, it is granted only the models this
            // extension depends on. The extension's own process has no such slack: each read is a dependency.
            let Some(mv) = dep else {
                if !own {
                    continue;
                }
                return Err(ResolveError::ReadNotDependency {
                    extension: extension.into(),
                    version: version.into(),
                    unit: unit.clone(),
                    model: m.clone(),
                });
            };
            out.insert(Grant {
                extension: extension.into(),
                unit: unit.clone(),
                unit_version: uv.clone(),
                own_process: *own,
                model: m.clone(),
                model_version: mv.clone(),
            });
        }
    }
    for d in deps.keys().filter(|d| kind(d) == Some(PackageKind::Model)) {
        if !out.iter().any(|g| &g.model == d) {
            return Err(ResolveError::NoReader {
                extension: extension.into(),
                version: version.into(),
                model: d.clone(),
            });
        }
    }
    Ok(out.into_iter().collect())
}

/// Every grant of a resolved set ([`extension_grants`] for each extension in it).
pub fn derive_grants(packages: &BTreeMap<String, ResolvedPackage>) -> Result<Vec<Grant>, ResolveError> {
    let mut out = vec![];
    for p in packages
        .values()
        .filter(|p| p.entry.kind == PackageKind::Extension)
    {
        let service = |s: &str, sv: &str| {
            packages
                .get(&crate::qrtz_lock::key(s, sv))
                .filter(|sp| sp.entry.kind == PackageKind::Service)
                .map(|sp| (sv.to_owned(), sp.entry.models.clone()))
        };
        out.extend(extension_grants(
            &p.name,
            &p.version,
            &p.dependencies,
            &p.entry.models,
            &service,
        )?);
    }
    Ok(out)
}

/// ADR 0157 §7, ADR 0212: every model is read by a unit that declares it, and nothing reads an undeclared model.
pub fn check_readers(packages: &BTreeMap<String, ResolvedPackage>) -> Result<(), ResolveError> {
    derive_grants(packages).map(|_| ())
}

/// The grants of the installed extension `extension` as `qrtz.lock` records it (its resolved version, its exact
/// dependency versions, each unit's declared reads): what a launch hands out (ADR 0212).
pub fn grants_in_lock(
    lock: &crate::qrtz_lock::QrtzLock,
    extension: &str,
) -> Result<Vec<Grant>, ResolveError> {
    let v = lock
        .workspace
        .resolved
        .get(extension)
        .ok_or_else(|| ResolveError::Invalid(format!("{extension}: not installed")))?;
    let e = lock
        .get(extension, v)
        .filter(|l| l.kind == PackageKind::Extension)
        .ok_or_else(|| ResolveError::Invalid(format!("{extension}@{v}: not in the lock")))?;
    let service = |s: &str, sv: &str| {
        lock.get(s, sv)
            .filter(|l| l.kind == PackageKind::Service)
            .map(|l| (sv.to_owned(), l.models.clone()))
    };
    extension_grants(extension, v, &e.dependencies, &e.models, &service)
}

/// Every extension version in a set of index entries (`packages[name][version]`, what `qrtz-publish build` is about
/// to sign) that isn't revoked passes [`extension_grants`]; a service range is read as its highest unrevoked
/// published version. ADR 0214: a revoked version stays in the index (published objects are immutable) but no
/// client ever resolves or installs it, so it is the one thing the check skips; every entry a client can resolve
/// must pass.
pub fn check_index_reads(
    packages: &BTreeMap<String, BTreeMap<String, PackageEntry>>,
    revoked: &[crate::sideload::Revoked],
) -> Result<(), ResolveError> {
    let is_revoked = |id: &str, v: &str| {
        revoked
            .iter()
            .any(|r| r.package == id && (r.versions.is_empty() || r.versions.iter().any(|x| x == v)))
    };
    for (name, vs) in packages {
        for (v, e) in vs
            .iter()
            .filter(|(v, e)| e.kind == PackageKind::Extension && !is_revoked(name, v))
        {
            let service = |s: &str, range: &str| {
                let req = VersionReq::parse(range).ok()?;
                packages
                    .get(s)?
                    .iter()
                    .filter(|(sv, se)| se.kind == PackageKind::Service && !is_revoked(s, sv))
                    .filter_map(|(sv, se)| Version::parse(sv).ok().map(|x| (x, se)))
                    .filter(|(x, _)| req.matches(x))
                    .max_by(|a, b| a.0.cmp(&b.0))
                    .map(|(x, se)| (x.to_string(), se.models.clone()))
            };
            extension_grants(name, v, &e.dependencies, &e.models, &service)?;
        }
    }
    Ok(())
}
