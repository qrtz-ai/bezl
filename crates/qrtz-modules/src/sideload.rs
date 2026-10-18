//! Install decisions for the two ways a module arrives (ADR 0146 §5, Bozniak's sideload rules):
//!
//! 1. A **network** install or update refuses an expired (or missing) revocation list. There is no override.
//! 2. A **`.qrtzpkg` sideload** may install with an expired list, but is checked against the newest list stored;
//!    a revoked module or package is refused, with no override.
//! 3. A `.qrtzpkg` may carry a signed revocation snapshot. It is adopted only if newer than the stored one
//!    ([`adopt_snapshot`]); one older than the stored one refuses the install as a rollback (ADR 0144 §6 gate (g)
//!    case 3, [`Refusal::RevocationRollback`]).
//! 4. Every sideload needs an explicit confirmation for that file that showed the list's age; a file found in a
//!    watched folder is refused ([`SideloadOrigin::WatchedFolder`]).
//! 5. A sideload installs exactly what is in the file: the module and its pinned packages, matching the lockfile
//!    where a package is already installed. No resolution, no fetch, no update.
//! 6. A module sideloaded with a stale list is marked [`crate::lock::LockedModule::stale_revocation`]; the next
//!    fresh list rechecks every installed module and disables (and reports) the revoked ones
//!    ([`recheck_after_fresh_list`]).
//! 7. The signature, team id, designated requirement and notarization checks are the same on both paths: both go
//!    through [`PackageChecks`] before anything else.
//!
//! Gate hooks: the confinement track's gate tests for rules 2, 3 and 6 drive [`decide`], [`adopt_snapshot`] and
//! [`recheck_after_fresh_list`] directly, with their own [`PackageChecks`] and [`RevocationVerifier`].
use crate::lock::{LockedPackage, Lockfile, PinnedRef, Source};
use crate::manifest::ModuleManifest;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Revoked {
    /// A module id (`echo`) or package id (`model:parakeet-tdt-v2`).
    pub package: String,
    /// Exact versions; empty means every version.
    #[serde(default)]
    pub versions: Vec<String>,
    pub reason: String,
}

/// The revocation part of the signed index (proposal §4: `revoked` folded into `index.json`, with the timestamp's
/// expiry). Only reachable through [`RevocationVerifier`], so an unsigned list can't be adopted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevocationSnapshot {
    /// Monotonic index version.
    pub version: u64,
    /// ms since the epoch.
    pub issued_ms: u64,
    /// ms since the epoch; at most 8 days after `issued_ms` (the ADR 0046 clamp).
    pub expires_ms: u64,
    #[serde(default)]
    pub revoked: Vec<Revoked>,
}

impl RevocationSnapshot {
    pub fn is_fresh(&self, now_ms: u64) -> bool {
        now_ms < self.expires_ms
    }
    pub fn revokes(&self, id: &str, version: &str) -> Option<&Revoked> {
        self.revoked
            .iter()
            .find(|r| r.package == id && (r.versions.is_empty() || r.versions.iter().any(|v| v == version)))
    }
}

/// A snapshot whose signature chained to the pinned root (rule 3: "timestamp and root keys"). Constructed only by
/// [`VerifiedSnapshot::verify`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedSnapshot(RevocationSnapshot);

impl VerifiedSnapshot {
    pub fn verify(v: &dyn RevocationVerifier, bytes: &[u8], sig: &[u8]) -> Result<Self, Refusal> {
        v.verify(bytes, sig)
            .map(Self)
            .map_err(Refusal::BadRevocationSignature)
    }
    pub fn get(&self) -> &RevocationSnapshot {
        &self.0
    }
}

/// The index/timestamp signature check (the ADR 0046 Ed25519 verifier, generalised in M7).
pub trait RevocationVerifier {
    fn verify(&self, bytes: &[u8], sig: &[u8]) -> Result<RevocationSnapshot, String>;
}

