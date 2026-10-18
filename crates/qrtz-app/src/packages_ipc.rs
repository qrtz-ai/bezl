//! ADR 0157: `packages:*`, the Extensions screen's channels, over the same [`Packages`] as the `qrtz` CLI and the
//! speech model download. A change is two calls: `packages:plan` shows what would be added, removed and downloaded
//! (and keeps that plan here), then `packages:apply` carries out exactly that plan, named by its hash and download
//! size, so a renderer can't install anything the user wasn't shown. No package code runs on install.
use crate::module_hosting::{Change, InstallError, Op, Packages, Plan, RefreshConsent};
use crate::ports::PackagesPort;
use async_trait::async_trait;
use qrtz_ipc::{AppError, ErrorCode};
use qrtz_modules::index::PackageKind;
use serde_json::{json, Value};
use std::sync::Mutex;

pub struct PackagesIpc {
    packages: Packages,
    plan: Mutex<Option<Plan>>,
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
        }
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
        Ok(json!({"packages": rows, "index": index}))
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
                let a = self
                    .packages
                    .apply(&plan, &plan.consent(), now_ms(), &|_, _| {})
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
