//! ADR 0153/0157: the shell's side of the package manager: the HTTPS transport to the pinned package hosts (and the
//! pinned model mirror hosts), and the Apple code checks, around `qrtz_modules::install::Installer` (which has no
//! network code of its own). The app's Extensions screen, the `qrtz` CLI and the speech model download all go through
//! [`Packages`]: one installer, one fetcher, one verify path.
//!
//! - **Hosts** are pinned in this binary (`module_hosts.json`): `https://modules.qrtz.ai` (Cloudflare R2), then
//!   each mirror (Backblaze B2), then, for a model file only, the signed tree's own mirror URL when it is under a
//!   pinned model mirror host (`https://huggingface.co`). Nothing else is reachable from here, and nothing is
//!   reached without a consent: `refresh` takes a `RefreshConsent`, `apply` an `InstallConsent` for exactly the plan
//!   the user saw.
//! - **Transport** is `qrtz-speech`'s downloader (ADR 0078's one network path): HTTPS only, size caps, exact byte
//!   ranges. Hosts are untrusted: every blob is checked against its signed sha256 before it enters the store.
//! - **Code checks** (Bozniak's rule 7, both paths): on macOS, `codesign --verify --strict --deep` against the
//!   package's exact designated requirement, the signing team, and Gatekeeper's notarization assessment of the
//!   staged helper or `.xpc`; a placeholder team (`0000000000`) is refused. Elsewhere every code install is refused
//!   (models, which are data, install anywhere).
use qrtz_modules::fetch::{Source, Transport};
use qrtz_modules::index::{layout, PackageKind};
use qrtz_modules::install::{CodeChecks, Installer};
use qrtz_speech::download::{DownloadControl, Downloader};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub use qrtz_modules::install::{
    Applied, Change, InstallConsent, InstallError, Listed, Op, Plan, Pruned, RefreshConsent, Refreshed,
};

pub const HOSTS: &str = include_str!("module_hosts.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hosts {
    pub primary: String,
    pub mirrors: Vec<String>,
    /// Hosts a signed tree's extra mirror URL may point at (models only).
    pub model_mirrors: Vec<String>,
    pub root1_sha256: Option<String>,
}

impl Hosts {
    pub fn pinned() -> Self {
        serde_json::from_str(HOSTS).expect("module_hosts.json")
    }
    pub fn all(&self) -> Vec<String> {
        std::iter::once(self.primary.clone())
            .chain(self.mirrors.iter().cloned())
            .collect()
    }
}

/// The package hosts in order, plus the allowed model mirror hosts, over the shared downloader.
pub struct HttpTransport {
    hosts: Vec<Downloader>,
    /// `(allowed prefix, downloader)`: a mirror URL `<prefix>/<path>` is fetched as `<path>` from the downloader's
    /// base (the prefix itself in the app; a loopback server in tests).
    extra: Vec<(String, Downloader)>,
    ctl: DownloadControl,
}

impl HttpTransport {
    /// `bases` are host roots (`https://modules.qrtz.ai`); the layout paths carry the `v1/`. `extra` maps each
    /// allowed mirror prefix to the base actually reached. Tests pass loopback servers; the app passes
    /// [`Hosts::pinned`].
    pub fn new(bases: &[String], extra: &[(String, String)]) -> Self {
        Self {
            hosts: bases.iter().map(|b| Downloader::new(b)).collect(),
            extra: extra
                .iter()
                .map(|(p, b)| (p.trim_end_matches('/').to_owned(), Downloader::new(b)))
                .collect(),
            ctl: DownloadControl::default(),
        }
    }
    /// The pinned hosts and model mirrors.
    pub fn pinned() -> Self {
        let h = Hosts::pinned();
        let extra: Vec<(String, String)> = h.model_mirrors.iter().map(|m| (m.clone(), m.clone())).collect();
        Self::new(&h.all(), &extra)
    }
    /// Pause stops at the next range boundary, keeping the `.part` and its finished ranges; the next `apply`
    /// resumes them.
    pub fn control(&self) -> DownloadControl {
        self.ctl.clone()
    }
    pub fn with_control(mut self, ctl: DownloadControl) -> Self {
        self.ctl = ctl;
        self
    }
}

#[async_trait::async_trait]
impl Transport for HttpTransport {
    fn hosts(&self) -> usize {
        self.hosts.len()
    }

    async fn get(&self, host: usize, path: &str, max: usize) -> Result<Option<Vec<u8>>, String> {
        let b = self.hosts[host]
            .fetch_meta(path)
            .await
            .map_err(|e| e.to_string())?;
        match b {
            Some(b) if b.len() > max => Err(format!("{path}: too large")),
            b => Ok(b),
        }
    }

