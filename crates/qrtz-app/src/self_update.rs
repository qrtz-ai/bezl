//! ADR 0181: qrtz updates itself from its GitHub releases (QRT-313; it was the package host before).
//!
//! The feed is `https://github.com/qrtz-ai/qrtz/releases/latest/download/latest.json` ([`FEED_URL`]), a signed envelope `{"feed": <base64>, "signature":
//! <base64 minisign>}` ([`open_feed`]). The signature (the **app-update key**) covers the feed's exact bytes, and the
//! feed (Tauri's static updater JSON plus `sha256`, `size` and `expires`) names one `darwin-aarch64` release whose
//! archive is the release asset `<RELEASES>/download/app-v<version>/qrtz_<version>_aarch64.app.tar.gz` of a notarized,
//! stapled qrtz.app. GitHub redirects both to its asset storage; `update_redirect.rs` allows only HTTPS hops that end on
//! exactly `objects.githubusercontent.com` or `release-assets.githubusercontent.com` (else `update_redirect_blocked`).
//! Hosts are untrusted either way: everything below is checked on the bytes, wherever they came from.
//! A release is taken only if, in this order:
//!
//! 0. the envelope's signature verifies and the feed hasn't expired (`expires`, at most [`MAX_FEED_LIFETIME`] ahead;
//!    `app-feed-refresh.yml` re-signs it weekly), so a host or network replaying an old feed can hide an update for
//!    weeks at most, never forever (a freeze attack, [`check_fresh`]);
//! 1. the feed parses and names exactly that archive URL under [`ARCHIVE_BASE`] ([`parse_feed`]);
//! 2. its version is strictly newer than this build (plain `x.y.z`, never a downgrade or a re-install, [`is_update`]);
//! 3. the archive has the feed's size and sha256, and its minisign signature verifies against the **app-update key**
//!    compiled into this build (`app_update_key.pub`; not the module root, index or timestamp key), with the signed
//!    version (the signature's trusted comment) equal to the feed's ([`verify_signature`]);
//! 4. unpacked, it is `qrtz.app` with identifier `ai.qrtz.desktop`, that version, a valid Developer ID signature of
//!    team MR26CCZ496 and a notarization ticket (`codesign -R` with `notarized`, and Gatekeeper's verdict
//!    "Notarized Developer ID"); only then is the high-water mark raised and the bundle swapped into place
//!    ([`install_bundle`]): a download or check that fails leaves the version free to retry.
//!
//! The archive is streamed to a file (never held in memory) and hashed and signature-checked from there.
//!
//! Nothing runs from the new bundle until the user presses "restart to update" (ADR 0172's pattern): the swap
//! leaves the running process on its mapped code, and the relaunch opens the new one. The quiet check runs once a
//! minute after launch and every 6 hours; a download happens only when the user presses "update".
use crate::service::Events;
use async_trait::async_trait;
use qrtz_ipc::{AppError, ErrorCode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// QRT-313: qrtz's GitHub releases; the feed and every archive are assets of one of them.
pub const RELEASES: &str = "https://github.com/qrtz-ai/qrtz/releases";
/// The newest release's signed feed (GitHub redirects `latest` to the tagged release's asset).
pub const FEED_URL: &str = "https://github.com/qrtz-ai/qrtz/releases/latest/download/latest.json";
/// What every archive URL starts with: `<ARCHIVE_BASE>/` + [`archive_path`] (`scripts/app/feed.mjs` `HOST`).
pub const ARCHIVE_BASE: &str = "https://github.com/qrtz-ai/qrtz/releases/download";
/// The one platform qrtz ships (Apple Silicon).
pub const TARGET: &str = "darwin-aarch64";
/// The app's bundle identifier (tauri.conf.json `identifier`), the only one a new bundle may carry.
pub const APP_IDENTIFIER: &str = "ai.qrtz.desktop";
pub const MAX_FEED_BYTES: usize = 64 * 1024;
pub const MAX_ARCHIVE_BYTES: u64 = 1 << 30;
/// The furthest ahead a feed's `expires` may be: one that claims longer is refused (a mis-signed or forged lifetime).
pub const MAX_FEED_LIFETIME: Duration = Duration::from_secs(60 * 24 * 3600);
const MAX_NOTES: usize = 4000;
pub const FIRST_CHECK_AFTER: Duration = Duration::from_secs(60);
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

/// The committed key file (comments start with `#`); see [`pinned_key`].
pub const KEY_FILE: &str = include_str!("app_update_key.pub");

/// The app-update public key compiled into this build: the first non-comment line of `app_update_key.pub`, base64
/// of a minisign public key. `None` until `scripts/ci/setup-secrets.sh app-update` has written it (then this build
/// answers "updates aren't set up yet" and asks no host).
pub fn pinned_key() -> Option<&'static str> {
    key_from(KEY_FILE)
}

pub fn key_from(file: &str) -> Option<&str> {
    file.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
}

/// A key line that is a minisign public key (base64 of the two-line `.pub` text).
pub fn key_is_valid(key_b64: &str) -> bool {
    decode_b64_text(key_b64)
        .and_then(|t| minisign_verify::PublicKey::decode(&t).map_err(|e| e.to_string()))
        .is_ok()
}

