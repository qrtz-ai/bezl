//! The on-disk speech module store (ADR 0046 §4-5, rev 3 + Bozniak's rev-2 review N61-N78):
//!
//! ```text
//! <root>/ 0700
//!   current.json(.bak) 0600  pointer + anti-rollback state (temp + fsync + rename; the .bak is the last good copy)
//!   staging/<v>/       0700  downloads (`.part` files 0600), verified here
//!   versions/<v>/      0500  installed, read-only (files 0400, entry 0500); only N and N-1 are kept
//!   versions/.cand-<v>-<n>/  a verified candidate awaiting its health check, then renamed or exchanged into place
//! ```
//!
//! All pointer moves and pin counts happen under one mutex (N74); the store hands out opened, hashed files only
//! (N73, `files.rs`).
use crate::bundle::{self, Manifest, PinnedKey};
use crate::files::{self, ModuleFiles};
use crate::fsx;
use crate::SpeechError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// Speech dir disk cap (N + N-1 + one staging bundle).
pub const DISK_CAP_BYTES: u64 = 3 * 1024 * 1024 * 1024;
pub const CRASH_LOOP_FAILURES: usize = 3;
pub const CRASH_LOOP_WINDOW_MS: u64 = 5 * 60 * 1000;
pub const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Fail-open has a floor (condition 5, N70): a revocation list older than this stops sessions.
pub const REVOCATION_HARD_CAP_MS: u64 = 90 * DAY_MS;
/// N63: a list is valid at most this long after it was issued, whatever it says (a stolen key cannot sign a
/// 70-year list); and an `issued` this far ahead of our clock mark is refused (bounded clock skew).
pub const MAX_LIST_VALIDITY_MS: u64 = 8 * DAY_MS;

/// Why the pointer moves back (N71). With an expired revocation list only `CrashLoop` (to a clean N-1) is allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RollbackKind {
    User,
    CrashLoop,
    Revoked,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Revocation {
    /// `issued` of the newest list we applied (0 = none yet).
    pub issued: u64,
    /// Effective expiry (clamped, N63).
    pub expires: u64,
    /// Effective now (clock high-water mark) minus `issued`.
    pub age_ms: u64,
    /// Settings shows "speech couldn't check for revocations since <issued>".
    pub expired: bool,
    /// Past the 90-day hard cap: sessions stop (talk uses its host-side fallback).
    pub stale: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub current: Option<String>,
    pub previous: Option<String>,
    /// Monotonic: the highest version ever verified and installed. Never lowered (not even by delete).
    pub highest_verified: Option<String>,
    /// Pinned module key after a rotation (else the key built into the app).
    pub pinned_key_id: Option<String>,
    pub pinned_key: Option<String>,
    /// N63: seq space per signing key; `denylist_seq` belongs to `denylist_key_id`.
    pub denylist_key_id: Option<String>,
    pub denylist_seq: u64,
    pub denylist_expires: u64,
    pub denylist_issued: u64,
    /// Every version any list we have ever seen denied (never shrinks: condition 1).
    pub denied: Vec<String>,
    /// Persisted clock high-water mark (condition 2, N62).
    pub clock_hwm: u64,
    pub rotation_seq: u64,
    /// N64: versions that crash-looped; auto-update skips them until a newer one (a user re-download overrides).
    pub quarantined: Vec<String>,
    /// N66: a rollback found while a session was pinned; run by the next pin once idle.
    pub pending_rollback: Option<RollbackKind>,
    /// A verified, health-checked candidate waiting for idle (`activate`).
    pub waiting: Option<Waiting>,
    /// Failure times (ms) of sessions on `current`.
    pub failures: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Waiting {
    pub version: String,
    pub dir: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rollback {
    pub from: String,
    pub to: Option<String>,
    pub kind: RollbackKind,
}

/// What settings shows. No engine metadata (N72): version, size, revocation age.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreStatus {
    pub current: Option<String>,
    pub previous: Option<String>,
    pub bytes: u64,
    pub sessions: usize,
    pub waiting: Option<String>,
    pub revocation: Revocation,
}

struct Inner {
    state: State,
    /// Active pins per version (N74: under the same lock as every pointer move and sweep).
    pins: HashMap<String, usize>,
}

impl Inner {
    fn total_pins(&self) -> usize {
        self.pins.values().sum()
    }
}

/// A session's hold on one installed version: the pointer cannot move and the dir cannot be swept while it lives.
pub struct SessionPin {
    pub version: String,
    pub manifest: Manifest,
    dir: PathBuf,
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for SessionPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionPin")
            .field("version", &self.version)
            .finish()
    }
}

