//! Check → download → verify in staging → candidate → health check → promote (ADR 0046 §4-5). Never swaps while a
//! session runs: a ready update waits (`Waiting`) and is promoted at the next idle pin or `activate`.
use crate::bundle::{self, Manifest};
use crate::download::{DownloadControl, Downloader, Progress};
use crate::files::ModuleFiles;
use crate::store::{Rollback, SpeechStore};
use crate::SpeechError;
use async_trait::async_trait;

#[async_trait]
pub trait HealthCheck: Send + Sync {
    /// Spawn the candidate from its opened, hashed files (N73) under full confinement, require the handshake
    /// (version, protocol) and the host-side confinement checks, and the golden clip's words within tolerance.
    async fn check(&self, m: &Manifest, files: ModuleFiles) -> Result<(), String>;
}

#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum UpdateOutcome {
    UpToDate {
        version: String,
    },
    Installed {
        version: String,
        previous: Option<String>,
    },
    /// Verified and health-checked, but a session is running; promoted once idle.
    Waiting {
        version: String,
    },
    /// N64: the latest release is the version that crash-looped here; skipped until a newer one or a re-download.
    Quarantined {
        version: String,
    },
    RolledBack(Rollback),
}

pub struct Updater<'a> {
    pub store: &'a SpeechStore,
    pub dl: &'a Downloader,
    pub health: &'a dyn HealthCheck,
    /// Wall clock in ms (expiry checks).
    pub now_ms: &'a (dyn Fn() -> u64 + Send + Sync),
}

impl Updater<'_> {
    /// Apply a key rotation and a denylist if the server has newer ones.
    pub async fn refresh_trust(&self) -> Result<Option<Rollback>, SpeechError> {
        if let (Some(b), Some(s)) = (
            self.dl.fetch_meta("key-rotation.json").await?,
            self.dl.fetch_meta("key-rotation.sigs.json").await?,
        ) {
            // Already applied, replayed or not for us: ignored (the pinned key stays).
            if let Err(e) = self.store.apply_rotation(&b, &s, (self.now_ms)()) {
                tracing::debug!("speech key rotation not applied: {e}");
            }
        }
        if let (Some(b), Some(s)) = (
            self.dl.fetch_meta("denylist.json").await?,
            self.dl.fetch_meta("denylist.json.sig").await?,
        ) {
            return self.store.apply_denylist(&b, &s, (self.now_ms)());
        }
        Ok(None)
    }

    pub async fn latest(&self) -> Result<(Manifest, Vec<u8>, Vec<u8>), SpeechError> {
        let mb = self
            .dl
            .fetch_meta("latest/manifest.json")
            .await?
            .ok_or_else(|| SpeechError::Network("no speech release".into()))?;
        let sb = self
            .dl
            .fetch_meta("latest/manifest.sig")
            .await?
            .ok_or_else(|| SpeechError::Signature("missing signature".into()))?;
        let m = bundle::verify_manifest(&self.store.key(), &mb, &sb, self.store.app_version())?;
        Ok((m, mb, sb))
    }

    /// `force` = the user's "re-download" (also overrides a quarantine).
    pub async fn install_latest(
        &self,
        force: bool,
        ctl: &DownloadControl,
        on_progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<UpdateOutcome, SpeechError> {
        if let Some(rb) = self.refresh_trust().await? {
            if rb.to.is_some() {
                return Ok(UpdateOutcome::RolledBack(rb));
            }
        }
        // Condition 3: no install, update or re-download on an expired list.
        if !self.store.denylist_fresh((self.now_ms)()) {
            return Err(SpeechError::RevocationExpired);
        }
        let (m, mb, sb) = self.latest().await?;
        let st = self.store.state();
        if !force && st.current.as_deref() == Some(m.version.as_str()) {
            return Ok(UpdateOutcome::UpToDate { version: m.version });
        }
        if !force && self.store.is_quarantined(&m.version) {
            return Ok(UpdateOutcome::Quarantined { version: m.version });
        }
        self.store.check_candidate(&m)?;
        let staging = self.store.stage_manifest(&m, &mb, &sb)?;
        self.dl.download_bundle(&m, &staging, ctl, on_progress).await?;
        let cand = self.store.commit_staged(&m.version, (self.now_ms)())?;
        let files = match cand.open_files() {
            Ok(f) => f,
            Err(e) => {
                self.store.discard(cand)?;
                return Err(e);
            }
        };
        if let Err(e) = self.health.check(&cand.manifest, files).await {
            self.store.discard(cand)?;
            return Err(SpeechError::Health(e));
        }
        let version = cand.version.clone();
        match self.store.promote(cand, (self.now_ms)()) {
            Ok(()) => Ok(UpdateOutcome::Installed {
                version,
                previous: self.store.state().previous,
            }),
            Err(SpeechError::Busy) => Ok(UpdateOutcome::Waiting { version }),
            Err(e) => Err(e),
        }
    }
}
