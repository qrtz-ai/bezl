//! qrtz speech (ADR 0046). The host side of the confined speech module: protocol + untrusted-output validation,
//! the model-agnostic `SpeechEngine`, and the signed-bundle store/updater.
pub mod bundle;
pub mod confine;
#[cfg(feature = "net")]
pub mod download;
pub mod files;
pub mod fsx;
pub mod spawn;
pub mod store;
#[cfg(feature = "testkit")]
pub mod testkit;
#[cfg(feature = "net")]
pub mod update;

// ADR 0146 M2: the engine seam, the protocol, the validator and SpeechError live in `qrtz-speech-types` (shared
// with echo's and nano's crates); re-exported here so every `qrtz_speech::…` path keeps working.
pub use engine::{
    Channel, EngineInfo, EngineKind, FailReason, SessionKind, SessionSpec, SpeakerId, SpeakerMode,
    SpeechEngine, SpeechEvent, SpeechSession, StatusError,
};
pub use qrtz_speech_types::{engine, protocol, validate, SpeechError};
pub use store::{
    choose_engine, EngineChoice, Revocation, RollbackKind, SpeechAvailability, SpeechStore, StoreStatus,
};