impl SessionPin {
    /// Open and hash every file of the pinned version (N73: on every load, from the opened fds).
    pub fn open_files(&self) -> Result<ModuleFiles, SpeechError> {
        files::open_module_files(&self.dir, &self.manifest)
    }

    /// A safe-point check for a running session (N66): `true` once any applied list denies the pinned version.
    /// The session then finishes the utterance in flight and ends; the rollback or fixed install runs once idle.
    pub fn revoked(&self) -> bool {
        self.inner.lock().unwrap().state.denied.contains(&self.version)
    }
}

/// Whether the on-device module can run a session now, and if not, why (talk picks its engine from this).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum SpeechAvailability {
    Ready {
        version: String,
    },
    NotInstalled,
    /// The current version is revoked and could not be replaced: with an expired list there is no rollback or
    /// install (fail closed, ADR 0046 §4b); with a fresh one the replacement is pending (busy or downloading).
    Revoked {
        version: String,
        list_expired: bool,
    },
    Stale {
        days: u64,
    },
}

/// Which app asks for speech (N84: consent is per app).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum App {
    Talk,
    Note,
}

/// The user's cloud consent (N84). The ElevenLabs opt-in in talk settings is `talk` only; it never covers a
/// meeting (`note`), which has its own opt-in. `privacy.localOnly` overrides both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloudConsent {
    pub talk: bool,
    pub note: bool,
    pub local_only: bool,
}

impl CloudConsent {
    pub fn allows(&self, app: App) -> bool {
        !self.local_only
            && match app {
                App::Talk => self.talk,
                App::Note => self.note,
            }
    }
}

/// The engine a talk session or a note recording uses (ADR 0046 §9.1, N84).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "engine")]
pub enum EngineChoice {
    OnDevice,
    /// The user's cloud engine (ElevenLabs, host-side), only with that app's consent and never in local-only.
    /// `fallback_reason` says why on-device speech is not used; a note recording records it on the capture record
    /// and shows it live ("transcribing in the cloud: on-device speech unavailable").
    Cloud {
        fallback_reason: String,
    },
    /// Speech unavailable: the UI says so (with the expired-list notice when `list_expired`). Capture does not
    /// start: never record silently with no transcript.
    Unavailable {
        reason: String,
        list_expired: bool,
    },
}

fn unavailable_reason(a: &SpeechAvailability) -> (&'static str, bool) {
    match a {
        SpeechAvailability::Ready { .. } => ("ready", false),
        SpeechAvailability::NotInstalled => ("not-installed", false),
        SpeechAvailability::Revoked { list_expired, .. } => ("revoked", *list_expired),
        SpeechAvailability::Stale { .. } => ("stale", true),
    }
}

pub fn choose_engine(a: &SpeechAvailability, consent: CloudConsent, app: App) -> EngineChoice {
    if let SpeechAvailability::Ready { .. } = a {
        return EngineChoice::OnDevice;
    }
    let (reason, list_expired) = unavailable_reason(a);
    if consent.allows(app) {
        return EngineChoice::Cloud {
            fallback_reason: reason.into(),
        };
    }
    EngineChoice::Unavailable {
        reason: reason.into(),
        list_expired,
    }
}

impl Drop for SessionPin {
    fn drop(&mut self) {
        let mut g = self.inner.lock().unwrap();
        if let Some(n) = g.pins.get_mut(&self.version) {
            *n -= 1;
            if *n == 0 {
                g.pins.remove(&self.version);
            }
        }
    }
}

