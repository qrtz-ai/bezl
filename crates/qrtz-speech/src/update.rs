//! Check → download → verify in staging → rename → health check → activate (ADR 0046 §4-5). Never swaps while a
//! session runs: a ready update waits (`Busy`) and the caller retries when idle.
use crate::bundle::{self, Manifest};
use crate::download::{DownloadControl, Downloader, Progress};
use crate::store::{Rollback, SpeechStore};
use crate::SpeechError;
use async_trait::async_trait;
use std::path::Path;

#[async_trait]
pub trait HealthCheck: Send + Sync {
    /// Spawn the candidate from `dir`, require the handshake (version, protocol, probes refused) and the golden
    /// clip's words within tolerance.
    async fn check(&self, dir: &Path, m: &Manifest) -> Result<(), String>;
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
    /// Verified and committed, but a session is running; `activate` once idle.
    Waiting {
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

    /// `force` re-downloads the current version (settings "re-download").
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
        if !self.store.denylist_fresh((self.now_ms)()) {
            // Condition 3: no install, update or re-download on an expired list.
            return Err(SpeechError::RevocationExpired);
        }
        let (m, mb, sb) = self.latest().await?;
        let st = self.store.state();
        if !force && st.current.as_deref() == Some(m.version.as_str()) {
            return Ok(UpdateOutcome::UpToDate { version: m.version });
        }
        self.store.check_candidate(&m)?;
        let staging = self.store.stage_manifest(&m, &mb, &sb)?;
        self.dl.download_bundle(&m, &staging, ctl, on_progress).await?;
        let m = self.store.commit_staged(&m.version)?;
        let dir = self.store.version_dir(&m.version);
        if let Err(e) = self.health.check(&dir, &m).await {
            if st.current.as_deref() != Some(m.version.as_str()) {
                self.store.discard(&m.version)?;
            }
            return Err(SpeechError::Health(e));
        }
        match self.store.activate(&m.version, (self.now_ms)()) {
            Ok(()) => Ok(UpdateOutcome::Installed {
                version: m.version,
                previous: self.store.state().previous,
            }),
            Err(SpeechError::Busy) => Ok(UpdateOutcome::Waiting { version: m.version }),
            Err(e) => Err(e),
        }
    }
}
