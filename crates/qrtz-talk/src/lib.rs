//! qrtz talk for the Tauri build: push-to-talk dictation (the port of Electron's `TalkService`).
//!
//! The helper owns the mic and feeds the on-device qrtz speech module behind `qrtz_speech_types::SpeechEngine`
//! (`dictation.start/stop/cancel`, busy ids, warm mic, mux with a recording). Talk is entirely offline: there is no
//! cloud engine (ADR 0076), so a missing, revoked or stale module means talk is unavailable. History and dictionary
//! live in `talk.json`, the same file and format as Electron (owner-only, atomic). ADR 0058 (on ADR 0046); no LLM
//! cleanup (ADR 0071).
#![forbid(unsafe_code)]
pub mod bench;
pub mod cleanup;
pub mod config;
pub mod contract;
pub mod level;
pub mod mock;
mod poison;
pub mod service;
pub mod speech;
pub mod store;

pub use config::TalkConfig;
pub use contract::{
    DictionaryEntry, Engine, InsertMethod, Latency, TalkEntry, TalkErrorKind, TalkPhase, TalkView,
};
/// The shared speech seam talk consumes (crates/qrtz-speech-types, ADR 0046 / ADR 0146 M2), under its old path.
pub use qrtz_speech_types as qrtz_speech;
pub use service::{
    nothing_heard, AsrInfo, Clipboard, Clock, HelperSink, HelperTail, IdSource, RandomIds, SystemClock,
    TalkDeps, TalkEvents, TalkService, Timeouts, INSERT_REFUSED_SECURE, MAX_SECURE_REFUSED, MSG_LATE_FINAL,
    MSG_MIC_FAILED, MSG_NOTHING_HEARD, MSG_SECURE_FIELD, MSG_SILENT_MIC, MSG_SPEECH_MISSING,
    SEGMENT_OVERLAP_MS,
};
pub use speech::{HelperAsrSpeech, TalkValidator};
pub use store::{FsIo, StoreIo};
