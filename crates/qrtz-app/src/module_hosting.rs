//! ADR 0153: the shell's side of installable modules: the HTTPS transport to the pinned module hosts and the
//! Apple code checks, around `qrtz_modules::install::Installer` (which has no network code of its own).
//!
//! - **Hosts** are pinned in this binary (`module_hosts.json`): `https://modules.qrtz.ai` (Cloudflare R2), then
//!   each mirror (Backblaze B2). Nothing else is reachable from here, and nothing is reached without a consent:
//!   `refresh` takes a `RefreshConsent`, `install` an `InstallConsent` for exactly the plan the user saw.
//! - **Transport** is `qrtz-speech`'s downloader (ADR 0078's one network path, ADR 0079's callers): HTTPS only,
//!   resumable `.part` files, size caps, sha256 before rename. Hosts are untrusted: the installer checks every
//!   byte against the signed index, and moves to the next host when one fails or serves something stale.
//! - **Code checks** (Bozniak's rule 7, both paths): on macOS, `codesign --verify --strict --deep` against the
//!   manifest's exact designated requirement, the signing team, and Gatekeeper's notarization assessment of the
//!   staged bundle; a placeholder team (`0000000000`) is refused. Elsewhere every install is refused.
use qrtz_modules::index::Chunk;
use qrtz_modules::install::{CodeChecks, Installer, Transport};
use qrtz_modules::manifest::ModuleManifest;
use qrtz_speech::bundle::FileEntry;
use qrtz_speech::download::{DownloadControl, Downloader, Progress};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub use qrtz_modules::install::{InstallConsent, InstallError, Installed, Plan, RefreshConsent, Refreshed};

pub const HOSTS: &str = include_str!("module_hosts.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hosts {
    pub primary: String,
    pub mirrors: Vec<String>,
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

/// The primary host and its mirrors, in order, over the shared downloader.
pub struct HttpTransport {
    hosts: Vec<Downloader>,
    ctl: DownloadControl,
}

impl HttpTransport {
    /// `bases` are host roots (`https://modules.qrtz.ai`); the layout paths carry the `v1/`. Tests pass loopback
    /// servers; the app passes [`Hosts::pinned`].
    pub fn new(bases: &[String]) -> Self {
        Self {
            hosts: bases.iter().map(|b| Downloader::new(b)).collect(),
            ctl: DownloadControl::default(),
        }
    }
    /// Pause keeps the `.part` files; the next `install` resumes them.
    pub fn control(&self) -> DownloadControl {
        self.ctl.clone()
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

    async fn get_pinned(&self, host: usize, path: &str, size: u64, sha256: &str) -> Result<Vec<u8>, String> {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("qrtz-index-{}-{n}", std::process::id()));
        let entry = FileEntry {
            path: "index.json".into(),
            size,
            sha256: sha256.into(),
        };
        let r = self.hosts[host]
            .download_files(&[(path.to_owned(), entry)], &dir, &self.ctl, &|_| {})
            .await
            .map_err(|e| e.to_string())
            .and_then(|()| std::fs::read(dir.join("index.json")).map_err(|e| e.to_string()));
        let _ = std::fs::remove_dir_all(&dir);
        r
    }

    async fn get_blobs(
        &self,
        host: usize,
        blobs: &[Chunk],
        dir: &Path,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), String> {
        let files: Vec<(String, FileEntry)> = blobs
            .iter()
            .map(|c| {
                (
                    qrtz_modules::index::layout::blob(&c.sha256),
                    FileEntry {
                        path: c.sha256.clone(),
                        size: c.size,
                        sha256: c.sha256.clone(),
                    },
                )
            })
            .collect();
        self.hosts[host]
            .download_files(&files, dir, &self.ctl, &|p: Progress| progress(p.done, p.total))
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

fn bundle_in(dir: &Path, entry: Option<&str>, ext: &[&str]) -> Result<PathBuf, String> {
    if let Some(e) = entry {
        return Ok(dir.join(e));
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| ext.contains(&x))
        })
        .collect();
    found.sort();
    match found.len() {
        1 => Ok(found.remove(0)),
        n => Err(format!("{}: expected one bundle, found {n}", dir.display())),
    }
}

impl CodeChecks for AppleCodeChecks {
    fn check_module(&self, dir: &Path, m: &ModuleManifest) -> Result<(), String> {
        let b = bundle_in(dir, Some(&m.launch.entry), &[])?;
        check_bundle(&b, &m.code.team_id, &m.code.designated_requirement)
    }
    fn check_service(
        &self,
        dir: &Path,
        bundle_id: &str,
        team_id: &str,
        requirement: &str,
    ) -> Result<(), String> {
        let named = dir.join(format!("{bundle_id}.xpc"));
        let b = if named.exists() {
            named
        } else {
            bundle_in(dir, None, &["xpc", "app"])?
        };
        check_bundle(&b, team_id, requirement)
    }
}

/// The installer over the pinned hosts.
pub struct ModuleHosting {
    pub installer: Installer,
    pub transport: HttpTransport,
}

impl ModuleHosting {
    /// `~/Library/Application Support/qrtz/modules`, the pinned hosts and root, and the Apple checks.
    pub fn pinned(root: Option<PathBuf>) -> Option<Self> {
        let h = Hosts::pinned();
        let root = root.or_else(qrtz_modules::install::default_root)?;
        Some(Self::with(
            root,
            &h.all(),
            h.root1_sha256,
            Box::new(AppleCodeChecks),
        ))
    }

    pub fn with(
        root: PathBuf,
        hosts: &[String],
        root1_sha256: Option<String>,
        checks: Box<dyn CodeChecks>,
    ) -> Self {
        Self {
            installer: Installer::new(root, root1_sha256, checks),
            transport: HttpTransport::new(hosts),
        }
    }

    pub async fn refresh(&self, consent: &RefreshConsent, now_ms: u64) -> Result<Refreshed, InstallError> {
        self.installer.refresh(&self.transport, consent, now_ms).await
    }

    pub fn plan(&self, module: &str, version: Option<&str>) -> Result<Plan, InstallError> {
        self.installer.plan(module, version)
    }

    pub async fn install(
        &self,
        consent: &InstallConsent,
        now_ms: u64,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<Installed, InstallError> {
        self.installer
            .install(&self.transport, consent, now_ms, progress)
            .await
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
    }
}