/// Rule 7: the same checks on both paths. M7 implements it with the index signature, and the module's and each
/// xpc-service's `SecStaticCodeCheckValidity` against its designated requirement plus the notarization ticket.
pub trait PackageChecks {
    fn check(&self, c: &Candidate) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideloadOrigin {
    /// Settings → modules → "install from file", or opening a `.qrtzpkg` (a user action, one file).
    UserPickedFile,
    /// Anything found without the user naming that file (a watched folder, Downloads scan). Always refused.
    WatchedFolder,
}

/// What the user said yes to. Binds to the file and to the revocation state the prompt showed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
    pub package_sha256: String,
    /// `issued_ms` of the list the prompt described (`None`: the prompt said there is no list at all).
    pub revocation_issued_ms: Option<u64>,
}

/// What the confirmation prompt must show (rule 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub package_sha256: String,
    pub module: String,
    pub version: String,
    pub revocation_issued_ms: Option<u64>,
    /// `now - issued`; `None` when no list was ever stored.
    pub revocation_age_ms: Option<u64>,
    pub revocation_expired: bool,
}

pub enum Request<'a> {
    Network,
    Sideload {
        origin: SideloadOrigin,
        confirmation: Option<&'a Confirmation>,
        carried: Option<VerifiedSnapshot>,
    },
}

/// One module install, as received: the manifest bytes' hash, the file's hash, and every package it brings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub manifest: ModuleManifest,
    pub manifest_sha256: String,
    /// sha256 of the `.qrtzpkg` (sideload) or of the index entry (network).
    pub package_sha256: String,
    pub packages: Vec<(PinnedRef, LockedPackage)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    ChecksFailed(String),
    BadRevocationSignature(String),
    /// Rule 3 / gate (g) case 3: the file carries a revocation snapshot older than the stored one.
    RevocationRollback {
        carried: u64,
        stored: u64,
    },
    /// Rule 1.
    RevocationListExpired,
    /// Rule 2 (both paths).
    Revoked {
        package: String,
        version: String,
        reason: String,
    },
    /// Rule 4.
    NotUserInitiated,
    /// Rule 4: show this and ask again.
    NeedsConfirmation(Prompt),
    /// Rule 4: the yes was for another file or another revocation state.
    ConfirmationMismatch,
    /// Never install below what is installed (ADR 0046 anti-rollback).
    Downgrade {
        installed: String,
    },
    /// Rule 5 and the lockfile rules.
    Lock(crate::ModuleError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    /// Write this (with [`Lockfile::save`]) to apply the install.
    pub next_lock: Lockfile,
    /// The revocation snapshot to store from now on, if a carried one was adopted (rule 3).
    pub adopted: Option<RevocationSnapshot>,
    /// Rule 6.
    pub stale_revocation: bool,
}

/// Rule 3: the carried snapshot replaces the stored one only if it is strictly newer (higher version, not issued
/// earlier). Returns the snapshot to use; `true` if it was the carried one.
pub fn adopt_snapshot(
    stored: Option<&RevocationSnapshot>,
    carried: Option<&VerifiedSnapshot>,
) -> (Option<RevocationSnapshot>, bool) {
    match (stored, carried.map(VerifiedSnapshot::get)) {
        (None, Some(c)) => (Some(c.clone()), true),
        (Some(s), Some(c)) if c.version > s.version && c.issued_ms >= s.issued_ms => (Some(c.clone()), true),
        (s, _) => (s.cloned(), false),
    }
}

fn revoked_in(snap: Option<&RevocationSnapshot>, c: &Candidate) -> Result<(), Refusal> {
    let Some(s) = snap else { return Ok(()) };
    let m = &c.manifest;
    let pins = m.dependencies.iter().map(|d| (d.id.as_str(), d.version.as_str()));
    for (id, v) in std::iter::once((m.id.as_str(), m.version.as_str())).chain(pins) {
        if let Some(r) = s.revokes(id, v) {
            return Err(Refusal::Revoked {
                package: id.to_owned(),
                version: v.to_owned(),
                reason: r.reason.clone(),
            });
        }
    }
    Ok(())
}

