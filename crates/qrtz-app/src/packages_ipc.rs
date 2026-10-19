//! ADR 0157: `packages:*`, the Extensions screen's channels, over the same [`Packages`] as the `qrtz` CLI and the
//! speech model download. A change is two calls: `packages:plan` shows what would be added, removed and downloaded
//! (and keeps that plan here), then `packages:apply` carries out exactly that plan, named by its hash and download
//! size, so a renderer can't install anything the user wasn't shown. No package code runs on install.
//!
//! ADR 0161: `packages:list` also answers the marketplace's catalog (`available`), read from the stored signed index
//! only (never the network), and `packages:apply` reports its download as `packages:progress` events.
use crate::module_hosting::{Change, InstallError, Op, Packages, Plan, RefreshConsent};
use crate::ports::PackagesPort;
use crate::service::Events;
use async_trait::async_trait;
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_modules::index::{Index, PackageKind};
use serde_json::{json, Value};
use std::collections::{BTreeSet, VecDeque};
use std::sync::Mutex;

pub struct PackagesIpc {
    packages: Packages,
    plan: Mutex<Option<Plan>>,
    events: Option<Events>,
}

/// The highest unrevoked version of `name` matching `range` (`*` when the range doesn't parse).
fn pick<'a>(
    index: &'a Index,
    name: &str,
    range: &str,
) -> Option<(String, &'a qrtz_modules::index::PackageEntry)> {
    let req = semver::VersionReq::parse(range).unwrap_or(semver::VersionReq::STAR);
    index
        .matching(name, &req)
        .into_iter()
        .next()
        .map(|(v, e)| (v.to_string(), e))
}

/// One marketplace card: an extension's newest unrevoked version, everything it pulls in (breadth first, one
/// version per name, the way the resolver picks the highest match), the size of all of it, and the macOS
/// permissions it uses (from the shell's bundled manifest of that module; none for an extension this build
/// doesn't know yet, whose permissions are asked for when it first needs them).
pub fn catalog(index: &Index) -> Vec<Value> {
    let mut out = vec![];
    for name in index.packages.keys() {
        let Some((version, entry)) = index.latest(name) else {
            continue;
        };
        if entry.kind != PackageKind::Extension {
            continue;
        }
        let mut versions: Vec<semver::Version> = index.packages[name]
            .keys()
            .filter(|v| !index.is_revoked(name, v))
            .filter_map(|v| semver::Version::parse(v).ok())
            .collect();
        versions.sort_by(|a, b| b.cmp(a));
        let direct: BTreeSet<&String> = entry.dependencies.keys().collect();
        let mut seen = BTreeSet::from([name.clone()]);
        let mut deps = vec![];
        let mut total = entry.size;
        let mut queue: VecDeque<(String, String)> = entry
            .dependencies
            .iter()
            .map(|(n, r)| (n.clone(), r.clone()))
            .collect();
        while let Some((dep, range)) = queue.pop_front() {
            if !seen.insert(dep.clone()) {
                continue;
            }
            match pick(index, &dep, &range) {
                Some((v, e)) => {
                    total += e.size;
                    deps.push(json!({"name": dep, "range": range, "version": v, "kind": kind(e.kind),
                                     "sizeBytes": e.size, "direct": direct.contains(&dep)}));
                    queue.extend(e.dependencies.iter().map(|(n, r)| (n.clone(), r.clone())));
                }
                None => deps.push(json!({"name": dep, "range": range, "version": null,
                                         "kind": kind(qrtz_modules::qrtz_json::kind_of(&dep).unwrap_or(PackageKind::Model)),
                                         "sizeBytes": 0, "direct": direct.contains(&dep)})),
            }
        }
        let permissions: Vec<Value> = crate::modules::bundled()
            .manifests
            .iter()
            .find(|m| &m.id == name)
            .map(|m| {
                m.permissions
                    .iter()
                    .map(|p| serde_json::to_value(p).unwrap())
                    .collect()
            })
            .unwrap_or_default();
        out.push(json!({
            "name": name, "kind": "extension", "version": version,
            "versions": versions.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
            "sizeBytes": entry.size, "installSizeBytes": total, "dependencies": deps,
            "permissions": permissions, "minOs": entry.min_os, "teamId": entry.team_id,
        }));
    }
    out
}

/// At most one `packages:progress` per percent (and always the first and last), so a 600 MB model doesn't flood
/// the bus.
struct Throttle(Mutex<i64>);
impl Throttle {
    fn due(&self, done: u64, total: u64) -> bool {
        let pct = if total == 0 {
            100
        } else {
            (done.saturating_mul(100) / total) as i64
        };
        let mut last = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if pct != *last || done >= total {
            *last = pct;
            true
        } else {
            false
        }
    }
}

fn kind(k: PackageKind) -> &'static str {
    match k {
        PackageKind::Extension => "extension",
        PackageKind::Service => "service",
        PackageKind::Model => "model",
        PackageKind::Shell => "shell",
    }
}

fn changes(c: &[Change]) -> Value {
    Value::Array(
        c.iter()
            .map(|c| match c {
                Change::Add { name, version } => json!({"change": "add", "name": name, "version": version}),
                Change::Remove { name, version } => {
                    json!({"change": "remove", "name": name, "version": version})
                }
            })
            .collect(),
    )
}