    async fn get_range(&self, src: &Source, sha256: &str, start: u64, len: u64) -> Result<Vec<u8>, String> {
        if self.ctl.is_paused() {
            return Err("paused".into());
        }
        let (dl, path) = match src {
            Source::Host(i) => (self.hosts.get(*i).ok_or("no such host")?, layout::blob(sha256)),
            Source::Url(u) => self
                .extra
                .iter()
                .find_map(|(p, d)| {
                    let rest = u.strip_prefix(p.as_str())?.strip_prefix('/')?;
                    Some((d, rest.to_owned()))
                })
                .ok_or_else(|| format!("{u}: not under a pinned model mirror host"))?,
        };
        dl.fetch_range(&path, start, len, &self.ctl)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Rule 7 on macOS: the signature, the exact designated requirement (which names the bundle id and the team), the
/// team, and the notarization assessment. Runs `/usr/bin/codesign` and `/usr/sbin/spctl` on the staged bundle.
pub struct AppleCodeChecks;

#[cfg(target_os = "macos")]
fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let o = std::process::Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    if o.status.success() {
        Ok(text)
    } else {
        Err(format!("{cmd}: {}", text.trim()))
    }
}

/// The checks on one bundle (`.app` or `.xpc`).
#[cfg(target_os = "macos")]
pub fn check_bundle(bundle: &Path, team_id: &str, requirement: &str) -> Result<(), String> {
    if team_id == qrtz_modules::manifest::PLACEHOLDER_TEAM_ID {
        return Err("placeholder team id: not a Developer ID build".into());
    }
    let p = bundle.to_str().ok_or("bundle path is not utf-8")?;
    let req = format!("-R={requirement}");
    run("/usr/bin/codesign", &["--verify", "--strict", "--deep", &req, p])?;
    let info = run("/usr/bin/codesign", &["-d", "--verbose=2", p])?;
    if !info
        .lines()
        .any(|l| l.trim() == format!("TeamIdentifier={team_id}"))
    {
        return Err(format!("signed by another team (want {team_id})"));
    }
    // Gatekeeper: Developer ID + a notarization ticket (stapled by the publisher, so this works offline).
    let g = run(
        "/usr/sbin/spctl",
        &["--assess", "--type", "execute", "--verbose=2", p],
    )?;
    if !g.contains("Notarized Developer ID") {
        return Err("not notarized".into());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn check_bundle(_bundle: &Path, _team_id: &str, _requirement: &str) -> Result<(), String> {
    Err("module code checks need macOS".into())
}

impl CodeChecks for AppleCodeChecks {
    /// `code` is an extension's helper executable or a service's `.xpc` bundle, already staged.
    fn check(&self, _kind: PackageKind, code: &Path, team_id: &str, requirement: &str) -> Result<(), String> {
        check_bundle(code, team_id, requirement)
    }
}

/// The installer over the pinned hosts: what the Extensions screen, the `qrtz` CLI and the speech model download
/// share.
pub struct Packages {
    pub installer: Installer,
    pub transport: HttpTransport,
}

impl Packages {
    /// The app's data dir (or `QRTZ_USER_DATA`), the pinned hosts and root, and the Apple checks.
    pub fn pinned(root: Option<PathBuf>) -> Option<Self> {
        let root = root.or_else(qrtz_modules::install::default_root)?;
        Some(Self {
            installer: Installer::new(root, Hosts::pinned().root1_sha256, Box::new(AppleCodeChecks)),
            transport: HttpTransport::pinned(),
        })
    }

    pub fn with(installer: Installer, transport: HttpTransport) -> Self {
        Self { installer, transport }
    }

    pub async fn refresh(&self, consent: &RefreshConsent, now_ms: u64) -> Result<Refreshed, InstallError> {
        self.installer.refresh(&self.transport, consent, now_ms).await
    }

    pub fn plan(&self, op: Op) -> Result<Plan, InstallError> {
        self.installer.plan(op)
    }

    pub async fn apply(
        &self,
        plan: &Plan,
        consent: &InstallConsent,
        now_ms: u64,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<Applied, InstallError> {
        self.installer
            .apply(&self.transport, plan, consent, now_ms, progress)
            .await
    }

    pub fn prune(&self) -> Result<Pruned, InstallError> {
        self.installer.prune()
    }

    pub fn list(&self) -> Result<Vec<Listed>, InstallError> {
        self.installer.list()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_hosts_are_https_and_name_no_user() {
        let h = Hosts::pinned();
        assert_eq!(h.primary, "https://modules.qrtz.ai");
        for m in &h.mirrors {
            assert!(
                m.starts_with("https://") && m.ends_with(".backblazeb2.com"),
                "{m}"
            );
        }
        assert!(h.all().iter().all(|u| !u.contains('?') && !u.ends_with('/')));
        assert_eq!(h.model_mirrors, ["https://huggingface.co"]);
    }
}
