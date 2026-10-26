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
#[cfg(not(feature = "fixture-extension"))]
pub const EXTENSION_NAMES: &[&str] = &["echo", "nano", "ctrl", "snap"];
/// ADR 0237: with every first-party extension bundled, the installer's extension path (index, resolve, revocation,
/// sideload, launch) is tested with `demo` (and `demo-two`, `demo-bad`), extensions known only to test builds (the `fixture-extension` feature,
/// enabled by dev-dependencies only; no shipped build has it).
#[cfg(feature = "fixture-extension")]
pub const EXTENSION_NAMES: &[&str] = &["echo", "nano", "ctrl", "snap", "demo", "demo-two", "demo-bad"];
/// ADR 0174, ADR 0237: the extensions qrtz.app carries itself (`Contents/MacOS/qrtz-<id>`, signed `ai.qrtz.<id>`).
/// Always installed, never a marketplace package; their bundled `module.json` has launch kind `bundled-helper` and
/// pins the designated requirement the launcher checks. Since ADR 0237 every first-party extension is bundled; the
/// marketplace carries only models and third-party modules, and refuses any package named like one of these.
pub const BUNDLED_EXTENSIONS: &[&str] = &["echo", "nano", "ctrl", "snap"];
/// ADR 0237: the bundled extensions whose models still install as read-only `model.*` packages their process is
/// granted (ADR 0212): ctrl's `model.laya`, which the host hands to the helper per question. echo's and nano's
/// models are pinned speech sets under `<data>/models/<set>/` (local_models.json).
/// QRT-367: bundled echo too, for its opt-in experimental Phonon-2.5 model only ([`bundled_read_allowed`]).
pub const BUNDLED_PACKAGE_READS: &[&str] = &["ctrl", "echo"];

/// Which of a bundled extension's declared reads it is granted as a package: ctrl all of its own (`model.laya`); echo
/// only `model.phonon-2.5-en` (its Parakeet stays a pinned speech set, never a package read).
pub fn bundled_read_allowed(extension: &str, model: &str) -> bool {
    match extension {
        "ctrl" => true,
        "echo" => model == "model.phonon-2.5-en",
        _ => false,
    }
}

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

    /// ADR 0212: `(extension, its extension.reads)` from this build's manifests of the downloadable modules and of
    /// the bundled ones in [`crate::BUNDLED_PACKAGE_READS`] (ADR 0237: ctrl keeps `model.laya` as a read-only
    /// package; echo's and nano's pinned speech sets load from `<data>/models/<set>/` instead). Data, not an allowlist: what a
    /// module declares in its qrtz.json is what is read here, for a model that arrives before its module's lock
    /// entry does (the installed lock and the signed index are the other sources).
    pub fn declared_reads() -> Vec<(String, Vec<String>)> {
        package_manifests()
            .iter()
            .filter(|(id, _)| !crate::is_bundled_extension(id) || crate::BUNDLED_PACKAGE_READS.contains(id))
            .filter_map(|(_, t)| crate::qrtz_json::PackageManifest::parse(t.as_bytes()).ok())
            .filter_map(|m| m.extension.map(|e| (m.name, e.reads)))
            .map(|(n, mut r)| {
                if crate::is_bundled_extension(&n) {
                    r.retain(|m| crate::bundled_read_allowed(&n, m));
                }
                (n, r)
            })
            .filter(|(_, r)| !r.is_empty())
            .collect()
    }
}

/// Lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
