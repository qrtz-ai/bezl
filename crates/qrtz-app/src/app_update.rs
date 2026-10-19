//! Audit P1 (§5 item 19, first step): "check for updates", started only by the user, through the signed index.
//! The qrtz shell is the index's `qrtz` package (`PackageKind::Shell`, ADR 0157). A check refreshes the index the
//! same way `packages:refresh` does (the user's press is the consent; every document is verified against the pinned
//! root), then compares the newest unrevoked `qrtz` version with this build. It installs nothing: the answer is a
//! version to show. Before the package hosts are set up (no pinned root) it answers "not set up" without a request.
use crate::module_hosting::{InstallError, Packages, RefreshConsent};
use serde_json::{json, Value};

/// The index name of the qrtz shell.
pub const SHELL_PACKAGE: &str = "qrtz";

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// No pinned root yet (`module_hosts.json` `root1Sha256` is null): nothing was asked of any host.
    NotSetUp,
    /// The signed index lists no `qrtz` release.
    NoneListed,
    UpToDate {
        latest: String,
    },
    Available {
        latest: String,
        size_bytes: u64,
    },
}

/// `latest` is newer than `current` (both semver; an unparsable one is never "newer").
pub fn is_newer(latest: &str, current: &str) -> bool {
    qrtz_modules::install::is_newer(latest, current)
}

/// Refresh the signed index (user-started) and compare its newest `qrtz` with `current`.
pub async fn check(packages: &Packages, current: &str, now_ms: u64) -> Result<UpdateCheck, InstallError> {
    if !packages.installer_has_root() || !packages.transport.hosts_configured() {
        return Ok(UpdateCheck::NotSetUp);
    }
    packages.refresh(&RefreshConsent::user_asked(), now_ms).await?;
    let (_, v) = packages.installer.cached_index()?;
    Ok(match v.index.latest(SHELL_PACKAGE) {
        None => UpdateCheck::NoneListed,
        Some((latest, e)) if is_newer(latest, current) => UpdateCheck::Available {
            latest: latest.to_owned(),
            size_bytes: e.size,
        },
        Some((latest, _)) => UpdateCheck::UpToDate {
            latest: latest.to_owned(),
        },
    })
}

/// The `packages:check-update` answer.
pub fn to_json(current: &str, c: &UpdateCheck) -> Value {
    let (state, latest, size) = match c {
        UpdateCheck::NotSetUp => ("not-set-up", None, None),
        UpdateCheck::NoneListed => ("none-listed", None, None),
        UpdateCheck::UpToDate { latest } => ("up-to-date", Some(latest.clone()), None),
        UpdateCheck::Available { latest, size_bytes } => {
            ("available", Some(latest.clone()), Some(*size_bytes))
        }
    };
    json!({"current": current, "state": state, "latest": latest, "sizeBytes": size})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_is_semver_order() {
        assert!(is_newer("0.4.0", "0.3.9"));
        assert!(is_newer("1.0.0", "1.0.0-beta.2"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.9", "0.3.0"));
        assert!(!is_newer("not a version", "0.3.0"));
    }

    #[test]
    fn the_answer_is_shaped_for_the_contract() {
        let v = to_json(
            "0.3.0",
            &UpdateCheck::Available {
                latest: "0.4.0".into(),
                size_bytes: 9,
            },
        );
        assert_eq!(
            v,
            json!({"current": "0.3.0", "state": "available", "latest": "0.4.0", "sizeBytes": 9})
        );
        assert_eq!(to_json("0.3.0", &UpdateCheck::NotSetUp)["state"], "not-set-up");
    }

    /// Today (no pinned root) a check reaches no host at all.
    #[test]
    fn the_pinned_check_asks_nothing_before_the_hosts_exist() {
        if crate::module_hosting::Hosts::pinned().configured() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let p = Packages::pinned(Some(tmp.path().to_path_buf())).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(rt.block_on(check(&p, "0.3.0", 0)).unwrap(), UpdateCheck::NotSetUp);
    }
}