/// `app-v<v>/qrtz_<v>_aarch64.app.tar.gz` (the release tag, then the asset release.yml uploads)
pub fn archive_path(version: &str) -> String {
    format!("app-v{version}/qrtz_{version}_aarch64.app.tar.gz")
}

/// One release as the feed announces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: Option<String>,
    /// Relative to [`ARCHIVE_BASE`], always [`archive_path`] of `version`.
    pub path: String,
    pub signature: String,
    pub sha256: String,
    pub size: u64,
    /// When the feed that announced it stops being trusted (unix seconds).
    pub expires: u64,
}

fn plain_version(v: &str) -> Result<semver::Version, String> {
    let p = semver::Version::parse(v).map_err(|_| format!("version {v:?} isn't x.y.z"))?;
    if !p.pre.is_empty() || !p.build.is_empty() {
        return Err(format!("version {v:?} isn't a plain release"));
    }
    Ok(p)
}

/// Parse `latest.json` with archives under `host` ([`ARCHIVE_BASE`]). The archive URL must be exactly
/// `<host>/` + [`archive_path`] of the announced version: a feed can't point qrtz anywhere else, or pair a version
/// with another version's archive.
pub fn parse_feed(bytes: &[u8], host: &str) -> Result<Release, String> {
    if bytes.len() > MAX_FEED_BYTES {
        return Err("the update feed is too large".into());
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("the update feed isn't json: {e}"))?;
    let version = v["version"].as_str().ok_or("the update feed has no version")?;
    let version = version.strip_prefix('v').unwrap_or(version);
    plain_version(version)?;
    let p = &v["platforms"][TARGET];
    if !p.is_object() {
        return Err(format!("the update feed has no {TARGET} release"));
    }
    let path = archive_path(version);
    let want = format!("{}/{path}", host.trim_end_matches('/'));
    if p["url"].as_str() != Some(want.as_str()) {
        return Err(format!("the update feed's archive isn't {want}"));
    }
    let signature = p["signature"].as_str().unwrap_or_default().trim();
    if signature.is_empty() || signature.len() > 4096 {
        return Err("the update feed has no signature".into());
    }
    let sha256 = p["sha256"].as_str().unwrap_or_default();
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("the update feed has no sha256".into());
    }
    let size = p["size"].as_u64().unwrap_or(0);
    if size == 0 || size > MAX_ARCHIVE_BYTES {
        return Err("the update feed's archive size is missing or too large".into());
    }
    let expires = v["expires"]
        .as_str()
        .and_then(parse_utc)
        .ok_or("the update feed has no expires (YYYY-MM-DDTHH:MM:SSZ)")?;
    let notes = v["notes"]
        .as_str()
        .map(|n| n.chars().take(MAX_NOTES).collect::<String>());
    Ok(Release {
        version: version.to_owned(),
        notes: notes.filter(|n| !n.trim().is_empty()),
        path,
        signature: signature.to_owned(),
        sha256: sha256.to_owned(),
        size,
        expires,
    })
}

/// `YYYY-MM-DDTHH:MM:SSZ` (UTC, no fraction) as unix seconds.
pub fn parse_utc(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        t.bytes().all(|c| c.is_ascii_digit()).then(|| t.parse().ok())?
    };
    let (y, mo, d, h, mi, se) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + mi * 60 + se).ok()
}

/// The feed is still good at `now` (unix seconds): not expired, and not claiming more than [`MAX_FEED_LIFETIME`].
pub fn check_fresh(expires: u64, now: u64) -> Result<(), String> {
    if expires <= now {
        return Err("the update feed has expired (the release host serves an old one)".into());
    }
    if expires - now > MAX_FEED_LIFETIME.as_secs() {
        return Err("the update feed claims to be good for too long".into());
    }
    Ok(())
}

/// Open the signed envelope `latest.json` is (`{"feed": <base64 of the feed's bytes>, "signature": <base64 of the
/// minisign signature of those bytes>}`): the feed's bytes, if the signature verifies against `key_b64`.
pub fn open_feed(envelope: &[u8], key_b64: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    if envelope.len() > MAX_FEED_BYTES {
        return Err("the update feed is too large".into());
    }
    let v: Value =
        serde_json::from_slice(envelope).map_err(|e| format!("the update feed isn't json: {e}"))?;
    let feed = v["feed"]
        .as_str()
        .and_then(|f| base64::engine::general_purpose::STANDARD.decode(f).ok())
        .ok_or("the update feed isn't signed (no feed)")?;
    let sig = v["signature"].as_str().ok_or("the update feed isn't signed")?;
    let (key, sig) = decode_key_and_sig(key_b64, sig)?;
    key.verify(&feed, &sig, false)
        .map_err(|e| format!("the update feed's signature doesn't verify: {e}"))?;
    Ok(feed)
}

fn decode_key_and_sig(
    key_b64: &str,
    sig_b64: &str,
) -> Result<(minisign_verify::PublicKey, minisign_verify::Signature), String> {
    let key = minisign_verify::PublicKey::decode(
        &decode_b64_text(key_b64).map_err(|e| format!("app-update key: {e}"))?,
    )
    .map_err(|e| format!("app-update key: {e}"))?;
    let sig =
        minisign_verify::Signature::decode(&decode_b64_text(sig_b64).map_err(|e| format!("signature: {e}"))?)
            .map_err(|e| format!("signature: {e}"))?;
    Ok((key, sig))
}

