//! ADR 0153/0157: the shell's side of the package manager: the HTTPS transport to the pinned package hosts (and the
//! pinned model mirror hosts), and the Apple code checks, around `qrtz_modules::install::Installer` (which has no
//! network code of its own). The app's Extensions screen, the `qrtz` CLI and the speech model download all go through
//! [`Packages`]: one installer, one fetcher, one verify path.
//!
//! - **Hosts** are pinned in this binary (`module_hosts.json`): the sole package host `https://modules.qrtz.ai`
//!   (Cloudflare R2; ADR 0167, no Backblaze B2 mirror), then, for a model file only, the signed tree's own mirror
//!   URL when it is under a pinned model mirror host (`https://huggingface.co`). Nothing else is reachable from
//!   here, and nothing is reached without a consent: `refresh` takes a `RefreshConsent`, `apply` an
//!   `InstallConsent` for exactly the plan the user saw.
//! - **Transport** is `qrtz-speech`'s downloader (ADR 0078's one network path): HTTPS only, size caps, exact byte
//!   ranges. Hosts are untrusted: every blob is checked against its signed sha256 before it enters the store.
//! - **Code checks** (Bozniak's rule 7, both paths): on macOS, `codesign --verify --strict --deep` against the
//!   package's exact designated requirement and the signing team (MR26CCZ496 only), quarantine stripped (or
//!   refused), and the staged code's cdhash equal to the notarization the signed index records (ADR 0170: all
//!   offline, no Gatekeeper lookup); a placeholder team (`0000000000`) is refused. Elsewhere every code install is
//!   refused (models, which are data, install anywhere).
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
    /// The package host is set up once the offline root exists and is pinned here (`hosting-setup.md`). Before
    /// that nothing is published to it, so the fetcher never asks it for a blob (audit P0-3: no failed requests
    /// to `modules.qrtz.ai` ahead of every Hugging Face file).
    pub fn configured(&self) -> bool {
        self.root1_sha256.is_some()
    }
    pub fn all(&self) -> Vec<String> {
        std::iter::once(self.primary.clone())
            .chain(self.mirrors.iter().cloned())
            .collect()
    }
}

