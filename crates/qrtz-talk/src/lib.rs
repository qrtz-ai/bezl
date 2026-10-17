//! qrtz talk for the Tauri build: push-to-talk dictation (the port of Electron's `TalkService`).
//!
//! The helper owns the mic and feeds the default, on-device qrtz speech module behind `qrtz_speech::SpeechEngine` (`dictation.start/stop/cancel`,
//! busy ids, warm mic, mux with a recording), so dictation works offline. An opt-in cloud provider behind
//! [`provider::StreamingProvider`] (ElevenLabs Scribe v2 Realtime; Deepgram for parity) streams alongside when
//! picked; [`arbiter::Arbiter`] fails over to the on-device text, or, with auto-failover off, the dictation ends
//! with [`TalkErrorKind::Offline`]. History and dictionary live in `talk.json`, the same file and format as
//! Electron (owner-only, atomic). ADR 0058 (on ADR 0046).
pub mod arbiter;
pub mod cleanup;
pub mod config;
pub mod contract;
pub mod mock;
pub mod provider;
pub mod service;
pub mod session;
pub mod speech;
pub mod store;
pub mod ws;

pub use cleanup::{ChatMessage, CleanupLlm};
pub use config::TalkConfig;
pub use contract::{
    DictionaryEntry, Engine, InsertMethod, Latency, TalkEntry, TalkErrorKind, TalkPhase, TalkView,
};
pub use provider::StreamingProvider;
/// The shared speech seam talk consumes (crates/qrtz-speech, ADR 0046).
pub use qrtz_speech;
pub use service::{
    CleanupLlmSource, Clipboard, Clock, HelperSink, IdSource, NoLlm, Providers, RandomIds, SecretSource,
    SystemClock, TalkDeps, TalkEvents, TalkService, Timeouts,
};
pub use speech::{CloudSpeech, HelperAsrSpeech, TalkValidator};
pub use store::{FsIo, StoreIo};