/// `latest` is strictly newer than `current` (both plain `x.y.z`). Equal, older or unparsable is never an update:
/// qrtz never downgrades or re-installs itself.
pub fn is_update(current: &str, latest: &str) -> bool {
    match (plain_version(current), plain_version(latest)) {
        (Ok(c), Ok(l)) => l > c,
        _ => false,
    }
}

fn decode_b64_text(s: &str) -> Result<String, String> {
    use base64::Engine;
    let b = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|_| "not base64".to_string())?;
    String::from_utf8(b).map_err(|_| "not text".into())
}

/// The minisign signature (base64 of the `.sig` text `tauri signer sign` writes) of `bytes` verifies against
/// `key_b64`, and the version it was signed for (`version:` in its trusted comment, covered by the signature) is
/// `version`. A signature without a signed version is refused.
pub fn verify_signature(bytes: &[u8], sig_b64: &str, key_b64: &str, version: &str) -> Result<(), String> {
    verify_signature_from(bytes, sig_b64, key_b64, version)
}

/// [`verify_signature`] over a stream (the downloaded archive file), 1 MiB at a time.
pub fn verify_signature_from(
    mut r: impl std::io::Read,
    sig_b64: &str,
    key_b64: &str,
    version: &str,
) -> Result<(), String> {
    let (key, sig) = decode_key_and_sig(key_b64, sig_b64)?;
    let mut v = key
        .verify_stream(&sig)
        .map_err(|e| format!("the update's signature doesn't verify: {e}"))?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf).map_err(|e| format!("reading the update: {e}"))?;
        if n == 0 {
            break;
        }
        v.update(&buf[..n]);
    }
    v.finalize()
        .map_err(|e| format!("the update's signature doesn't verify: {e}"))?;
    let signed = sig
        .trusted_comment()
        .split('\t')
        .find_map(|f| f.strip_prefix("version:"))
        .ok_or("the update's signature names no version")?;
    if signed.trim_start_matches('v') != version {
        return Err(format!("the update is signed for {signed}, not {version}"));
    }
    Ok(())
}

/// The requirement a new qrtz.app must meet: Developer ID, team MR26CCZ496, identifier ai.qrtz.desktop, notarized.
pub fn app_requirement() -> String {
    format!(
        "{} and notarized",
        qrtz_modules::manifest::designated_requirement(
            APP_IDENTIFIER,
            qrtz_modules::manifest::RELEASE_TEAM_ID
        )
    )
}

/// `spctl --assess --type execute -vv` said the bundle is accepted as notarized Developer ID code.
pub fn spctl_says_notarized(out: &str) -> bool {
    out.lines().any(|l| l.trim_end().ends_with(": accepted")) && out.contains("source=Notarized Developer ID")
}

/// Gatekeeper is switched off on this Mac (`spctl --master-disable`): its verdict means nothing, and only the
/// `codesign -R … and notarized` check speaks for the ticket.
pub fn spctl_disabled(out: &str) -> bool {
    out.contains("assessments disabled")
}

/// What the settings row shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// No app-update key in this build: nothing is asked of any host.
    NotSetUp,
    /// Not checked yet this launch.
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading {
        release: Release,
        done: u64,
    },
    /// Verifying the archive and the new bundle, and swapping it in.
    Installing(Release),
    /// The new version is in place: "restart to update".
    Ready(Release),
    Failed {
        message: String,
        release: Option<Release>,
    },
}

impl State {
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotSetUp => "not-set-up",
            Self::Idle => "idle",
            Self::Checking => "checking",
            Self::UpToDate => "up-to-date",
            Self::Available(_) => "available",
            Self::Downloading { .. } => "downloading",
            Self::Installing(_) => "installing",
            Self::Ready(_) => "ready",
            Self::Failed { .. } => "failed",
        }
    }
    fn release(&self) -> Option<&Release> {
        match self {
            Self::Available(r) | Self::Installing(r) | Self::Ready(r) => Some(r),
            Self::Downloading { release, .. } => Some(release),
            Self::Failed { release, .. } => release.as_ref(),
            _ => None,
        }
    }
    fn busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing(_) | Self::Ready(_)
        )
    }
}

/// The `app-update:status` answer (and `app-update:state` event).
pub fn to_json(current: &str, s: &State, checked_at_ms: Option<u64>) -> Value {
    let r = s.release();
    let blocked = match s {
        State::Failed { message, .. } => crate::update_redirect::RedirectBlocked::from_message(message),
        _ => None,
    };
    json!({
        "current": current,
        "state": s.name(),
        "latest": r.map(|r| r.version.clone()),
        "notes": r.and_then(|r| r.notes.clone()),
        "sizeBytes": r.map(|r| r.size),
        "doneBytes": match s { State::Downloading { done, .. } => Some(*done), _ => None },
        "error": match s { State::Failed { message, .. } => Some(message.clone()), _ => None },
        // QRT-313: a download sent somewhere other than GitHub's asset hosts (foo's "couldn't download" message).
        "errorCode": blocked.as_ref().map(|_| crate::update_redirect::CODE),
        "errorDetails": blocked.map(|b| json!({ "host": b.host, "scheme": b.scheme })),
        "checkedAtMs": checked_at_ms,
    })
}

