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
//! - [`index`], [`trust`], [`pkg`] (ADR 0153): the signed root/timestamp/index served from `modules.qrtz.ai` and
//!   its mirror, the client's trust state, and the `.qrtzpkg` sideload file.
//! - The package manager (ADR 0157): [`qrtz_json`] (the manifest in every package: extension, service or model,
//!   with semver ranges), [`resolve`] (ranges, online only, deduped), [`qrtz_lock`] (the text lockfile), [`store`]
//!   (the content-addressed store, cloned into place), [`fetch`] (the one fetcher: parallel resumable ranges from
//!   the hosts and signed mirror URLs), [`models`] (today's model sets as packages) and [`install`] (plan, consent,
//!   rules, swap; `install`, `remove`, `update`, `prune`). No network code: bytes come through
//!   [`fetch::Transport`] (the shell's HTTPS transport in qrtz-app, or a `.qrtzpkg`).
//!
//! Shared code is never loaded into a module: a dependency with code is a sandboxed XPC service in its own process
//! (`kind: "xpc-service"`); everything else is read-only data (`kind: "data"`, e.g. a model set). A dylib or
//! framework dependency is refused when the manifest is parsed.
//!
//! This crate has no network, no process spawning and no platform code. The macOS bindings (LaunchServices,
//! `SecStaticCodeCheckValidity`, `NSXPCConnection`) plug in through the traits in [`launcher`] and [`ipc`] in M1.
#![forbid(unsafe_code)]
pub mod error;
pub mod fetch;
pub mod fsutil;
pub mod index;
pub mod install;
pub mod ipc;
pub mod launcher;
pub mod lock;
pub mod manifest;
pub mod models;
pub mod pkg;
pub mod qrtz_json;
pub mod qrtz_lock;
pub mod resolve;
pub mod settings;
pub mod sideload;
pub mod store;
pub mod trust;

pub use error::ModuleError;
pub use manifest::{Dependency, DependencyKind, ModuleManifest};

/// The modules this shell knows. A manifest with any other id is refused (an allowlist, like ADR 0046's module name).
pub const MODULE_IDS: &[&str] = &["echo", "nano", "ctrl", "snap"];
/// The extensions this shell knows (ADR 0157 §2). An extension runs inside qrtz's own TCC identity, so the list is
/// part of the shell: a new extension needs a shell release, never just an index entry.
pub const EXTENSION_NAMES: &[&str] = &["echo", "nano", "ctrl", "snap"];
/// ADR 0174: the extensions qrtz.app carries itself (v1: echo, `Contents/MacOS/qrtz-echo`, signed `ai.qrtz.echo`).
/// Always installed, never a marketplace package; their bundled `module.json` has launch kind `bundled-helper` and
/// pins the designated requirement the launcher checks. The others (nano, ctrl, snap) install from the marketplace.
pub const BUNDLED_EXTENSIONS: &[&str] = &["echo"];

/// True if `name` ships inside qrtz.app ([`BUNDLED_EXTENSIONS`]).
pub fn is_bundled_extension(name: &str) -> bool {
    BUNDLED_EXTENSIONS.contains(&name)
}

/// The manifests that ship in this repo (`modules/<id>/module.json`) and the lockfile for the bundled state.
pub mod bundled {
    pub const ECHO: &str = include_str!("../../../modules/echo/module.json");
    pub const NANO: &str = include_str!("../../../modules/nano/module.json");
    pub const CTRL: &str = include_str!("../../../modules/ctrl/module.json");
    /// ADR 0159: qrtz snap, Spectacle's window snapping.
    pub const SNAP: &str = include_str!("../../../modules/snap/module.json");
    pub const LOCK: &str = include_str!("../../../modules/modules.lock.json");

    pub fn manifests() -> [(&'static str, &'static str); 4] {
        [("echo", ECHO), ("nano", NANO), ("ctrl", CTRL), ("snap", SNAP)]
    }

    /// This build's package manifests (`modules/<id>/qrtz.json`), next to [`manifests`].
    pub fn package_manifests() -> [(&'static str, &'static str); 4] {
        [
            ("echo", include_str!("../../../modules/echo/qrtz.json")),
            ("nano", include_str!("../../../modules/nano/qrtz.json")),
            ("ctrl", include_str!("../../../modules/ctrl/qrtz.json")),
            ("snap", include_str!("../../../modules/snap/qrtz.json")),
        ]
    }

    /// ADR 0212: `(extension, its extension.reads)` from this build's manifests of the downloadable modules (a
    /// module bundled in qrtz.app isn't installed through the package manager). Data, not an allowlist: what a
    /// module declares in its qrtz.json is what is read here, for a model that arrives before its module's lock
    /// entry does (the installed lock and the signed index are the other sources).
    pub fn declared_reads() -> Vec<(String, Vec<String>)> {
        package_manifests()
            .iter()
            .filter(|(id, _)| !crate::is_bundled_extension(id))
            .filter_map(|(_, t)| crate::qrtz_json::PackageManifest::parse(t.as_bytes()).ok())
            .filter_map(|m| m.extension.map(|e| (m.name, e.reads)))
            .filter(|(_, r)| !r.is_empty())
            .collect()
    }
}

/// Lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
