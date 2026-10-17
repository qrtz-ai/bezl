//! The on-disk speech module store (ADR 0046 §4-5):
//!
//! ```text
//! <root>/ 0700
//!   current.json      0600  pointer + anti-rollback state (temp + fsync + rename)
//!   staging/<v>/      0700  downloads (`.part` files 0600), verified here, then rename(2)
//!   versions/<v>/     0500  installed, read-only (files 0400, entry 0500); only N and N-1 are kept
//! ```
use crate::bundle::{self, Manifest, PinnedKey};
use crate::fsx;
use crate::SpeechError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Speech dir disk cap (N + N-1 + one staging bundle).
pub const DISK_CAP_BYTES: u64 = 3 * 1024 * 1024 * 1024;
pub const CRASH_LOOP_FAILURES: usize = 3;
pub const CRASH_LOOP_WINDOW_MS: u64 = 5 * 60 * 1000;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub current: Option<String>,
    pub previous: Option<String>,
    /// Monotonic: the highest version ever verified and installed. Never lowered (not even by delete).
    pub highest_verified: Option<String>,
    /// Pinned module key after a rotation (else the key built into the app).
    pub pinned_key_id: Option<String>,
    pub pinned_key: Option<String>,
    pub denylist_seq: u64,
    /// Expiry (ms) of the newest applied denylist; installs need it in the future (N60).
    pub denylist_expires: u64,
    pub denied: Vec<String>,
    pub rotation_seq: u64,
    /// Versions whose files are hashed again on their first load (after an install or update).
    pub needs_rehash: Vec<String>,
    /// Failure times (ms) of sessions on `current`.
    pub failures: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rollback {
    pub from: String,
    pub to: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreStatus {
    pub current: Option<String>,
    pub previous: Option<String>,
    pub bytes: u64,
    /// Opaque engine metadata of `current` (settings only).
    pub engine: serde_json::Value,
    pub sessions: usize,
}

/// A session's hold on one installed version: the pointer cannot move while any pin is alive.
#[derive(Debug)]
pub struct SessionPin {
    pub version: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
    pins: Arc<AtomicUsize>,
}

impl Drop for SessionPin {
    fn drop(&mut self) {
        self.pins.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct SpeechStore {
    root: PathBuf,
    app_version: semver::Version,
    builtin_key: PinnedKey,
    root_key: PinnedKey,
    state: Mutex<State>,
    pins: Arc<AtomicUsize>,
}

fn v(s: &str) -> semver::Version {
    semver::Version::parse(s).unwrap_or(semver::Version::new(0, 0, 0))
}

impl SpeechStore {
    /// `builtin_key`: the module signing key built into the app; `root_key`: the offline speech root key that signs
    /// rotations (N60).
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
        let state = match std::fs::read(root.join("current.json")) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
            Err(_) => State::default(),
        };
        let s = Self {
            root,
            app_version: semver::Version::parse(app_version)
                .map_err(|_| SpeechError::Invalid("app version".into()))?,
            builtin_key,
            root_key,
            state: Mutex::new(state),
            pins: Arc::new(AtomicUsize::new(0)),
        };
        s.sweep();
        Ok(s)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn app_version(&self) -> &semver::Version {
        &self.app_version
    }
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }
    pub fn version_dir(&self, ver: &str) -> PathBuf {
        self.root.join("versions").join(ver)
    }
    pub fn staging_dir(&self, ver: &str) -> PathBuf {
        self.root.join("staging").join(ver)
    }
    pub fn active_sessions(&self) -> usize {
        self.pins.load(Ordering::SeqCst)
    }

    pub fn key(&self) -> PinnedKey {
        let st = self.state.lock().unwrap();
        match (&st.pinned_key_id, &st.pinned_key) {
            (Some(id), Some(k)) => PinnedKey::from_hex(id, k).unwrap_or_else(|_| self.builtin_key.clone()),
            _ => self.builtin_key.clone(),
        }
    }

    fn save(&self, st: &State) -> Result<(), SpeechError> {
        let b = serde_json::to_vec_pretty(st).expect("state json");
        fsx::write_private_atomic(&self.root.join("current.json"), &b)?;
        Ok(())
    }

    /// Drop installed versions that are neither current nor previous (an interrupted promote or prune).
    fn sweep(&self) {
        let st = self.state.lock().unwrap().clone();
        if let Ok(rd) = std::fs::read_dir(self.root.join("versions")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if Some(&name) != st.current.as_ref() && Some(&name) != st.previous.as_ref() {
                    let _ = fsx::remove_tree(&e.path());
                }
            }
        }
    }

    pub fn status(&self) -> StoreStatus {
        let st = self.state();
        let engine = st
            .current
            .as_ref()
            .and_then(|c| std::fs::read(self.version_dir(c).join("manifest.json")).ok())
            .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
            .map(|m| m.engine)
            .unwrap_or(serde_json::Value::Null);
        StoreStatus {
            current: st.current,
            previous: st.previous,
            bytes: fsx::dir_size(&self.root.join("versions")),
            engine,
            sessions: self.active_sessions(),
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
            .filter(|x| **x != m.version)
            .map(|x| fsx::dir_size(&self.version_dir(x)))
            .sum();
        if kept + m.total_size() > DISK_CAP_BYTES {
            return Err(SpeechError::DiskCap);
        }
        Ok(())
    }

    /// Create the 0700 staging dir and store the (already verified) manifest bytes there.
    pub fn stage_manifest(&self, m: &Manifest, manifest: &[u8], sig: &[u8]) -> Result<PathBuf, SpeechError> {
        let dir = self.staging_dir(&m.version);
        fsx::mkdir_private(&dir)?;
        fsx::write_private_atomic(&dir.join("manifest.json"), manifest)?;
        fsx::write_private_atomic(&dir.join("manifest.sig"), sig)?;
        Ok(dir)
    }

    fn verify_tree(&self, dir: &Path) -> Result<Manifest, SpeechError> {
        let mb = std::fs::read(dir.join("manifest.json"))?;
        let sb = std::fs::read(dir.join("manifest.sig"))?;
        let m = bundle::verify_manifest(&self.key(), &mb, &sb, &self.app_version)?;
        for f in &m.files {
            let p = dir.join(&f.path);
            let md = std::fs::symlink_metadata(&p).map_err(|_| SpeechError::Missing(f.path.clone()))?;
            if !md.is_file() {
                return Err(SpeechError::Invalid(format!("{} is not a regular file", f.path)));
            }
            let (n, h) = fsx::sha256_file(&p)?;
            if n != f.size || h != f.sha256 {
                return Err(SpeechError::Checksum(f.path.clone()));
            }
        }
        // Nothing unlisted (besides the manifest and its signature).
        fn walk(base: &Path, d: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else {
                    out.push(p.strip_prefix(base).unwrap().to_string_lossy().to_string());
                }
            }
        }
        let mut all = Vec::new();
        walk(dir, dir, &mut all);
        for f in all {
            if f != "manifest.json" && f != "manifest.sig" && m.file(&f).is_none() {
                return Err(SpeechError::Invalid(format!("unlisted file {f}")));
            }
        }
        Ok(m)
    }

    /// Verify the whole staged bundle (signature, every size and sha256, no extra files), then rename(2) it into
    /// `versions/<v>` and make it read-only. Not live yet: run the health check, then `activate` (or `discard`).
    pub fn commit_staged(&self, ver: &str) -> Result<Manifest, SpeechError> {
        let staged = self.staging_dir(ver);
        let m = self.verify_tree(&staged)?;
        if m.version != ver {
            return Err(SpeechError::Invalid("staged version mismatch".into()));
        }
        self.check_candidate(&m)?;
        let dest = self.version_dir(ver);
        if dest.exists() {
            if self.state().current.as_deref() == Some(ver) && self.active_sessions() > 0 {
                return Err(SpeechError::Busy);
            }
            fsx::remove_tree(&dest)?;
        }
        std::fs::rename(&staged, &dest)?;
        fsx::make_read_only(&dest, &m.entry)?;
        Ok(m)
    }

    pub fn discard(&self, ver: &str) -> Result<(), SpeechError> {
        let st = self.state();
        if st.current.as_deref() == Some(ver) || st.previous.as_deref() == Some(ver) {
            return Ok(());
        }
        fsx::remove_tree(&self.version_dir(ver))?;
        Ok(())
    }

    /// Move the pointer to a committed, health-checked version (never while a session runs). Keeps N and N-1.
    pub fn activate(&self, ver: &str) -> Result<(), SpeechError> {
        if self.active_sessions() > 0 {
            return Err(SpeechError::Busy);
        }
        let mut st = self.state.lock().unwrap();
        if st.current.as_deref() != Some(ver) {
            st.previous = st.current.take();
            st.current = Some(ver.to_owned());
        }
        if st.highest_verified.as_deref().is_none_or(|h| v(ver) > v(h)) {
            st.highest_verified = Some(ver.to_owned());
        }
        if !st.needs_rehash.iter().any(|x| x == ver) {
            st.needs_rehash.push(ver.to_owned());
        }
        st.failures.clear();
        let snap = st.clone();
        drop(st);
        self.save(&snap)?;
        self.sweep();
        Ok(())
    }

    /// Pin the current version for one session. The first load after an install re-hashes every file.
    pub fn pin_session(&self) -> Result<SessionPin, SpeechError> {
        // Pin first so a concurrent activate sees the session.
        self.pins.fetch_add(1, Ordering::SeqCst);
        let guard = PinGuard(&self.pins);
        let st = self.state();
        let ver = st.current.clone().ok_or(SpeechError::NotInstalled)?;
        if st.denied.contains(&ver) {
            return Err(SpeechError::Denied(ver));
        }
        let dir = self.version_dir(&ver);
        let m = if st.needs_rehash.contains(&ver) {
            let m = self.verify_tree(&dir)?;
            let mut s = self.state.lock().unwrap();
            s.needs_rehash.retain(|x| x != &ver);
            let snap = s.clone();
            drop(s);
            self.save(&snap)?;
            m
        } else {
            let mb = std::fs::read(dir.join("manifest.json"))?;
            let sb = std::fs::read(dir.join("manifest.sig"))?;
            bundle::verify_manifest(&self.key(), &mb, &sb, &self.app_version)?
        };
        std::mem::forget(guard);
        Ok(SessionPin {
            version: ver,
            dir,
            manifest: m,
            pins: self.pins.clone(),
        })
    }

    /// Back to the local N-1 (kept and already verified). The failed version is deleted.
    pub fn rollback(&self, reason: &str) -> Result<Rollback, SpeechError> {
        if self.active_sessions() > 0 {
            return Err(SpeechError::Busy);
        }
        let mut st = self.state.lock().unwrap();
        let from = st.current.clone().ok_or(SpeechError::NotInstalled)?;
        let to = st
            .previous
            .take()
            .filter(|p| !st.denied.contains(p) && self.version_dir(p).exists());
        st.current = to.clone();
        st.failures.clear();
        if let Some(t) = &to {
            if !st.needs_rehash.contains(t) {
                st.needs_rehash.push(t.clone());
            }
        }
        let snap = st.clone();
        drop(st);
        self.save(&snap)?;
        self.sweep();
        Ok(Rollback {
            from,
            to,
            reason: reason.into(),
        })
    }

    pub fn record_success(&self) {
        let mut st = self.state.lock().unwrap();
        if !st.failures.is_empty() {
            st.failures.clear();
            let snap = st.clone();
            drop(st);
            let _ = self.save(&snap);
        }
    }

    /// A failed session on `current`. Three in five minutes roll back to N-1 (once no session runs).
    pub fn record_failure(&self, now_ms: u64) -> Result<Option<Rollback>, SpeechError> {
        let mut st = self.state.lock().unwrap();
        st.failures
            .retain(|t| now_ms.saturating_sub(*t) < CRASH_LOOP_WINDOW_MS);
        st.failures.push(now_ms);
        let looped = st.failures.len() >= CRASH_LOOP_FAILURES && st.previous.is_some();
        let snap = st.clone();
        drop(st);
        self.save(&snap)?;
        if looped && self.active_sessions() == 0 {
            return self.rollback("crash loop").map(Some);
        }
        Ok(None)
    }

    pub fn apply_rotation(&self, bytes: &[u8], sigs: &[u8], now_ms: u64) -> Result<PinnedKey, SpeechError> {
        let last = self.state().rotation_seq;
        let (k, r) = bundle::verify_rotation(&self.root_key, &self.key(), last, now_ms, bytes, sigs)?;
        let mut st = self.state.lock().unwrap();
        st.pinned_key_id = Some(k.key_id.clone());
        st.pinned_key = Some(k.hex());
        st.rotation_seq = r.seq;
        let snap = st.clone();
        drop(st);
        self.save(&snap)?;
        Ok(k)
    }

    /// N60: installs and updates need an unexpired denylist (sessions keep running without one).
    pub fn denylist_fresh(&self, now_ms: u64) -> bool {
        self.state().denylist_expires > now_ms
    }

    /// Apply a signed denylist (newer seq only). A denied current version rolls back to an allowed N-1, or leaves
    /// speech uninstalled ("speech needs an update").
    pub fn apply_denylist(
        &self,
        bytes: &[u8],
        sig: &[u8],
        now_ms: u64,
    ) -> Result<Option<Rollback>, SpeechError> {
        let d = bundle::verify_denylist(&self.key(), bytes, sig)?;
        let mut st = self.state.lock().unwrap();
        // A replay (equal or older seq) or an expired document changes nothing.
        if d.seq <= st.denylist_seq || d.expires <= now_ms {
            return Ok(None);
        }
        st.denylist_seq = d.seq;
        st.denylist_expires = d.expires;
        for x in d.versions {
            if !st.denied.contains(&x) {
                st.denied.push(x);
            }
        }
        if st.previous.as_ref().is_some_and(|p| st.denied.contains(p)) {
            st.previous = None;
        }
        let cur_denied = st.current.as_ref().is_some_and(|c| st.denied.contains(c));
        let snap = st.clone();
        drop(st);
        self.save(&snap)?;
        if cur_denied && self.active_sessions() == 0 {
            return self.rollback("revoked").map(Some);
        }
        self.sweep();
        Ok(None)
    }

    /// Settings "delete": remove every installed version and staged download. Anti-rollback state and the pinned
    /// key survive.
    pub fn delete_all(&self) -> Result<(), SpeechError> {
        if self.active_sessions() > 0 {
            return Err(SpeechError::Busy);
        }
        let mut st = self.state.lock().unwrap();
        st.current = None;
        st.previous = None;
        st.failures.clear();
        st.needs_rehash.clear();
        let snap = st.clone();
        drop(st);
        self.save(&snap)?;
        fsx::remove_tree(&self.root.join("versions"))?;
        fsx::remove_tree(&self.root.join("staging"))?;
        fsx::mkdir_private(&self.root.join("versions"))?;
        fsx::mkdir_private(&self.root.join("staging"))?;
        Ok(())
    }
}

struct PinGuard<'a>(&'a AtomicUsize);
impl Drop for PinGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
