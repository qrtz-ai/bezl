//! qrtz speech types (ADR 0046, ADR 0146 M2): what echo's and nano's crates share with the shell's speech manager
//! (`qrtz-speech`): the `SpeechEngine` seam and its events, the speech protocol with its untrusted-output validator,
//! and `SpeechError`. `qrtz-speech` re-exports all of it, so its users see no change.
#![forbid(unsafe_code)]
pub mod engine;
pub mod protocol;
pub mod validate;

pub use engine::{
    Channel, EngineInfo, EngineKind, FailReason, SessionKind, SessionSpec, SpeakerId, SpeakerMode,
    SpeechEngine, SpeechEvent, SpeechSession, StatusError,
};

#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    #[error("speech is not downloaded")]
    NotInstalled,
    #[error("signature: {0}")]
    Signature(String),
    #[error("invalid bundle: {0}")]
    Invalid(String),
    #[error("incompatible: {0}")]
    Incompatible(String),
    #[error("checksum mismatch: {0}")]
    Checksum(String),
    #[error("missing file: {0}")]
    Missing(String),
    #[error("bundle too large")]
    TooLarge,
    #[error("speech disk cap reached")]
    DiskCap,
    #[error("refused: {offered} is older than the installed {highest}")]
    Rollback { offered: String, highest: String },
    #[error("version {0} was revoked")]
    Denied(String),
    #[error("the revocation list has expired; connect to update speech")]
    RevocationExpired,
    #[error("speech has not checked for revocations in {days} days; connect to keep using it")]
    Stale { days: u64 },
    #[error("speech cannot be confined here: {0}")]
    Unconfined(String),
    #[error("speech state is corrupt and has no good copy")]
    StateCorrupt,
    #[error("a speech session is running")]
    Busy,
    /// The package manager's install lock stayed taken (another install, update or index refresh) for as long as a
    /// model download waits for it (N189, ADR 0213). Nothing was installed or changed; ask again.
    #[error("another download or update is running")]
    InstallBusy,
    #[error("download paused")]
    Paused,
    #[error("network: {0}")]
    Network(String),
    #[error("health check failed: {0}")]
    Health(String),
    #[error("module: {0}")]
    Module(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl SpeechError {
    /// Worth a "retry" button (vs. a hard refusal).
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            SpeechError::Network(_)
                | SpeechError::Checksum(_)
                | SpeechError::Paused
                | SpeechError::RevocationExpired
                | SpeechError::Stale { .. }
                | SpeechError::Io(_)
                | SpeechError::Busy
                | SpeechError::InstallBusy
        )
    }
}
