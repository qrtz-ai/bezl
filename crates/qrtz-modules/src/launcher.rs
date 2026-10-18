//! The launcher plan (ADR 0146 §3): which installed modules start, and what is checked before each one does.
//!
//! A module starts only if it is in the lockfile, not disabled by a revocation, switched on in the global
//! settings, compatible with this app version, its installed manifest still hashes to the lockfile's value, and
//! every pinned dependency resolves from the lockfile. Then, immediately before launching, the module's code and
//! each xpc-service it depends on are checked against their designated requirements ([`CodeVerifier`]); one
//! failure stops that module.
//!
//! M0 has no behaviour change: the host still spawns the one bundled helper. The plan is what M3 (one helper per
//! module) executes. Extensions are not apps: the host spawns each helper executable directly and stays its
//! responsible process, so every extension uses qrtz's own TCC grants (ADR 0146 M2.5; no LaunchServices).
use crate::lock::{LockedModule, Lockfile};
use crate::manifest::{DependencyKind, LaunchKind, ModuleManifest};
use crate::settings::GlobalSettings;
use crate::ModuleError;
use std::path::{Path, PathBuf};

/// ADR 0144 §5: from M1 every extension helper accepts `--qrtz-gate-probe <ax|mic|socket|ae|sysaudio>` for the Mac
/// gates (`sysaudio`: ADR 0150 N173). The launcher never passes it; only `scripts/modules/mac-gate.sh` does.
pub const GATE_PROBE_FLAG: &str = "--qrtz-gate-probe";
pub const GATE_PROBES: &[&str] = &["ax", "mic", "socket", "ae", "sysaudio"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// A signed release: the placeholder team id is refused.
    Release,
    /// A local unsigned build (dev, the box, the Mac check).
    Dev,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    NotInstalled,
    SwitchedOff,
    Revoked(String),
    ManifestChanged,
    IncompatibleApp,
    Unresolved(ModuleError),
    PlaceholderIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceCheck {
    pub id: String,
    pub path: PathBuf,
    pub requirement: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchStep {
    pub module: String,
    pub kind: LaunchKind,
    pub entry: PathBuf,
    pub requirement: String,
    pub channels: Vec<String>,
    pub services: Vec<ServiceCheck>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub launch: Vec<LaunchStep>,
    pub skipped: Vec<(String, Skip)>,
}

/// `SecStaticCodeCheckValidity` with the requirement on macOS (M6); a test double elsewhere.
pub trait CodeVerifier {
    fn verify(&self, path: &Path, requirement: &str) -> Result<(), String>;
}

pub trait Spawner {
    fn spawn(&mut self, step: &LaunchStep) -> Result<(), String>;
}

/// An installed module as read from disk for planning: its manifest and the sha256 of the manifest bytes.
pub struct Installed<'a> {
    pub manifest: &'a ModuleManifest,
    pub manifest_sha256: &'a str,
    /// Where the module's files are (bundled: the app's resources; installed: `modules/<id>/versions/<v>`).
    pub dir: PathBuf,
}

fn in_range(app: &semver::Version, m: &ModuleManifest) -> bool {
    let min = semver::Version::parse(&m.min_app).expect("validated");
    let max = m
        .max_app
        .as_deref()
        .map(|v| semver::Version::parse(v).expect("validated"));
    *app >= min && max.is_none_or(|max| *app <= max)
}

fn check_one(
    id: &str,
    lm: &LockedModule,
    inst: &Installed,
    lock: &Lockfile,
    app: &semver::Version,
    root: &Path,
    policy: Policy,
) -> Result<LaunchStep, Skip> {
    let m = inst.manifest;
    if let Some(r) = &lm.disabled {
        return Err(Skip::Revoked(r.clone()));
    }
    if m.id != id
        || m.version != lm.version
        || inst.manifest_sha256 != lm.manifest_sha256
        || m.code != lm.code
    {
        return Err(Skip::ManifestChanged);
    }
    if policy == Policy::Release && m.code.is_placeholder() {
        return Err(Skip::PlaceholderIdentity);
    }
    if !in_range(app, m) {
        return Err(Skip::IncompatibleApp);
    }
    let resolved = lock.resolve(m).map_err(Skip::Unresolved)?;
    let services = resolved
        .iter()
        .filter(|r| r.dependency.kind == DependencyKind::XpcService)
        .filter_map(|r| r.package.as_ref().map(|p| (r, p)))
        .map(|(r, p)| ServiceCheck {
            id: r.dependency.id.clone(),
            path: root.join(&p.path),
            requirement: p.designated_requirement.clone().expect("validated"),
        })
        .collect();
    Ok(LaunchStep {
        module: id.to_owned(),
        kind: m.launch.kind.clone(),
        entry: inst.dir.join(&m.launch.entry),
        requirement: m.code.designated_requirement.clone(),
        channels: m.launch.channels.clone(),
        services,
    })
}

/// `installed(id)` returns the module's manifest as found on disk. `root` is the app data dir.
pub fn plan<'a>(
    global: &GlobalSettings,
    lock: &Lockfile,
    installed: impl Fn(&str) -> Option<Installed<'a>>,
    app_version: &str,
    root: &Path,
    policy: Policy,
) -> Plan {
    let app = semver::Version::parse(app_version).expect("the app's own version");
    let mut p = Plan::default();
    for id in crate::MODULE_IDS {
        let id = (*id).to_owned();
        let Some(lm) = lock.modules.get(&id) else {
            p.skipped.push((id, Skip::NotInstalled));
            continue;
        };
        if !global.is_on(&id) {
            p.skipped.push((id, Skip::SwitchedOff));
            continue;
        }
        let Some(inst) = installed(&id) else {
            p.skipped.push((id, Skip::NotInstalled));
            continue;
        };
        match check_one(&id, lm, &inst, lock, &app, root, policy) {
            Ok(step) => p.launch.push(step),
            Err(s) => p.skipped.push((id, s)),
        }
    }
    p
}

/// Verify, then spawn, each step. A failed check never reaches the spawner.
pub fn launch(
    plan: &Plan,
    verifier: &dyn CodeVerifier,
    spawner: &mut dyn Spawner,
) -> Vec<(String, Result<(), String>)> {
    plan.launch
        .iter()
        .map(|s| {
            let r = (|| {
                for svc in &s.services {
                    verifier
                        .verify(&svc.path, &svc.requirement)
                        .map_err(|e| format!("{}: {e}", svc.id))?;
                }
                verifier.verify(&s.entry, &s.requirement)?;
                spawner.spawn(s)
            })();
            (s.module.clone(), r)
        })
        .collect()
}
