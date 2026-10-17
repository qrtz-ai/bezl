//! qrtz-app: the UI-free service behind the Tauri shell. See docs/tauri-migration.md.
pub mod config;
pub mod ports;
#[cfg(feature = "runtime")]
pub mod runtime_port;
pub mod secrets;
pub mod service;

pub use config::ConfigStore;
pub use service::{is_talk_message, AppDeps, AppInfo, AppService, Events, Shared};