/// What the controller needs from the outside world: the release files, and the swap.
#[async_trait]
pub trait Backend: Send + Sync {
    /// The feed's bytes (`None` = the host has no feed yet).
    async fn fetch_feed(&self) -> Result<Option<Vec<u8>>, String>;
    /// Stream the archive into the new file `dest` (never all in memory); `progress(done, total)`.
    async fn fetch_archive(
        &self,
        r: &Release,
        dest: &std::path::Path,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), String>;
    /// Unpack, check (step 4 of the module doc) and swap the new qrtz.app in. Only called with a verified archive.
    /// `commit` raises the high-water mark: call it after every check passed, right before the swap, and swap only
    /// if it succeeds (ADR 0181).
    async fn install(
        &self,
        r: &Release,
        archive: &std::path::Path,
        commit: HighWaterCommit,
    ) -> Result<(), String>;
    /// What the archive URL must start with ([`ARCHIVE_BASE`]).
    fn host(&self) -> String;
}

/// Raises the high-water mark to one version; handed to [`Backend::install`] to call right before the swap.
pub struct HighWaterCommit {
    path: Option<std::path::PathBuf>,
    current: String,
    version: String,
}

impl HighWaterCommit {
    /// For a backend test: a commit that keeps no mark.
    pub fn none(version: &str) -> Self {
        Self {
            path: None,
            current: String::new(),
            version: version.to_owned(),
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Raise the mark to the version (only ever raised; written atomically). Errs, and then nothing may be swapped
    /// in, when the version isn't above the build and the mark.
    pub fn raise(self) -> Result<(), String> {
        let Some(p) = &self.path else {
            return Ok(());
        };
        let floor = floor_of(&self.current, Some(p));
        if !is_update(&floor, &self.version) {
            return Err(format!(
                "{} isn't above the highest version accepted ({floor})",
                self.version
            ));
        }
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, format!("{}\n", self.version))
            .and_then(|()| std::fs::rename(&tmp, p))
            .map_err(|e| format!("couldn't keep the update's version: {e}"))
    }
}

fn floor_of(current: &str, high_water: Option<&std::path::PathBuf>) -> String {
    let hw = high_water
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| plain_version(s).is_ok());
    match hw {
        Some(h) if is_update(current, &h) => h,
        _ => current.to_owned(),
    }
}

struct Inner {
    state: State,
    checked_at_ms: Option<u64>,
}

