//! qrtz-app: the UI-free service behind the Tauri shell. See docs/tauri-migration.md.

/// The repository's THIRD_PARTY_NOTICES.md, built in so settings → "open-source licenses" always has the notices of
/// the build that runs (audit P0-4). The bundle also carries the file in Contents/Resources (tauri.conf.json).
pub const THIRD_PARTY_NOTICES: &str = include_str!("../../../THIRD_PARTY_NOTICES.md");
#[cfg(feature = "runtime")]
pub mod app_update;
#[cfg(feature = "runtime")]
pub mod config;
pub mod ctrl_helper;
#[cfg(feature = "runtime")]
pub mod ctrl_model;
pub mod diagnostics;
pub mod dirs;
#[cfg(feature = "runtime")]
pub mod local_models;
pub mod logging;
pub mod module_helpers;
#[cfg(feature = "runtime")]
pub mod module_hosting;
pub mod modules;
#[cfg(feature = "runtime")]
pub mod packages_ipc;
#[cfg(feature = "runtime")]
pub mod pill;
#[cfg(feature = "runtime")]
pub mod pm_cli;
pub mod ports;
#[cfg(feature = "runtime")]
pub mod runtime_port;
pub mod secrets;
pub mod service;
#[cfg(feature = "runtime")]
pub mod setup;
pub mod snap;
#[cfg(feature = "runtime")]
pub mod speech;
#[cfg(feature = "runtime")]
pub mod storage_port;
#[cfg(feature = "runtime")]
pub mod summary;
#[cfg(feature = "runtime")]
pub mod talk;
pub mod tcc;
#[cfg(feature = "runtime")]
pub mod wiring;

pub use config::ConfigStore;
pub use dirs::{private_dir, AppDirs};
pub use service::{is_talk_message, AppDeps, AppInfo, AppService, Events, Shared, SharedConfig};
