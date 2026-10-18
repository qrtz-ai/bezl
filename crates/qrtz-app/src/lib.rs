//! qrtz-app: the UI-free service behind the Tauri shell. See docs/tauri-migration.md.
#[cfg(feature = "runtime")]
pub mod config;
pub mod ctrl_helper;
#[cfg(feature = "runtime")]
pub mod ctrl_model;
pub mod dirs;
#[cfg(feature = "runtime")]
pub mod local_models;
pub mod logging;
pub mod module_helpers;
pub mod modules;
#[cfg(feature = "runtime")]
pub mod pill;
pub mod ports;
#[cfg(feature = "runtime")]
pub mod runtime_port;
pub mod secrets;
pub mod service;
#[cfg(feature = "runtime")]
pub mod setup;
#[cfg(feature = "runtime")]
pub mod speech;
#[cfg(feature = "runtime")]
pub mod storage_port;
#[cfg(feature = "runtime")]
pub mod summary;
#[cfg(feature = "runtime")]
pub mod talk;
#[cfg(feature = "runtime")]
pub mod wiring;

pub use config::ConfigStore;
pub use dirs::{private_dir, AppDirs};
pub use service::{is_talk_message, AppDeps, AppInfo, AppService, Events, Shared, SharedConfig};