/// The app's own updater (one per process).
pub struct SelfUpdate {
    current: String,
    key: Option<String>,
    backend: Arc<dyn Backend>,
    inner: Mutex<Inner>,
    events: Option<Events>,
    /// Where the highest version this install ever swapped in is kept (`app-update-high-water`).
    high_water: Option<std::path::PathBuf>,
    /// Where downloads are streamed to (a private temp dir under it per download).
    work_dir: std::path::PathBuf,
    /// Unix seconds now (a test sets it).
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

/// ADR 0181: the high-water mark file, in qrtz's data dir.
pub const HIGH_WATER_FILE: &str = "app-update-high-water";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl SelfUpdate {
    /// `key`: the app-update public key ([`pinned_key`] in the app); `None` = not set up.
    pub fn new(current: impl Into<String>, key: Option<String>, backend: Arc<dyn Backend>) -> Arc<Self> {
        let state = if key.is_some() {
            State::Idle
        } else {
            State::NotSetUp
        };
        Arc::new(Self {
            current: current.into(),
            key,
            backend,
            inner: Mutex::new(Inner {
                state,
                checked_at_ms: None,
            }),
            events: None,
            high_water: None,
            work_dir: std::env::temp_dir(),
            clock: Arc::new(|| now_ms() / 1000),
        })
    }

    /// Stream downloads into a temp dir under `dir` (the app's data dir) instead of the system temp dir.
    pub fn with_work_dir(self: Arc<Self>, dir: impl Into<std::path::PathBuf>) -> Arc<Self> {
        let mut me = Arc::try_unwrap(self).unwrap_or_else(|_| panic!("with_work_dir before sharing"));
        me.work_dir = dir.into();
        Arc::new(me)
    }

    /// The clock feed expiry is judged by (unix seconds).
    pub fn with_clock(self: Arc<Self>, clock: impl Fn() -> u64 + Send + Sync + 'static) -> Arc<Self> {
        let mut me = Arc::try_unwrap(self).unwrap_or_else(|_| panic!("with_clock before sharing"));
        me.clock = Arc::new(clock);
        Arc::new(me)
    }

    /// ADR 0181: keep the highest signed version accepted in `path`; a feed or archive below it is refused even
    /// when it is newer than this build and validly signed (a replayed old release after a rollback of the app).
    pub fn with_high_water(self: Arc<Self>, path: impl Into<std::path::PathBuf>) -> Arc<Self> {
        let mut me = Arc::try_unwrap(self).unwrap_or_else(|_| panic!("with_high_water before sharing"));
        me.high_water = Some(path.into());
        Arc::new(me)
    }

    /// The floor any update must be strictly above: this build, or the high-water mark if higher.
    pub fn floor(&self) -> String {
        floor_of(&self.current, self.high_water.as_ref())
    }

    pub fn with_events(self: Arc<Self>, events: Events) -> Arc<Self> {
        let mut me = Arc::try_unwrap(self).unwrap_or_else(|_| panic!("with_events before sharing"));
        me.events = Some(events);
        Arc::new(me)
    }

    pub fn state(&self) -> State {
        self.lock().state.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn status(&self) -> Value {
        let g = self.lock();
        to_json(&self.current, &g.state, g.checked_at_ms)
    }

    fn set(&self, s: State) {
        let v = {
            let mut g = self.lock();
            g.state = s;
            to_json(&self.current, &g.state, g.checked_at_ms)
        };
        if let Some(e) = &self.events {
            e.emit("app-update:state", v);
        }
    }

    /// Ask the feed. `quiet` (the launch and 6-hour checks): a failure leaves the previous state and is only logged.
    pub async fn check(&self, quiet: bool) -> State {
        let prev = {
            let g = self.lock();
            if self.key.is_none() || g.state.busy() {
                return g.state.clone();
            }
            g.state.clone()
        };
        self.set(State::Checking);
        let res = async {
            let Some(b) = self.backend.fetch_feed().await? else {
                return Ok(None);
            };
            let key = self.key.as_deref().ok_or("updates aren't set up")?;
            let r = parse_feed(&open_feed(&b, key)?, &self.backend.host())?;
            check_fresh(r.expires, (self.clock)())?;
            Ok(Some(r))
        }
        .await;
        self.lock().checked_at_ms = Some(now_ms());
        let next = match res {
            Ok(Some(r)) if is_update(&self.floor(), &r.version) => State::Available(r),
            Ok(_) => State::UpToDate,
            Err(e) if quiet => {
                tracing::warn!(error = %e, "app update check failed");
                if matches!(prev, State::Checking) {
                    State::Idle
                } else {
                    prev
                }
            }
            Err(e) => State::Failed {
                message: e,
                release: None,
            },
        };
        self.set(next.clone());
        next
    }

    /// Take the release the last check found and mark it downloading (`None`: nothing to download, or it isn't
    /// newer than this build).
    fn take_release(&self) -> Option<(Release, String)> {
        let key = self.key.clone()?;
        let v = {
            let mut g = self.lock();
            let r = match &g.state {
                State::Available(r) | State::Failed { release: Some(r), .. } => r.clone(),
                _ => return None,
            };
            if !is_update(&self.floor(), &r.version) {
                return None;
            }
            g.state = State::Downloading {
                release: r.clone(),
                done: 0,
            };
            (r, to_json(&self.current, &g.state, g.checked_at_ms))
        };
        if let Some(e) = &self.events {
            e.emit("app-update:state", v.1);
        }
        Some((v.0, key))
    }

    /// Download, verify and swap in the release the last check found (the user pressed "update").
    pub async fn download(&self) -> State {
        match self.take_release() {
            Some((r, key)) => self.finish_download(r, key).await,
            None => self.state(),
        }
    }

    async fn finish_download(&self, r: Release, key: String) -> State {
        let next = match self.fetch_and_install(&r, &key).await {
            Ok(()) => State::Ready(r),
            Err(message) => {
                tracing::warn!(error = %message, version = %r.version, "app update not installed");
                State::Failed {
                    message,
                    release: Some(r),
                }
            }
        };
        self.set(next.clone());
        next
    }

    async fn fetch_and_install(&self, r: &Release, key: &str) -> Result<(), String> {
        // Never a downgrade or a re-install, whatever state led here: above this build and the high-water mark.
        let floor = self.floor();
        if !is_update(&floor, &r.version) {
            return Err(format!("{} isn't newer than {floor}", r.version));
        }
        let last = Mutex::new(0u64);
        let progress = |done: u64, _total: u64| {
            let mut l = last.lock().unwrap_or_else(|p| p.into_inner());
            // About every 1%: enough for the bar, few events.
            if done == r.size || done.saturating_sub(*l) >= (r.size / 100).max(1) {
                *l = done;
                self.set(State::Downloading {
                    release: r.clone(),
                    done,
                });
            }
        };
        check_fresh(r.expires, (self.clock)())?;
        std::fs::create_dir_all(&self.work_dir).map_err(|e| format!("{}: {e}", self.work_dir.display()))?;
        let dl = tempfile::Builder::new()
            .prefix(".qrtz-update-download-")
            .tempdir_in(&self.work_dir)
            .map_err(|e| format!("no room for the download: {e}"))?;
        let file = dl.path().join("update.tar.gz");
        self.backend.fetch_archive(r, &file, &progress).await?;
        self.set(State::Installing(r.clone()));
        let (size, sha) = {
            let file = file.clone();
            tokio::task::spawn_blocking(move || qrtz_modules::fsutil::sha256_file(&file))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| format!("the update: {e}"))?
        };
        if size != r.size {
            return Err(format!("the update is {size} bytes, the feed says {}", r.size));
        }
        if sha != r.sha256 {
            return Err("the update's sha256 isn't the feed's".into());
        }
        {
            let (file, sig, key, v) = (
                file.clone(),
                r.signature.clone(),
                key.to_owned(),
                r.version.clone(),
            );
            tokio::task::spawn_blocking(move || {
                let f = std::fs::File::open(&file).map_err(|e| format!("the update: {e}"))?;
                verify_signature_from(std::io::BufReader::new(f), &sig, &key, &v)
            })
            .await
            .map_err(|e| e.to_string())??;
        }
        // The mark is raised by the install itself, after the new bundle passed every check and right before the
        // swap: a failure anywhere before leaves this version free to retry.
        let commit = HighWaterCommit {
            path: self.high_water.clone(),
            current: self.current.clone(),
            version: r.version.clone(),
        };
        let res = self.backend.install(r, &file, commit).await;
        drop(dl);
        res
    }

    /// `true` when the new version is in place and qrtz may restart into it.
    pub fn ready(&self) -> bool {
        matches!(self.lock().state, State::Ready(_))
    }

    /// The quiet checks: once [`FIRST_CHECK_AFTER`] launch, then every [`CHECK_EVERY`]. Call once, inside tokio.
    pub fn start_schedule(self: &Arc<Self>) {
        if self.key.is_none() {
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(FIRST_CHECK_AFTER).await;
            loop {
                me.check(true).await;
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
    }

    /// `app-update:*` except `restart`, which the service answers (it owns the shell's restart).
    pub async fn call(self: &Arc<Self>, channel: &str) -> Result<Value, AppError> {
        match channel {
            "app-update:status" => Ok(self.status()),
            "app-update:check" => {
                self.check(false).await;
                Ok(self.status())
            }
            "app-update:download" => {
                // Answers at once (downloading); the row follows `app-update:state`.
                if let Some((r, key)) = self.take_release() {
                    let me = self.clone();
                    tokio::spawn(async move {
                        me.finish_download(r, key).await;
                    });
                }
                Ok(self.status())
            }
            _ => Err(AppError::new(
                ErrorCode::Validation,
                format!("unknown channel {channel}"),
            )),
        }
    }
}

/// QRT-313: the release files from GitHub, through [`crate::module_hosting::ReleaseTransport`] (every redirect
/// hop judged by `update_redirect.rs`).
#[cfg(feature = "runtime")]
#[derive(Default)]
pub struct GithubBackend {
    pub transport: crate::module_hosting::ReleaseTransport,
}

#[cfg(feature = "runtime")]
#[async_trait]
impl Backend for GithubBackend {
    async fn fetch_feed(&self) -> Result<Option<Vec<u8>>, String> {
        let mut out = Vec::new();
        let found = self
            .transport
            .get(FEED_URL, MAX_FEED_BYTES as u64, &mut |c: &[u8]| {
                out.extend_from_slice(c);
                Ok(())
            })
            .await?;
        Ok(found.then_some(out))
    }
    async fn fetch_archive(
        &self,
        r: &Release,
        dest: &std::path::Path,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), String> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)
            .map_err(|e| format!("{}: {e}", dest.display()))?;
        let url = format!("{ARCHIVE_BASE}/{}", r.path);
        let mut done = 0u64;
        let found = self
            .transport
            .get(&url, r.size, &mut |c: &[u8]| {
                f.write_all(c).map_err(|e| format!("{}: {e}", dest.display()))?;
                done += c.len() as u64;
                progress(done, r.size);
                Ok(())
            })
            .await?;
        if !found {
            return Err(format!("{}: not found", r.path));
        }
        if done != r.size {
            return Err(format!("{}: short read", r.path));
        }
        f.sync_all().map_err(|e| format!("{}: {e}", dest.display()))
    }
    async fn install(
        &self,
        r: &Release,
        archive: &std::path::Path,
        commit: HighWaterCommit,
    ) -> Result<(), String> {
        let version = r.version.clone();
        let archive = archive.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let bundle = running_bundle()?;
            install_bundle(&archive, &version, &bundle, commit)
        })
        .await
        .map_err(|e| e.to_string())?
    }
    fn host(&self) -> String {
        ARCHIVE_BASE.into()
    }
}