/// A verified bundle renamed out of staging into `versions/.cand-*`, read-only, not live.
#[derive(Debug)]
pub struct Candidate {
    pub version: String,
    pub manifest: Manifest,
    dir: PathBuf,
}

impl Candidate {
    pub fn open_files(&self) -> Result<ModuleFiles, SpeechError> {
        files::open_module_files(&self.dir, &self.manifest)
    }
}

pub struct SpeechStore {
    root: PathBuf,
    app_version: semver::Version,
    builtin_key: PinnedKey,
    root_key: PinnedKey,
    inner: Arc<Mutex<Inner>>,
}

fn v(s: &str) -> semver::Version {
    semver::Version::parse(s).unwrap_or(semver::Version::new(0, 0, 0))
}

fn parse_state(b: &[u8]) -> Option<State> {
    let st: State = serde_json::from_slice(b).ok()?;
    if let (Some(id), Some(k)) = (&st.pinned_key_id, &st.pinned_key) {
        PinnedKey::from_hex(id, k).ok()?;
    } else if st.pinned_key_id.is_some() != st.pinned_key.is_some() {
        return None;
    }
    Some(st)
}

impl SpeechStore {
    /// `builtin_key`: the module signing key built into the app; `root_key`: the offline speech root key that signs
    /// rotations (N60). A corrupt state file falls back to the last good copy; with none, open fails closed (N67).
    pub fn open(
        root: impl AsRef<Path>,
        app_version: &str,
        builtin_key: PinnedKey,
        root_key: PinnedKey,
    ) -> Result<Self, SpeechError> {
        let root = root.as_ref().to_path_buf();
        fsx::mkdir_private(&root)?;
        fsx::mkdir_private(&root.join("staging"))?;
        fsx::mkdir_private(&root.join("versions"))?;
        let main = std::fs::read(root.join("current.json")).ok();
        let bak = std::fs::read(root.join("current.json.bak")).ok();
        let state = match (
            main.as_deref().and_then(parse_state),
            bak.as_deref().and_then(parse_state),
        ) {
            (Some(s), _) => s,
            (None, Some(s)) => {
                tracing::warn!("speech state was corrupt; restored the last good copy");
                s
            }
            (None, None) if main.is_none() && bak.is_none() => State::default(),
            _ => return Err(SpeechError::StateCorrupt),
        };
        let s = Self {
            root,
            app_version: semver::Version::parse(app_version)
                .map_err(|_| SpeechError::Invalid("app version".into()))?,
            builtin_key,
            root_key,
            inner: Arc::new(Mutex::new(Inner {
                state,
                pins: HashMap::new(),
            })),
        };
        {
            let g = s.lock();
            s.save(&g.state)?;
            s.sweep(&g);
        }
        Ok(s)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn app_version(&self) -> &semver::Version {
        &self.app_version
    }
    pub fn state(&self) -> State {
        self.lock().state.clone()
    }
    pub fn version_dir(&self, ver: &str) -> PathBuf {
        self.root.join("versions").join(ver)
    }
    pub fn staging_dir(&self, ver: &str) -> PathBuf {
        self.root.join("staging").join(ver)
    }
    pub fn active_sessions(&self) -> usize {
        self.lock().total_pins()
    }

    pub fn key(&self) -> PinnedKey {
        let g = self.lock();
        match (&g.state.pinned_key_id, &g.state.pinned_key) {
            // Validated at load (N67): a recorded rotation never falls back to the built-in key.
            (Some(id), Some(k)) => PinnedKey::from_hex(id, k).expect("validated pinned key"),
            _ => self.builtin_key.clone(),
        }
    }

    fn save(&self, st: &State) -> Result<(), SpeechError> {
        let b = serde_json::to_vec_pretty(st).expect("state json");
        fsx::write_private_atomic(&self.root.join("current.json"), &b)?;
        fsx::write_private_atomic(&self.root.join("current.json.bak"), &b)?;
        Ok(())
    }

    /// Remove installed versions that are neither current, previous, pinned nor the waiting candidate, and any
    /// stray candidate dirs (N74: under the lock, pinned versions are skipped).
    fn sweep(&self, g: &Inner) {
        let st = &g.state;
        let Ok(rd) = std::fs::read_dir(self.root.join("versions")) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let keep = Some(&name) == st.current.as_ref()
                || Some(&name) == st.previous.as_ref()
                || g.pins.contains_key(&name)
                || st.waiting.as_ref().is_some_and(|w| w.dir == name);
            if !keep {
                let _ = fsx::remove_tree(&e.path());
            }
        }
    }