/// The sole package host, plus the allowed model mirror hosts, over the shared downloader.
pub struct HttpTransport {
    hosts: Vec<Downloader>,
    /// `(allowed prefix, downloader)`: a mirror URL `<prefix>/<path>` is fetched as `<path>` from the downloader's
    /// base (the prefix itself in the app; a loopback server in tests).
    extra: Vec<(String, Downloader)>,
    /// Whether the package hosts serve anything yet ([`Hosts::configured`]); unconfigured hosts get no request.
    configured: bool,
    ctl: DownloadControl,
    /// Dev only (ADR 0169, [`dev_index`]): a published index dir on this disk (`qrtz-publish build --out`) instead
    /// of the hosts. Model files still come from their pinned Hugging Face URLs (ADR 0171), unless `url_dirs`
    /// serves that URL prefix from disk.
    dir: Option<PathBuf>,
    /// `(URL prefix, dir)`: a URL under the prefix is read from the dir (a local copy of a Hugging Face repo; tests
    /// and the dev index). Never consulted for anything but a model file's signed URL.
    url_dirs: Vec<(String, PathBuf)>,
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
            configured: true,
            ctl: DownloadControl::default(),
            dir: None,
            url_dirs: vec![],
        }
    }

    /// A published index dir as the one host (dev builds, [`dev_index`]; tests).
    pub fn local_dir(dir: impl Into<PathBuf>) -> Self {
        let mirrors: Vec<(String, String)> = Hosts::pinned()
            .model_mirrors
            .iter()
            .map(|m| (m.clone(), m.clone()))
            .collect();
        let mut t = Self::new(&[], &mirrors);
        t.dir = Some(dir.into());
        t
    }
    /// Serve model URLs under `prefix` from `dir` (`<dir>/<rest of the URL>`), e.g. a local copy of a Hugging Face
    /// repo. The bytes are checked against the signed hash like any source.
    pub fn with_url_dir(mut self, prefix: impl Into<String>, dir: impl Into<PathBuf>) -> Self {
        self.url_dirs.push((prefix.into(), dir.into()));
        self
    }
    /// Marks the package hosts as not set up: no index request and no blob request reaches them.
    pub fn unconfigured(mut self) -> Self {
        self.configured = false;
        self
    }
    pub fn hosts_configured(&self) -> bool {
        self.configured && !self.hosts.is_empty()
    }
    /// The pinned hosts and model mirrors.
    pub fn pinned() -> Self {
        let h = Hosts::pinned();
        let extra: Vec<(String, String)> = h.model_mirrors.iter().map(|m| (m.clone(), m.clone())).collect();
        let t = Self::new(&h.all(), &extra);
        if h.configured() {
            t
        } else {
            t.unconfigured()
        }
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
        if self.dir.is_some() {
            return 1;
        }
        self.hosts.len()
    }

    fn host_configured(&self, _host: usize) -> bool {
        self.configured
    }

    async fn get(&self, host: usize, path: &str, max: usize) -> Result<Option<Vec<u8>>, String> {
        if let Some(d) = &self.dir {
            return match std::fs::read(d.join(path)) {
                Ok(b) if b.len() > max => Err(format!("{path}: too large")),
                Ok(b) => Ok(Some(b)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(format!("{path}: {e}")),
            };
        }
        if !self.configured {
            return Err(format!("{path}: the package hosts aren't set up yet"));
        }
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
        let local = match src {
            Source::Host(_) => self.dir.as_ref().map(|d| d.join(layout::blob(sha256))),
            Source::Url(u) => self
                .url_dirs
                .iter()
                .filter(|(p, _)| u.starts_with(p.as_str()))
                .max_by_key(|(p, _)| p.len())
                .map(|(p, d)| d.join(&u[p.len()..])),
        };
        if let Some(path) = local {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
            f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; usize::try_from(len).map_err(|_| "range too large")?];
            f.read_exact(&mut buf).map_err(|e| e.to_string())?;
            return Ok(buf);
        }
        let (dl, path) = match src {
            Source::Host(_) if !self.configured => return Err("the package hosts aren't set up yet".into()),
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

/// Rule 7 on macOS, all offline (ADR 0170, Bozniak's model): the quarantine flag is stripped from the staged code
/// and refused if it stays, then the signature, the exact designated requirement (which names the identifier and
/// the team) and the team; [`CodeChecks::check_notarized`] then requires the staged code's cdhash to be the one the
/// signed index records as notarized. Nothing asks Gatekeeper online (a bare helper can't be stapled, and a
/// stapled-ticket `spctl` assessment of one fails anyway, ADR 0162).
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

pub const QUARANTINE: &str = "com.apple.quarantine";

/// Strip `com.apple.quarantine` from `path` (recursively), then refuse if any file still carries it.
#[cfg(target_os = "macos")]
pub fn strip_quarantine(path: &Path) -> Result<(), String> {
    let p = path.to_str().ok_or("path is not utf-8")?;
    // Fails when no file has it, which is the normal case (our fetcher never sets it).
    let _ = run("/usr/bin/xattr", &["-r", "-d", QUARANTINE, p]);
    let left = run("/usr/bin/xattr", &["-r", "-l", p]).unwrap_or_default();
    if left.contains(QUARANTINE) {
        return Err("quarantined code (com.apple.quarantine couldn't be removed)".into());
    }
    Ok(())
}

/// The checks on one staged helper or bundle (signature, requirement, team), after stripping quarantine.
#[cfg(target_os = "macos")]
pub fn check_bundle(bundle: &Path, team_id: &str, requirement: &str) -> Result<(), String> {
    if team_id == qrtz_modules::manifest::PLACEHOLDER_TEAM_ID {
        return Err("placeholder team id: not a Developer ID build".into());
    }
    if team_id != qrtz_modules::manifest::RELEASE_TEAM_ID {
        return Err(format!(
            "signed for team {team_id}, not qrtz's ({})",
            qrtz_modules::manifest::RELEASE_TEAM_ID
        ));
    }
    strip_quarantine(bundle)?;
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
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn check_bundle(_bundle: &Path, _team_id: &str, _requirement: &str) -> Result<(), String> {
    Err("module code checks need macOS".into())
}

/// The code directory hash `codesign` reports for `file` (`CDHash=<40 hex>`).
#[cfg(target_os = "macos")]
pub fn cdhash(file: &Path) -> Result<String, String> {
    let p = file.to_str().ok_or("path is not utf-8")?;
    let info = run("/usr/bin/codesign", &["-d", "-vvv", p])?;
    info.lines()
        .find_map(|l| l.trim().strip_prefix("CDHash="))
        .map(|h| h.trim().to_ascii_lowercase())
        .ok_or_else(|| "codesign reports no CDHash".into())
}

/// Dev only (ADR 0169, [`dev_index`]): a dev-signed module (Apple Development or ad hoc, `bun run modules:dev`)
/// must carry a valid signature with exactly the identifier its requirement names (`ai.qrtz.<name>`). Team and
/// notarization aren't checked; a release build never uses this.
#[cfg(debug_assertions)]
pub struct DevCodeChecks;

/// The identifier a designated requirement names (`identifier "ai.qrtz.echo" and ...` → `ai.qrtz.echo`).
pub fn requirement_identifier(requirement: &str) -> Option<&str> {
    let rest = requirement.trim_start().strip_prefix("identifier")?.trim_start();
    let rest = rest.strip_prefix('"')?;
    rest.split('"').next()
}

#[cfg(debug_assertions)]
impl CodeChecks for DevCodeChecks {
    #[cfg(target_os = "macos")]
    fn check(
        &self,
        _kind: PackageKind,
        code: &Path,
        _team_id: &str,
        requirement: &str,
    ) -> Result<(), String> {
        let id = requirement_identifier(requirement).ok_or("requirement names no identifier")?;
        strip_quarantine(code)?;
        let p = code.to_str().ok_or("path is not utf-8")?;
        run("/usr/bin/codesign", &["--verify", "--strict", p])?;
        let info = run("/usr/bin/codesign", &["-d", "--verbose=2", p])?;
        if !info.lines().any(|l| l.trim() == format!("Identifier={id}")) {
            return Err(format!("not signed as {id}"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    fn check(
        &self,
        _kind: PackageKind,
        code: &Path,
        _team_id: &str,
        requirement: &str,
    ) -> Result<(), String> {
        requirement_identifier(requirement).ok_or("requirement names no identifier")?;
        code.exists()
            .then_some(())
            .ok_or_else(|| format!("{}: missing", code.display()))
    }
}

/// `QRTZ_DEV_INDEX_DIR` (a `qrtz-publish build --out` dir, `bun run modules:dev`) and `QRTZ_DEV_ROOT1_SHA256` (its
/// root 1's sha256): a debug build installs modules from that local signed index, so the marketplace's install,
/// models and remove flow is testable before modules.qrtz.ai is live. A release build ignores both, always.
#[cfg(debug_assertions)]
pub fn dev_index() -> Option<(PathBuf, String)> {
    dev_index_from(cfg!(debug_assertions), |k| std::env::var(k).ok())
}

#[cfg(debug_assertions)]
pub fn dev_index_from(debug_build: bool, get: impl Fn(&str) -> Option<String>) -> Option<(PathBuf, String)> {
    if !debug_build {
        return None;
    }
    let dir = get("QRTZ_DEV_INDEX_DIR").filter(|d| !d.is_empty())?;
    let pin = get("QRTZ_DEV_ROOT1_SHA256")?.trim().to_ascii_lowercase();
    if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
        tracing::warn!("QRTZ_DEV_ROOT1_SHA256 is not a sha256: dev index ignored");
        return None;
    }
    Some((PathBuf::from(dir), pin))
}

impl CodeChecks for AppleCodeChecks {
    /// `code` is an extension's helper executable or a service's `.xpc` bundle, already staged.
    fn check(&self, _kind: PackageKind, code: &Path, team_id: &str, requirement: &str) -> Result<(), String> {
        check_bundle(code, team_id, requirement)
    }

    #[cfg(target_os = "macos")]
    fn check_notarized(&self, file: &Path, n: &qrtz_modules::index::Notarization) -> Result<(), String> {
        let got = cdhash(file)?;
        if got != n.cdhash {
            return Err(format!(
                "cdhash {got} isn't the notarized {} (submission {})",
                n.cdhash, n.submission_id
            ));
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn check_notarized(&self, _file: &Path, _n: &qrtz_modules::index::Notarization) -> Result<(), String> {
        Err("module code checks need macOS".into())
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
        // Compiled into debug builds only (design call (d)): a release qrtz has no dev index and no dev checks.
        #[cfg(debug_assertions)]
        if let Some((dir, pin)) = dev_index() {
            tracing::warn!(dir = %dir.display(), "dev build: modules install from a local dev index");
            return Some(Self {
                installer: Installer::new(root, Some(pin), Box::new(DevCodeChecks)),
                transport: HttpTransport::local_dir(dir),
            });
        }
        Some(Self {
            installer: Installer::new(root, Hosts::pinned().root1_sha256, Box::new(AppleCodeChecks)),
            transport: HttpTransport::pinned(),
        })
    }

    pub fn with(installer: Installer, transport: HttpTransport) -> Self {
        Self { installer, transport }
    }

    /// Whether the installer has a pinned root to verify an index against (none = nothing can be refreshed).
    pub fn installer_has_root(&self) -> bool {
        self.installer.pinned_root().is_some()
    }

    pub async fn refresh(&self, consent: &RefreshConsent, now_ms: u64) -> Result<Refreshed, InstallError> {
        self.installer.refresh(&self.transport, consent, now_ms).await
    }

    /// ADR 0174: a bundled extension (echo) is never a package, from the Extensions screen or the `qrtz` command.
    pub fn plan(&self, op: Op) -> Result<Plan, InstallError> {
        let named: Vec<&String> = match &op {
            Op::Install(reqs) => reqs.iter().map(|(n, _)| n).collect(),
            Op::Remove(names) | Op::Update { names, .. } => names.iter().collect(),
            Op::Pinned(pkgs) => pkgs.iter().map(|p| &p.name).collect(),
            Op::Engine { .. } | Op::Frozen => vec![],
        };
        if let Some(n) = named.into_iter().find(|n| qrtz_modules::is_bundled_extension(n)) {
            return Err(InstallError::NotLocked(format!(
                "{n} is built into qrtz.app (never installed, updated or removed as a package)"
            )));
        }
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

    /// Every locked package with who uses it ([`crate::local_models::pinned_users`] names the modules a pinned
    /// model is for), the one answer the module manager and `qrtz ls` show.
    pub fn list(&self) -> Result<Vec<Listed>, InstallError> {
        self.installer.list_with(&crate::local_models::pinned_users)
    }

    /// What `prune` would delete now (nothing is deleted): "clean up" is offered only when this isn't empty.
    pub fn reclaimable(&self) -> Result<Pruned, InstallError> {
        self.installer.reclaimable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_hosts_are_https_and_name_no_user() {
        let h = Hosts::pinned();
        assert_eq!(h.primary, "https://modules.qrtz.ai");
        assert!(h.mirrors.is_empty(), "ADR 0167: no package-host mirror");
        assert_eq!(h.all(), ["https://modules.qrtz.ai"]);
        assert!(h.all().iter().all(|u| !u.contains('?') && !u.ends_with('/')));
        assert_eq!(h.model_mirrors, ["https://huggingface.co"]);
    }

    /// P0-3: with no pinned root (today), the pinned transport reaches only the model mirror: a pinned model file
    /// never waits on modules.qrtz.ai first.
    #[test]
    fn the_pinned_hosts_are_skipped_until_the_root_is_pinned() {
        let h = Hosts::pinned();
        let t = HttpTransport::pinned();
        assert_eq!(t.hosts_configured(), h.configured());
        for i in 0..t.hosts() {
            assert_eq!(t.host_configured(i), h.configured());
        }
        // ADR 0171: a model file comes only from its Hugging Face URL, configured hosts or not.
        let b = qrtz_modules::index::BlobRef {
            sha256: "0".repeat(64),
            size: 1,
            mirrors: vec![format!("https://huggingface.co/o/r/resolve/{}/f", "a".repeat(40))],
        };
        let src = qrtz_modules::fetch::sources(&t, &b);
        assert_eq!(src, vec![Source::Url(b.mirrors[0].clone())]);
    }

    /// Design call (d): the dev index and the dev code checks aren't in a release build at all, not just unused.
    #[test]
    fn the_dev_index_and_dev_checks_exist_only_in_debug_builds() {
        let src = include_str!("module_hosting.rs");
        let src = &src[..src.find("#[cfg(test)]\nmod tests").expect("tests")];
        for item in [
            "pub struct DevCodeChecks;",
            "impl CodeChecks for DevCodeChecks {",
            "pub fn dev_index() -> Option<(PathBuf, String)> {",
            "pub fn dev_index_from(",
            "        if let Some((dir, pin)) = dev_index() {",
        ] {
            let at = src.find(item).unwrap_or_else(|| panic!("{item}"));
            let before: Vec<&str> = src[..at]
                .lines()
                .rev()
                .map(str::trim)
                .filter(|l| !l.starts_with("//"))
                .take(1)
                .collect();
            assert_eq!(before, ["#[cfg(debug_assertions)]"], "{item}");
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_release_build_never_uses_a_dev_index() {
        let pin = "a".repeat(64);
        let get = |k: &str| match k {
            "QRTZ_DEV_INDEX_DIR" => Some("/tmp/dev-index".to_owned()),
            "QRTZ_DEV_ROOT1_SHA256" => Some(pin.clone()),
            "QRTZ_ALLOW_DEV_KNOBS" => Some("1".to_owned()),
            _ => None,
        };
        assert_eq!(dev_index_from(false, get), None);
        assert_eq!(
            dev_index_from(true, get),
            Some((PathBuf::from("/tmp/dev-index"), pin.clone()))
        );
        assert_eq!(
            dev_index_from(true, |k| (k == "QRTZ_DEV_INDEX_DIR").then(|| "/x".to_owned())),
            None,
            "no pin, no dev index"
        );
    }

    #[test]
    fn the_dev_checks_read_the_identifier_from_the_requirement() {
        let dr = "identifier \"ai.qrtz.echo\" and anchor apple generic";
        assert_eq!(requirement_identifier(dr), Some("ai.qrtz.echo"));
        assert_eq!(requirement_identifier("anchor apple"), None);
    }

    #[test]
    fn an_unconfigured_transport_sends_nothing_to_its_hosts() {
        // A closed loopback port: a request would fail as a connection error, not this message.
        let t = HttpTransport::new(&["http://127.0.0.1:9".into()], &[]).unconfigured();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let e = rt.block_on(t.get(0, "v1/root/1.root.json", 1024)).unwrap_err();
        assert!(e.contains("aren't set up"), "{e}");
        let e = rt
            .block_on(t.get_range(&Source::Host(0), &"0".repeat(64), 0, 1))
            .unwrap_err();
        assert!(e.contains("aren't set up"), "{e}");
    }
}