/// The qrtz.app this process runs from (`…/qrtz.app/Contents/MacOS/qrtz-app`). Refused when macOS runs it from a
/// translocated or read-only place (a disk image, App Translocation): it can't replace itself there.
pub fn running_bundle() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    bundle_of(&exe)
}

pub fn bundle_of(exe: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let macos = exe.parent().ok_or("no app bundle")?;
    let contents = macos.parent().ok_or("no app bundle")?;
    let app = contents.parent().ok_or("no app bundle")?;
    if macos.file_name() != Some("MacOS".as_ref())
        || contents.file_name() != Some("Contents".as_ref())
        || app.extension() != Some("app".as_ref())
    {
        return Err("qrtz isn't running from qrtz.app (a dev build updates from git)".into());
    }
    let s = app.to_string_lossy();
    if s.contains("/AppTranslocation/") || s.starts_with("/Volumes/") {
        return Err("move qrtz to your applications folder first, then update".into());
    }
    Ok(app.to_path_buf())
}

/// Unpack `archive` next to `bundle` (same volume), check the new qrtz.app (identifier, `version`, Developer ID of
/// team MR26CCZ496, notarized), raise the high-water mark (`commit`), and swap it with `bundle` atomically
/// (`renamex_np(RENAME_SWAP)`); the old bundle is deleted. Nothing of the new bundle runs here.
#[cfg(target_os = "macos")]
pub fn install_bundle(
    archive: &std::path::Path,
    version: &str,
    bundle: &std::path::Path,
    commit: HighWaterCommit,
) -> Result<(), String> {
    let parent = bundle.parent().ok_or("no folder above qrtz.app")?;
    let stage = tempfile::Builder::new()
        .prefix(".qrtz-update-")
        .tempdir_in(parent)
        .map_err(|e| {
            format!(
                "can't write to {}: {e} (qrtz can't replace itself there)",
                parent.display()
            )
        })?;
    let out = stage.path().join("x");
    std::fs::create_dir(&out).map_err(|e| e.to_string())?;
    // bsdtar refuses absolute paths and `..` by default.
    run_tool(
        "/usr/bin/tar",
        &["-xzf", path_str(archive)?, "-C", path_str(&out)?],
    )?;
    let names: Vec<_> = std::fs::read_dir(&out)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    if names.len() != 1 || names[0] != "qrtz.app" {
        return Err("the update archive doesn't hold exactly qrtz.app".into());
    }
    let new = out.join("qrtz.app");
    if std::fs::symlink_metadata(&new).map_or(true, |m| !m.is_dir()) {
        return Err("the update's qrtz.app isn't a folder".into());
    }
    check_new_bundle(&new, version)?;
    if commit.version() != version {
        return Err("the high-water mark is for another version".into());
    }
    // ADR 0181 (U1): the mark goes up only now, with every check passed, right before the swap.
    commit.raise()?;
    swap(&new, bundle)?;
    // `new` now holds the old bundle; the running process keeps its mapped pages.
    let _ = std::fs::remove_dir_all(stage.path());
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn install_bundle(
    _archive: &std::path::Path,
    _version: &str,
    _bundle: &std::path::Path,
    _commit: HighWaterCommit,
) -> Result<(), String> {
    Err("qrtz updates itself only on macOS".into())
}

#[cfg(target_os = "macos")]
fn path_str(p: &std::path::Path) -> Result<&str, String> {
    p.to_str().ok_or_else(|| "path isn't utf-8".to_string())
}

#[cfg(target_os = "macos")]
fn run_tool(cmd: &str, args: &[&str]) -> Result<String, String> {
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

/// Step 4 of the module doc, all on this Mac.
#[cfg(target_os = "macos")]
pub fn check_new_bundle(app: &std::path::Path, version: &str) -> Result<(), String> {
    crate::module_hosting::strip_quarantine(app)?;
    let plist = app.join("Contents/Info.plist");
    let p = path_str(&plist)?;
    let read = |k: &str| {
        run_tool("/usr/bin/plutil", &["-extract", k, "raw", "-o", "-", p]).map(|s| s.trim().to_owned())
    };
    let id = read("CFBundleIdentifier")?;
    if id != APP_IDENTIFIER {
        return Err(format!("the update is {id}, not {APP_IDENTIFIER}"));
    }
    let v = read("CFBundleShortVersionString")?;
    if v != version {
        return Err(format!("the update's qrtz.app is {v}, the feed says {version}"));
    }
    let a = path_str(app)?;
    run_tool(
        "/usr/bin/codesign",
        &[
            "--verify",
            "--deep",
            "--strict",
            &format!("-R={}", app_requirement()),
            a,
        ],
    )
    .map_err(|e| format!("the update isn't qrtz's notarized Developer ID build: {e}"))?;
    let gk = std::process::Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute", "-vv", a])
        .output()
        .map_err(|e| format!("spctl: {e}"))?;
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&gk.stdout),
        String::from_utf8_lossy(&gk.stderr)
    );
    let accepted = gk.status.success() && spctl_says_notarized(&out);
    if !(accepted || spctl_disabled(&out)) {
        return Err(format!(
            "gatekeeper doesn't accept the update as notarized: {}",
            out.trim()
        ));
    }
    Ok(())
}

