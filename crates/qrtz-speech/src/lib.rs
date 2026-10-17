//! qrtz speech (ADR 0046). The host side of the confined speech module: protocol + untrusted-output validation,
//! the model-agnostic `SpeechEngine`, and the signed-bundle store/updater.
pub mod bundle;
pub mod download;
pub mod engine;
pub mod fsx;
pub mod protocol;
pub mod store;
pub mod update;
pub mod validate;

pub use engine::{Channel, SessionKind, SessionSpec, SpeakerMode, SpeechEngine, SpeechEvent, SpeechSession};
pub use store::{SpeechStore, StoreStatus};

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
    #[error("a speech session is running")]
    Busy,
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
                | SpeechError::Io(_)
                | SpeechError::Busy
        )
    }
}