fn err(e: InstallError) -> AppError {
    let (code, retry) = match &e {
        InstallError::Network(_) | InstallError::Busy => (ErrorCode::Unavailable, true),
        InstallError::NoIndex => (ErrorCode::NotFound, false),
        InstallError::Refused(_) => (ErrorCode::Forbidden, false),
        InstallError::Resolve(_)
        | InstallError::ConsentMismatch
        | InstallError::StalePlan
        | InstallError::NotLocked(_)
        | InstallError::NotInstalled(_) => (ErrorCode::Validation, false),
        _ => (ErrorCode::Internal, false),
    };
    AppError::new(code, e.to_string()).retryable(retry)
}

fn now_ms() -> u64 {
    crate::pm_cli::now_ms()
}

impl PackagesIpc {
    pub fn new(packages: Packages) -> Self {
        Self {
            packages,
            plan: Mutex::new(None),
            events: None,
        }
    }

    /// Report `packages:apply`'s download as `packages:progress` (the marketplace's install bar).
    pub fn with_events(mut self, events: Events) -> Self {
        self.events = Some(events);
        self
    }

    fn state(&self) -> Result<Value, AppError> {
        let rows: Vec<Value> = self
            .packages
            .list()
            .map_err(err)?
            .into_iter()
            .filter(|l| l.kind != PackageKind::Shell)
            .map(|l| {
                json!({"name": l.name, "version": l.version, "kind": kind(l.kind), "requested": l.requested,
                       "requiredBy": l.required_by, "disabled": l.disabled, "sizeBytes": l.size})
            })
            .collect();
        let index = self
            .packages
            .installer
            .trust()
            .map_err(err)?
            .and_then(|t| t.state.revocation)
            .map(|r| json!({"version": r.version, "issuedMs": r.issued_ms}));
        // The catalog is the stored index (signatures re-checked); none before the first check, no network here.
        let available = match self.packages.installer.cached_index() {
            Ok((_, v)) => catalog(&v.index),
            Err(_) => vec![],
        };
        Ok(json!({"packages": rows, "index": index, "available": available}))
    }

    fn op(p: &Value) -> Result<Op, AppError> {
        let names = |k: &str| -> Vec<String> {
            p[k].as_array()
                .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_owned)).collect())
                .unwrap_or_default()
        };
        let s = |k: &str| p[k].as_str().unwrap_or_default().to_owned();
        Ok(match p["op"].as_str() {
            Some("install") => Op::Install(names("names").into_iter().map(|n| (n, None)).collect()),
            Some("remove") => Op::Remove(names("names")),
            Some("update") => Op::Update {
                names: names("names"),
                latest: p["latest"].as_bool().unwrap_or(false),
            },
            Some("engine") => Op::Engine {
                extension: s("extension"),
                model: s("model"),
            },
            _ => return Err(AppError::new(ErrorCode::Validation, "unknown op")),
        })
    }
}

#[async_trait]
impl PackagesPort for PackagesIpc {
    async fn call(&self, channel: &str, p: Value) -> Result<Value, AppError> {
        match channel {
            "packages:list" => self.state(),
            "packages:refresh" => {
                self.packages
                    .refresh(&RefreshConsent::user_asked(), now_ms())
                    .await
                    .map_err(err)?;
                self.state()
            }
            "packages:plan" => {
                let plan = self.packages.plan(Self::op(&p)?).map_err(err)?;
                let out = json!({"planSha256": plan.plan_sha256, "changes": changes(&plan.changes),
                                 "downloadBytes": plan.download_bytes});
                *self.plan.lock().expect("plan") = Some(plan);
                Ok(out)
            }
            "packages:apply" => {
                let sha = p["planSha256"].as_str().unwrap_or_default();
                let bytes = p["downloadBytes"].as_u64().unwrap_or(u64::MAX);
                let plan = {
                    let mut g = self.plan.lock().expect("plan");
                    match g.as_ref() {
                        Some(pl) if pl.plan_sha256 == sha && pl.download_bytes == bytes => g.take(),
                        _ => None,
                    }
                }
                .ok_or_else(|| {
                    AppError::new(ErrorCode::Validation, "not the plan that was shown; plan again")
                })?;
                let throttle = Throttle(Mutex::new(-1));
                let events = self.events.clone();
                let progress = move |done: u64, total: u64| {
                    if let Some(e) = &events {
                        if throttle.due(done, total) {
                            e.emit(
                                "packages:progress",
                                json!({"doneBytes": done, "totalBytes": total}),
                            );
                        }
                    }
                };
                let a = self
                    .packages
                    .apply(&plan, &plan.consent(), now_ms(), &progress)
                    .await
                    .map_err(err)?;
                Ok(json!({"changes": changes(&a.changes), "staleRevocation": a.stale_revocation}))
            }
            "packages:prune" => {
                let r = self.packages.prune().map_err(err)?;
                Ok(json!({"dirs": r.dirs.len(), "blobs": r.blobs, "bytes": r.bytes}))
            }
            _ => Err(AppError::new(ErrorCode::UnknownChannel, channel.to_owned())),
        }
    }
}
