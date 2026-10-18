//! qrtz modules (ADR 0146): the shell's side of "a small shell with installable modules" (M0 groundwork).
//!
//! - [`manifest`]: what a module (echo, nano, ctrl) declares. Every dependency is pinned exactly: id, exact
//!   version, sha256, team id and (for code) the designated requirement. No semver ranges.
//! - [`lock`]: `modules.lock.json`, the local record of what is installed. Dependencies resolve only against it,
//!   never against a network index; an install changes it all at once or not at all; an installed package's bytes
//!   are never swapped underneath a module.
//! - [`sideload`]: the revocation rules for network installs and `.qrtzpkg` sideloads (Bozniak's seven rules).
//! - [`launcher`]: which modules start, and the code checks run before each launch.
//! - [`ipc`]: the XPC service scaffolding: who may connect to a shared service and per-connection state.
//! - [`settings`]: each module's own settings, plus the global settings that control all of them.
//!
//! Shared code is never loaded into a module: a dependency with code is a sandboxed XPC service in its own process
//! (`kind: "xpc-service"`); everything else is read-only data (`kind: "data"`, e.g. a model set). A dylib or
//! framework dependency is refused when the manifest is parsed.
//!
//! This crate is pure: no network, no process spawning, no platform code. The macOS bindings (LaunchServices,
//! `SecStaticCodeCheckValidity`, `NSXPCConnection`) plug in through the traits in [`launcher`] and [`ipc`] in M1.
pub mod error;
pub mod ipc;
pub mod launcher;
pub mod lock;
pub mod manifest;
pub mod settings;
pub mod sideload;

pub use error::ModuleError;
pub use manifest::{Dependency, DependencyKind, ModuleManifest};

/// The modules this shell knows. A manifest with any other id is refused (an allowlist, like ADR 0046's module name).
pub const MODULE_IDS: &[&str] = &["echo", "nano", "ctrl"];

/// The manifests that ship in this repo (`modules/<id>/module.json`) and the lockfile for the bundled state.
pub mod bundled {
    pub const ECHO: &str = include_str!("../../../modules/echo/module.json");
    pub const NANO: &str = include_str!("../../../modules/nano/module.json");
    pub const CTRL: &str = include_str!("../../../modules/ctrl/module.json");
    pub const LOCK: &str = include_str!("../../../modules/modules.lock.json");

    pub fn manifests() -> [(&'static str, &'static str); 3] {
        [("echo", ECHO), ("nano", NANO), ("ctrl", CTRL)]
    }
}

/// Lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