    fn eff_now(st: &mut State, now_ms: u64) -> (u64, bool) {
        if now_ms > st.clock_hwm {
            st.clock_hwm = now_ms;
            (now_ms, true)
        } else {
            (st.clock_hwm, false)
        }
    }

    fn revocation_of(st: &State, eff: u64) -> Revocation {
        let age_ms = eff.saturating_sub(st.denylist_issued);
        Revocation {
            issued: st.denylist_issued,
            expires: st.denylist_expires,
            age_ms,
            expired: st.denylist_expires <= eff,
            stale: age_ms > REVOCATION_HARD_CAP_MS,
        }
    }

    /// max(now, persisted high-water mark); raises and persists the mark when the clock moved forward.
    pub fn effective_now(&self, now_ms: u64) -> u64 {
        let mut g = self.lock();
        let (eff, moved) = Self::eff_now(&mut g.state, now_ms);
        if moved {
            let _ = self.save(&g.state);
        }
        eff
    }

    pub fn revocation(&self, now_ms: u64) -> Revocation {
        let eff = self.effective_now(now_ms);
        Self::revocation_of(&self.lock().state, eff)
    }

    /// What [`Self::pin_session`] would say, without pinning or moving anything.
    pub fn availability(&self, now_ms: u64) -> SpeechAvailability {
        let g = self.lock();
        let eff = g.state.clock_hwm.max(now_ms);
        let rev = Self::revocation_of(&g.state, eff);
        match g.state.current.clone() {
            None => SpeechAvailability::NotInstalled,
            Some(v) if g.state.denied.contains(&v) => SpeechAvailability::Revoked {
                version: v,
                list_expired: rev.expired,
            },
            Some(_) if rev.stale => SpeechAvailability::Stale {
                days: rev.age_ms / DAY_MS,
            },
            Some(version) => SpeechAvailability::Ready { version },
        }
    }

    pub fn denylist_fresh(&self, now_ms: u64) -> bool {
        !self.revocation(now_ms).expired
    }

    pub fn status(&self, now_ms: u64) -> StoreStatus {
        let revocation = self.revocation(now_ms);
        let g = self.lock();
        StoreStatus {
            current: g.state.current.clone(),
            previous: g.state.previous.clone(),
            bytes: fsx::dir_size(&self.root.join("versions")),
            sessions: g.total_pins(),
            waiting: g.state.waiting.as_ref().map(|w| w.version.clone()),
            revocation,
        }
    }

    /// Anti-rollback, denylist and disk cap for a verified manifest we are about to download.
    pub fn check_candidate(&self, m: &Manifest) -> Result<(), SpeechError> {
        let st = self.state();
        if st.denied.contains(&m.version) {
            return Err(SpeechError::Denied(m.version.clone()));
        }
        if let Some(h) = &st.highest_verified {
            if m.semver() < v(h) {
                return Err(SpeechError::Rollback {
                    offered: m.version.clone(),
                    highest: h.clone(),
                });
            }
        }
        let kept: u64 = [&st.current, &st.previous]
            .into_iter()
            .flatten()
            .map(|x| fsx::dir_size(&self.version_dir(x)))
            .sum();
        if kept + m.total_size() > DISK_CAP_BYTES {
            return Err(SpeechError::DiskCap);
        }
        Ok(())
    }

    pub fn is_quarantined(&self, ver: &str) -> bool {
        self.lock().state.quarantined.iter().any(|q| q == ver)
    }