/// Decide one install. Pure: nothing is written; the caller saves `next_lock` and `adopted`.
pub fn decide(
    req: Request<'_>,
    c: &Candidate,
    checks: &dyn PackageChecks,
    stored: Option<&RevocationSnapshot>,
    lock: &Lockfile,
    now_ms: u64,
) -> Result<Decision, Refusal> {
    // Rule 7 first, the same call on both paths.
    checks.check(c).map_err(Refusal::ChecksFailed)?;
    if let Some(have) = lock.modules.get(&c.manifest.id) {
        if semver::Version::parse(&have.version).is_ok_and(|h| c.manifest.semver() < h) {
            return Err(Refusal::Downgrade {
                installed: have.version.clone(),
            });
        }
    }
    let (source, snapshot, adopted) = match req {
        Request::Network => {
            // Rule 1: no override parameter exists.
            if !stored.is_some_and(|s| s.is_fresh(now_ms)) {
                return Err(Refusal::RevocationListExpired);
            }
            (Source::Network, stored.cloned(), None)
        }
        Request::Sideload {
            origin,
            confirmation,
            carried,
        } => {
            if origin != SideloadOrigin::UserPickedFile {
                return Err(Refusal::NotUserInitiated);
            }
            if let (Some(s), Some(c)) = (stored, carried.as_ref().map(VerifiedSnapshot::get)) {
                if c.version < s.version || (c.version > s.version && c.issued_ms < s.issued_ms) {
                    return Err(Refusal::RevocationRollback {
                        carried: c.version,
                        stored: s.version,
                    });
                }
            }
            let (snap, took) = adopt_snapshot(stored, carried.as_ref());
            let issued = snap.as_ref().map(|s| s.issued_ms);
            let Some(yes) = confirmation else {
                return Err(Refusal::NeedsConfirmation(Prompt {
                    package_sha256: c.package_sha256.clone(),
                    module: c.manifest.id.clone(),
                    version: c.manifest.version.clone(),
                    revocation_issued_ms: issued,
                    revocation_age_ms: issued.map(|i| now_ms.saturating_sub(i)),
                    revocation_expired: !snap.as_ref().is_some_and(|s| s.is_fresh(now_ms)),
                }));
            };
            if yes.package_sha256 != c.package_sha256 || yes.revocation_issued_ms != issued {
                return Err(Refusal::ConfirmationMismatch);
            }
            (
                Source::Sideload,
                snap.clone(),
                took.then(|| snap.expect("adopted")),
            )
        }
    };
    // Rule 2 (and the network path's own revocation check).
    revoked_in(snapshot.as_ref(), c)?;
    // Rule 5: exactly the file's contents against the lockfile; plan_install refuses extras, changed content
    // and anything missing.
    let mut next = lock
        .plan_install(&c.manifest, &c.manifest_sha256, source, &c.packages)
        .map_err(Refusal::Lock)?;
    let stale = !snapshot.as_ref().is_some_and(|s| s.is_fresh(now_ms));
    if let Some(m) = next.modules.get_mut(&c.manifest.id) {
        m.stale_revocation = stale;
    }
    Ok(Decision {
        next_lock: next,
        adopted,
        stale_revocation: stale,
    })
}

/// A module that a fresh list disabled; the shell shows a notification for each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub module: String,
    pub version: String,
    pub revoked: String,
    pub reason: String,
}

/// Rule 6: after a fresh list arrives (network index or an adopted carried snapshot that is still fresh), every
/// installed module is rechecked. A revoked module, or one whose pinned package is revoked, is disabled and
/// reported; the rest lose their stale mark. An expired list changes nothing.
pub fn recheck_after_fresh_list(
    lock: &Lockfile,
    fresh: &VerifiedSnapshot,
    now_ms: u64,
) -> (Lockfile, Vec<Notice>) {
    let s = fresh.get();
    let mut next = lock.clone();
    let mut notices = vec![];
    if !s.is_fresh(now_ms) {
        return (next, notices);
    }
    for (id, m) in next.modules.iter_mut() {
        let hit = std::iter::once((id.as_str(), m.version.as_str()))
            .chain(m.dependencies.iter().map(|r| (r.id.as_str(), r.version.as_str())))
            .find_map(|(p, v)| s.revokes(p, v).map(|r| (p.to_owned(), r.reason.clone())));
        match hit {
            Some((p, reason)) => {
                if m.disabled.is_none() {
                    notices.push(Notice {
                        module: id.clone(),
                        version: m.version.clone(),
                        revoked: p,
                        reason: reason.clone(),
                    });
                }
                m.disabled = Some(reason);
            }
            None => m.stale_revocation = false,
        }
    }
    (next, notices)
}