/// What a failed `renamex_np(RENAME_SWAP)` allows (ADR 0181, U2): only a volume that doesn't support the swap
/// (`ENOTSUP`) gets the two-rename fallback; any other error fails the update and leaves the old app untouched.
pub fn swap_fallback_allowed(errno: i32) -> bool {
    errno == libc::ENOTSUP
}

/// Swap two paths in one step (APFS `RENAME_SWAP`). Only where the volume can't (`ENOTSUP`): two renames with a
/// rollback, which isn't atomic (a crash between them leaves `qrtz.old` and no qrtz.app).
#[cfg(target_os = "macos")]
fn swap(new: &std::path::Path, old: &std::path::Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let a = CString::new(new.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let b = CString::new(old.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: two valid NUL-terminated paths; renamex_np only reads them.
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if !err.raw_os_error().is_some_and(swap_fallback_allowed) {
        return Err(format!("can't swap the new qrtz.app in: {err}"));
    }
    let aside = new.with_extension("old");
    std::fs::rename(old, &aside).map_err(|e| format!("can't move the running qrtz.app aside: {e}"))?;
    if let Err(e) = std::fs::rename(new, old) {
        let _ = std::fs::rename(&aside, old);
        return Err(format!("can't put the new qrtz.app in place: {e}"));
    }
    std::fs::rename(&aside, new).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_file_is_a_todo_or_one_valid_key() {
        match pinned_key() {
            None => assert!(
                KEY_FILE.contains("TODO(ADR 0181)"),
                "an empty key file keeps its TODO"
            ),
            Some(k) => assert!(key_is_valid(k), "app_update_key.pub: not a minisign public key"),
        }
        assert_eq!(key_from("# a\n\n  KEY  \n# b"), Some("KEY"));
        assert_eq!(key_from("# only comments\n"), None);
    }

    /// ADR 0181 TODO: a release build without the app-update key could never update itself again. `cargo test
    /// --release` (app-release.yml runs it) fails loudly until `setup-secrets.sh app-update` wrote the key.
    #[cfg(not(debug_assertions))]
    #[test]
    fn release_builds_pin_the_app_update_key() {
        let k = pinned_key().expect(
            "crates/qrtz-app/src/app_update_key.pub has no key: run `scripts/ci/setup-secrets.sh app-update` on the \
             release Mac and commit the file (ADR 0181)",
        );
        assert!(key_is_valid(k));
    }

    #[test]
    fn the_requirement_names_the_app_the_team_and_notarization() {
        let r = app_requirement();
        assert!(
            r.starts_with("identifier \"ai.qrtz.desktop\" and anchor apple generic"),
            "{r}"
        );
        assert!(r.contains("certificate leaf[subject.OU] = \"MR26CCZ496\""), "{r}");
        assert!(r.ends_with(" and notarized"), "{r}");
    }

    #[test]
    fn gatekeeper_verdicts() {
        let ok = "/Applications/qrtz.app: accepted\nsource=Notarized Developer ID\norigin=Developer ID Application: Jared Grimes (MR26CCZ496)\n";
        assert!(spctl_says_notarized(ok));
        assert!(
            !spctl_says_notarized("/x/qrtz.app: accepted\nsource=Developer ID\n"),
            "signed, not notarized"
        );
        assert!(!spctl_says_notarized(
            "/x/qrtz.app: rejected\nsource=Unnotarized Developer ID\n"
        ));
        assert!(spctl_disabled(
            "/x/qrtz.app: accepted\nsource=assessments disabled\n"
        ));
    }

    #[test]
    fn the_bundle_is_found_from_its_executable() {
        let p = std::path::Path::new("/Applications/qrtz.app/Contents/MacOS/qrtz-app");
        assert_eq!(
            bundle_of(p).unwrap(),
            std::path::Path::new("/Applications/qrtz.app")
        );
        assert!(bundle_of(std::path::Path::new("/repo/target/debug/qrtz-app")).is_err());
        assert!(bundle_of(std::path::Path::new(
            "/Volumes/qrtz/qrtz.app/Contents/MacOS/qrtz-app"
        ))
        .is_err());
        assert!(bundle_of(std::path::Path::new(
            "/private/var/folders/x/AppTranslocation/1/d/qrtz.app/Contents/MacOS/qrtz-app"
        ))
        .is_err());
    }

    #[test]
    fn only_an_unsupported_swap_falls_back_to_two_renames() {
        assert!(swap_fallback_allowed(libc::ENOTSUP));
        for e in [
            libc::EACCES,
            libc::EPERM,
            libc::ENOENT,
            libc::EXDEV,
            libc::EBUSY,
            libc::EINVAL,
        ] {
            assert!(!swap_fallback_allowed(e), "{e}");
        }
    }

    #[test]
    fn feed_times_parse_and_expire() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2026-10-19T05:00:00Z"), Some(1_792_386_000));
        assert_eq!(parse_utc("2024-02-29T23:59:59Z"), Some(1_709_251_199));
        for bad in [
            "2026-10-19",
            "2026-10-19T05:00:00.000Z",
            "2026-13-01T00:00:00Z",
            "2026-10-19T05:00:00+00:00",
            "+026-10-19T05:00:00Z",
        ] {
            assert_eq!(parse_utc(bad), None, "{bad}");
        }
        let now = 1_000_000_000;
        assert!(check_fresh(now + 1, now).is_ok());
        assert!(check_fresh(now, now).unwrap_err().contains("expired"));
        assert!(check_fresh(now - 1, now).is_err());
        assert!(check_fresh(now + MAX_FEED_LIFETIME.as_secs(), now).is_ok());
        assert!(check_fresh(now + MAX_FEED_LIFETIME.as_secs() + 1, now).is_err());
    }

    #[test]
    fn versions_only_go_up() {
        assert!(is_update("0.2.0", "0.2.1"));
        assert!(is_update("0.9.9", "0.10.0"));
        assert!(!is_update("0.2.1", "0.2.1"), "never a re-install");
        assert!(!is_update("0.2.1", "0.2.0"), "never a downgrade");
        assert!(!is_update("0.2.0", "0.3.0-beta.1"), "never a prerelease");
        assert!(!is_update("0.2.0", "latest"));
    }
}