    /// Create the 0700 staging dir and store the (already verified) manifest bytes there.
    pub fn stage_manifest(&self, m: &Manifest, manifest: &[u8], sig: &[u8]) -> Result<PathBuf, SpeechError> {
        let dir = self.staging_dir(&m.version);
        fsx::mkdir_private(&dir)?;
        fsx::write_private_atomic(&dir.join("manifest.json"), manifest)?;
        fsx::write_private_atomic(&dir.join("manifest.sig"), sig)?;
        Ok(dir)
    }

    fn read_manifest(&self, dir: &Path) -> Result<Manifest, SpeechError> {
        let root = files::open_dir(dir)?;
        let read = |name: &str| -> Result<Vec<u8>, SpeechError> {
            let c = std::ffi::CString::new(name).unwrap();
            use std::os::fd::{AsRawFd, FromRawFd};
            // SAFETY: valid dir fd and name; owned below.
            let fd = unsafe {
                libc::openat(
                    root.as_raw_fd(),
                    c.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(SpeechError::Missing(name.into()));
            }
            // SAFETY: fresh owned fd.
            let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
            let mut out = Vec::new();
            use std::io::Read;
            (&mut f)
                .take(bundle::MAX_META_BYTES as u64 + 1)
                .read_to_end(&mut out)?;
            Ok(out)
        };
        bundle::verify_manifest(
            &self.key(),
            &read("manifest.json")?,
            &read("manifest.sig")?,
            &self.app_version,
        )
    }

    /// Signature, then every listed file opened (no symlinks) and hashed, then nothing unlisted.
    fn verify_tree(&self, dir: &Path) -> Result<Manifest, SpeechError> {
        let m = self.read_manifest(dir)?;
        drop(files::open_module_files(dir, &m)?);
        fn walk(base: &Path, d: &Path, out: &mut Vec<String>) -> Result<(), SpeechError> {
            for e in std::fs::read_dir(d)? {
                let e = e?;
                let p = e.path();
                let ft = e.file_type()?; // does not follow symlinks
                if ft.is_dir() {
                    walk(base, &p, out)?;
                } else if ft.is_file() {
                    out.push(p.strip_prefix(base).unwrap().to_string_lossy().to_string());
                } else {
                    return Err(SpeechError::Invalid("symlink or special file in bundle".into()));
                }
            }
            Ok(())
        }
        let mut all = Vec::new();
        walk(dir, dir, &mut all)?;
        for f in all {
            if f != "manifest.json" && f != "manifest.sig" && m.file(&f).is_none() {
                return Err(SpeechError::Invalid(format!("unlisted file {f}")));
            }
        }
        Ok(m)
    }

    /// Verify the staged bundle and rename it into a fresh candidate dir (read-only). Never touches the live tree
    /// (N65). Needs a fresh revocation list (N71).
    pub fn commit_staged(&self, ver: &str, now_ms: u64) -> Result<Candidate, SpeechError> {
        if !self.denylist_fresh(now_ms) {
            return Err(SpeechError::RevocationExpired);
        }
        let staged = self.staging_dir(ver);
        let m = self.verify_tree(&staged)?;
        if m.version != ver {
            return Err(SpeechError::Invalid("staged version mismatch".into()));
        }
        self.check_candidate(&m)?;
        let name = format!(".cand-{ver}-{}-{}", std::process::id(), now_ms);
        let dir = self.root.join("versions").join(&name);
        std::fs::rename(&staged, &dir)?;
        fsx::make_read_only(&dir, &m.entry)?;
        Ok(Candidate {
            version: ver.into(),
            manifest: m,
            dir,
        })
    }

    pub fn discard(&self, c: Candidate) -> Result<(), SpeechError> {
        let mut g = self.lock();
        if g.state
            .waiting
            .as_ref()
            .is_some_and(|w| self.root.join("versions").join(&w.dir) == c.dir)
        {
            g.state.waiting = None;
            self.save(&g.state)?;
        }
        fsx::remove_tree(&c.dir)?;
        Ok(())
    }

    /// Put a health-checked candidate live. A re-download of the current version swaps the trees atomically
    /// (exchange), so there is never a moment without a valid current tree (N65). `Busy` while a session runs:
    /// the candidate is kept as `waiting` and `activate` finishes it once idle.
    pub fn promote(&self, c: Candidate, now_ms: u64) -> Result<(), SpeechError> {
        let mut g = self.lock();
        let (eff, _) = Self::eff_now(&mut g.state, now_ms);
        if Self::revocation_of(&g.state, eff).expired {
            return Err(SpeechError::RevocationExpired);
        }
        if g.state.denied.contains(&c.version) {
            drop(g);
            let _ = fsx::remove_tree(&c.dir);
            return Err(SpeechError::Denied(c.version));
        }
        if g.total_pins() > 0 {
            g.state.waiting = Some(Waiting {
                version: c.version.clone(),
                dir: c.dir.file_name().unwrap().to_string_lossy().to_string(),
            });
            self.save(&g.state)?;
            return Err(SpeechError::Busy);
        }
        self.promote_locked(&mut g, &c.version, &c.dir)
    }

    fn promote_locked(&self, g: &mut Inner, ver: &str, cand: &Path) -> Result<(), SpeechError> {
        let live = self.version_dir(ver);
        if live.exists() {
            match fsx::exchange(cand, &live) {
                Ok(()) => {
                    let _ = fsx::remove_tree(cand);
                }
                Err(_) => {
                    // Recoverable two-step: the old tree is kept until the new one is in place.
                    let old = self.root.join("versions").join(format!(".old-{ver}"));
                    let _ = fsx::remove_tree(&old);
                    std::fs::rename(&live, &old)?;
                    std::fs::rename(cand, &live)?;
                    let _ = fsx::remove_tree(&old);
                }
            }
        } else {
            std::fs::rename(cand, &live)?;
        }
        let st = &mut g.state;
        if st.current.as_deref() != Some(ver) {
            st.previous = st.current.take();
            st.current = Some(ver.to_owned());
        }
        if st.highest_verified.as_deref().is_none_or(|h| v(ver) > v(h)) {
            st.highest_verified = Some(ver.to_owned());
        }
        st.quarantined.retain(|q| q != ver);
        st.failures.clear();
        st.waiting = None;
        st.pending_rollback = None;
        self.save(&g.state)?;
        self.sweep(g);
        Ok(())
    }

    /// Finish a `waiting` candidate once idle (or re-point to an installed version).
    pub fn activate(&self, ver: &str, now_ms: u64) -> Result<(), SpeechError> {
        let mut g = self.lock();
        if g.total_pins() > 0 {
            return Err(SpeechError::Busy);
        }
        let (eff, _) = Self::eff_now(&mut g.state, now_ms);
        if Self::revocation_of(&g.state, eff).expired {
            return Err(SpeechError::RevocationExpired);
        }
        if g.state.denied.iter().any(|d| d == ver) {
            return Err(SpeechError::Denied(ver.into()));
        }
        match g.state.waiting.clone() {
            Some(w) if w.version == ver => {
                let cand = self.root.join("versions").join(&w.dir);
                self.promote_locked(&mut g, ver, &cand)
            }
            _ if self.version_dir(ver).exists() && g.state.current.as_deref() == Some(ver) => Ok(()),
            _ => Err(SpeechError::NotInstalled),
        }
    }

    /// Pin the current version for one session. A pending (or due) rollback runs first when idle (N66). Refused
    /// for a version any list ever denied and past the 90-day cap (N70).
    pub fn pin_session(&self, now_ms: u64) -> Result<SessionPin, SpeechError> {
        let mut g = self.lock();
        let (eff, moved) = Self::eff_now(&mut g.state, now_ms);
        if moved {
            self.save(&g.state)?;
        }
        if g.total_pins() == 0 {
            let cur_denied = g
                .state
                .current
                .as_ref()
                .is_some_and(|c| g.state.denied.contains(c));
            let kind = g
                .state
                .pending_rollback
                .or(cur_denied.then_some(RollbackKind::Revoked));
            if let Some(kind) = kind {
                match self.rollback_locked(&mut g, kind, eff) {
                    Ok(_) | Err(SpeechError::RevocationExpired) | Err(SpeechError::NotInstalled) => {}
                    Err(e) => return Err(e),
                }
            }
            if let Some(w) = g.state.waiting.clone() {
                if !Self::revocation_of(&g.state, eff).expired && !g.state.denied.contains(&w.version) {
                    let cand = self.root.join("versions").join(&w.dir);
                    self.promote_locked(&mut g, &w.version, &cand)?;
                }
            }
        }
        let rev = Self::revocation_of(&g.state, eff);
        let ver = g.state.current.clone().ok_or(SpeechError::NotInstalled)?;
        if g.state.denied.contains(&ver) {
            return Err(SpeechError::Denied(ver));
        }
        if rev.stale {
            return Err(SpeechError::Stale {
                days: rev.age_ms / DAY_MS,
            });
        }
        *g.pins.entry(ver.clone()).or_default() += 1;
        drop(g);
        let dir = self.version_dir(&ver);
        // The pin is held from here; its Drop releases it on any error below.
        let mut pin = SessionPin {
            version: ver,
            manifest: Manifest::default(),
            dir,
            inner: self.inner.clone(),
        };
        pin.manifest = self.read_manifest(&pin.dir)?;
        Ok(pin)
    }

    fn rollback_locked(&self, g: &mut Inner, kind: RollbackKind, eff: u64) -> Result<Rollback, SpeechError> {
        if g.total_pins() > 0 {
            return Err(SpeechError::Busy);
        }
        if kind != RollbackKind::CrashLoop && Self::revocation_of(&g.state, eff).expired {
            return Err(SpeechError::RevocationExpired);
        }
        let st = &mut g.state;
        let from = st.current.clone().ok_or(SpeechError::NotInstalled)?;
        let clean_prev = st
            .previous
            .clone()
            .filter(|p| !st.denied.contains(p) && self.version_dir(p).exists());
        if kind == RollbackKind::CrashLoop && clean_prev.is_none() {
            return Err(SpeechError::NotInstalled);
        }
        st.previous = None;
        st.current = clean_prev.clone();
        st.failures.clear();
        st.pending_rollback = None;
        if kind == RollbackKind::CrashLoop && !st.quarantined.contains(&from) {
            st.quarantined.push(from.clone());
        }
        self.save(&g.state)?;
        self.sweep(g);
        Ok(Rollback {
            from,
            to: clean_prev,
            kind,
        })
    }

    /// Back to the local N-1 (kept, verified, clean on every list seen). The rolled-back version is deleted.
    pub fn rollback(&self, kind: RollbackKind, now_ms: u64) -> Result<Rollback, SpeechError> {
        let mut g = self.lock();
        let (eff, _) = Self::eff_now(&mut g.state, now_ms);
        self.rollback_locked(&mut g, kind, eff)
    }

    pub fn record_success(&self) {
        let mut g = self.lock();
        if !g.state.failures.is_empty() {
            g.state.failures.clear();
            let _ = self.save(&g.state);
        }
    }

    /// A failed session on `current`. Three in five minutes roll back to a clean N-1 (now if idle, else at the
    /// next pin: N66) and quarantine the looping version (N64).
    pub fn record_failure(&self, now_ms: u64) -> Result<Option<Rollback>, SpeechError> {
        let mut g = self.lock();
        let (eff, _) = Self::eff_now(&mut g.state, now_ms);
        let st = &mut g.state;
        st.failures
            .retain(|t| now_ms.saturating_sub(*t) < CRASH_LOOP_WINDOW_MS);
        st.failures.push(now_ms);
        let looped = st.failures.len() >= CRASH_LOOP_FAILURES
            && st.previous.as_ref().is_some_and(|p| !st.denied.contains(p));
        if looped && g.total_pins() > 0 {
            g.state.pending_rollback = Some(RollbackKind::CrashLoop);
        }
        self.save(&g.state)?;
        if looped && g.total_pins() == 0 {
            return self
                .rollback_locked(&mut g, RollbackKind::CrashLoop, eff)
                .map(Some);
        }
        Ok(None)
    }

    pub fn apply_rotation(&self, bytes: &[u8], sigs: &[u8], now_ms: u64) -> Result<PinnedKey, SpeechError> {
        let pinned = self.key();
        let mut g = self.lock();
        let (eff, _) = Self::eff_now(&mut g.state, now_ms);
        let (k, r) =
            bundle::verify_rotation(&self.root_key, &pinned, g.state.rotation_seq, eff, bytes, sigs)?;
        g.state.pinned_key_id = Some(k.key_id.clone());
        g.state.pinned_key = Some(k.hex());
        g.state.rotation_seq = r.seq;
        self.save(&g.state)?;
        Ok(k)
    }

    /// Apply a signed denylist. Seq is per signing key (N63). A newer list applies its denials even if it has
    /// expired (condition 4, N61); its validity is clamped to `issued + 8 days` (N63) and only that gates installs.
    /// A denied current version rolls back to a clean N-1 when idle and the list is fresh, else at the next pin.
    pub fn apply_denylist(
        &self,
        bytes: &[u8],
        sig: &[u8],
        now_ms: u64,
    ) -> Result<Option<Rollback>, SpeechError> {
        let key = self.key();
        let d = bundle::verify_denylist(&key, bytes, sig)?;
        let mut g = self.lock();
        let (eff, _) = Self::eff_now(&mut g.state, now_ms);
        let st = &mut g.state;
        let last = if st.denylist_key_id.as_deref() == Some(key.key_id.as_str()) {
            st.denylist_seq
        } else {
            0
        };
        if d.seq <= last {
            return Ok(None);
        }
        if d.issued > eff + MAX_LIST_VALIDITY_MS {
            return Err(SpeechError::Signature("denylist issued in the future".into()));
        }
        st.denylist_key_id = Some(key.key_id.clone());
        st.denylist_seq = d.seq;
        st.denylist_issued = d.issued;
        st.denylist_expires = d.expires.min(d.issued.saturating_add(MAX_LIST_VALIDITY_MS));
        st.clock_hwm = st.clock_hwm.max(d.issued);
        for x in d.versions {
            if !st.denied.contains(&x) {
                st.denied.push(x);
            }
        }
        if st.previous.as_ref().is_some_and(|p| st.denied.contains(p)) {
            st.previous = None;
        }
        let cur_denied = st.current.as_ref().is_some_and(|c| st.denied.contains(c));
        if cur_denied && g.total_pins() > 0 {
            g.state.pending_rollback = Some(RollbackKind::Revoked);
        }
        self.save(&g.state)?;
        if cur_denied && g.total_pins() == 0 {
            let eff = g.state.clock_hwm.max(eff);
            return match self.rollback_locked(&mut g, RollbackKind::Revoked, eff) {
                Ok(rb) => Ok(Some(rb)),
                Err(SpeechError::RevocationExpired) => Ok(None),
                Err(e) => Err(e),
            };
        }
        self.sweep(&g);
        Ok(None)
    }

    /// Settings "delete": remove every installed version and staged download. Anti-rollback state, denials,
    /// quarantine and the pinned key survive.
    pub fn delete_all(&self) -> Result<(), SpeechError> {
        let mut g = self.lock();
        if g.total_pins() > 0 {
            return Err(SpeechError::Busy);
        }
        g.state.current = None;
        g.state.previous = None;
        g.state.failures.clear();
        g.state.waiting = None;
        g.state.pending_rollback = None;
        self.save(&g.state)?;
        fsx::remove_tree(&self.root.join("versions"))?;
        fsx::remove_tree(&self.root.join("staging"))?;
        fsx::mkdir_private(&self.root.join("versions"))?;
        fsx::mkdir_private(&self.root.join("staging"))?;
        Ok(())
    }
}
