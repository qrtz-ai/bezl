//! ADR 0169: qrtz ships empty. No module's code is in qrtz.app: echo, nano, ctrl and snap are signed packages the
//! marketplace installs (ADR 0157, ADR 0161), and what is installed is exactly what `<data>/qrtz.lock` resolves.
//!
//! - [`installed_patch`] turns the lock into `modules.<id>.installed` for config.json, which the host (never the
//!   renderer) writes at launch and after every `packages:apply`. The IPC gate, the sidebar, the helper switches and
//!   the speech selection all read it through `GlobalSettings::is_on` (installed and switched on).
//! - [`InstalledSpawn`] is how the host runs a module's helper: at every (re)start it reads the lock, takes the
//!   extension's `extension.entry` from its installed `qrtz.json` under `packages/<name>/<version>/`, and, in a
//!   release build, checks the executable against the designated requirement the lock pins for it (the install
//!   already checked signature, team and notarization; this catches a changed file), then starts it suspended
//!   and checks the running code by pid before letting it run (no window between the check and the exec). A
//!   module that isn't installed, is revoked, or fails either check never runs.
use qrtz_helper_bridge::{ChildIo, ProcessSpawn, Spawn};
use qrtz_modules::index::PackageKind;
use qrtz_modules::qrtz_json::PackageManifest;
use qrtz_modules::qrtz_lock::{package_dir, QrtzLock, LOCK_FILE};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// `<root>/qrtz.lock`, or an empty lock when there is none yet (a fresh install). A lock that doesn't parse counts as
/// empty too: nothing runs from a lock qrtz can't read.
pub fn read_lock(root: &Path) -> QrtzLock {
    let p = root.join(LOCK_FILE);
    if !p.exists() {
        return QrtzLock::default();
    }
    QrtzLock::load(&p).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "qrtz.lock unreadable: no module counts as installed");
        QrtzLock::default()
    })
}

/// The extensions the lock resolves and doesn't disable (a revoked one counts as not installed), plus the ones qrtz.app
/// bundles (ADR 0174: echo), which are always installed whatever the lock says.
pub fn installed_extensions(lock: &QrtzLock) -> BTreeSet<String> {
    qrtz_modules::EXTENSION_NAMES
        .iter()
        .filter(|n| {
            qrtz_modules::is_bundled_extension(n)
                || lock
                    .workspace
                    .resolved
                    .get(**n)
                    .and_then(|v| lock.get(n, v))
                    .is_some_and(|l| l.kind == PackageKind::Extension && l.disabled.is_none())
        })
        .map(|n| (*n).to_owned())
        .collect()
}

/// `{"modules": {"<id>": {"installed": bool}}}` for every module qrtz knows.
pub fn installed_patch(lock: &QrtzLock) -> Value {
    let on = installed_extensions(lock);
    let mut m = serde_json::Map::new();
    for id in qrtz_modules::MODULE_IDS {
        m.insert((*id).to_owned(), json!({"installed": on.contains(*id)}));
    }
    json!({ "modules": m })
}

/// ADR 0172: what is installed right now, as the host syncs it: the `modules.<id>.installed` patch and each
/// installed extension's locked version (a version change while `installed` stays true is an update, which
/// relaunches that module's helper once it is idle).
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledModules {
    pub patch: Value,
    pub versions: BTreeMap<String, String>,
}

pub fn installed_modules(lock: &QrtzLock) -> InstalledModules {
    let versions = installed_extensions(lock)
        .into_iter()
        // ADR 0174: a bundled extension updates only with qrtz.app, never through the lock.
        .filter(|n| !qrtz_modules::is_bundled_extension(n))
        .filter_map(|n| {
            let v = lock.workspace.resolved.get(&n)?.clone();
            Some((n, v))
        })
        .collect();
    InstalledModules {
        patch: installed_patch(lock),
        versions,
    }
}

/// True if a renderer's `config:set` patch touches `modules.<id>.installed` (only the host writes it).
pub fn patch_touches_installed(patch: &Value) -> bool {
    patch_touches(patch, "installed")
}

/// True if a renderer's `config:set` patch touches `modules.<id>.bundledFallback`: the host says when a bundled app
/// (echo) runs the copy inside qrtz.app instead of its update (ADR 0181); a renderer can neither set nor clear it.
pub fn patch_touches_bundled_fallback(patch: &Value) -> bool {
    patch_touches(patch, "bundledFallback")
}

/// A patch can change `modules.<id>.<key>`: it names it, or it replaces `modules` or a module's object wholesale
/// with something that isn't an object (`{"modules": null}`, `{"modules": {"echo": null}}`), which `deep_merge`
/// would apply as a replacement and so wipe every host-written key below it (Bozniak's P3.5).
fn patch_touches(patch: &Value, key: &str) -> bool {
    match patch.get("modules") {
        None => false,
        // `modules.enabled` (the global switch) is a scalar; a module's own entry is an object.
        Some(Value::Object(m)) => m.iter().any(|(id, v)| {
            v.get(key).is_some() || (qrtz_modules::MODULE_IDS.contains(&id.as_str()) && !v.is_object())
        }),
        Some(_) => true,
    }
}

/// A `config:set` patch is an object: anything else would replace the whole config (`deep_merge`).
pub fn patch_is_object(patch: &Value) -> bool {
    patch.is_object()
}

/// An installed extension's helper, as the lock and its `qrtz.json` name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleEntry {
    pub name: String,
    pub version: String,
    /// The installed package dir (`<root>/packages/<name>/<version>`); `program` is inside it.
    pub dir: PathBuf,
    pub program: PathBuf,
    /// The signing identifier the lock pins (`ai.qrtz.<name>`).
    pub identifier: Option<String>,
    pub requirement: Option<String>,
}

pub fn entry(root: &Path, name: &str) -> Result<ModuleEntry, String> {
    let lock = read_lock(root);
    let version = lock
        .workspace
        .resolved
        .get(name)
        .cloned()
        .ok_or_else(|| format!("{name} is not installed (install it from the marketplace)"))?;
    let l = lock
        .get(name, &version)
        .filter(|l| l.kind == PackageKind::Extension)
        .ok_or_else(|| format!("{name} is not installed (install it from the marketplace)"))?;
    if let Some(why) = &l.disabled {
        return Err(format!("{name} {version} is disabled: {why}"));
    }
    let dir = root.join(package_dir(name, &version));
    let bytes = std::fs::read(dir.join("qrtz.json")).map_err(|e| format!("{name}: qrtz.json: {e}"))?;
    let m = PackageManifest::parse(&bytes).map_err(|e| format!("{name}: qrtz.json: {e}"))?;
    let ext = m
        .extension
        .ok_or_else(|| format!("{name}: qrtz.json has no extension entry"))?;
    if m.name != name || m.version != version {
        return Err(format!(
            "{name}: the installed qrtz.json is {}@{}",
            m.name, m.version
        ));
    }
    Ok(ModuleEntry {
        name: name.to_owned(),
        version,
        program: dir.join(ext.entry),
        dir,
        identifier: l.identifier.clone(),
        requirement: l.designated_requirement.clone(),
    })
}

/// ADR 0174: a bundled extension's helper (echo: `qrtz.app/Contents/MacOS/qrtz-echo`). The requirement comes from the
/// manifest compiled into this signed build (`modules/<id>/module.json` `code`, launch kind `bundled-helper`), never
/// from `qrtz.lock` or any other user-writable file, so the check needs no lock or index. A release launch still
/// rebuilds the requirement from the name and team MR26CCZ496 ([`launch_requirement`]) and refuses a bundled manifest
/// that says anything else; the program must be a plain file directly in its dir (`Contents/MacOS`).
pub fn bundled_entry(name: &str, program: &Path) -> Result<ModuleEntry, String> {
    if !qrtz_modules::is_bundled_extension(name) {
        return Err(format!("{name} isn't bundled in qrtz.app"));
    }
    let m = crate::modules::bundled()
        .manifests
        .iter()
        .find(|m| m.id == name)
        .ok_or_else(|| format!("{name}: no bundled manifest"))?;
    if m.launch.kind != qrtz_modules::manifest::LaunchKind::BundledHelper {
        return Err(format!(
            "{name}: the bundled manifest doesn't launch a bundled helper"
        ));
    }
    if !program.is_absolute() {
        return Err(format!(
            "{name}: {} isn't an absolute path in qrtz.app",
            program.display()
        ));
    }
    let dir = program
        .parent()
        .ok_or_else(|| format!("{name}: {} has no parent dir", program.display()))?;
    Ok(ModuleEntry {
        name: name.to_owned(),
        version: m.version.clone(),
        dir: dir.to_path_buf(),
        program: program.to_path_buf(),
        identifier: Some(m.code.bundle_id.clone()),
        requirement: Some(m.code.designated_requirement.clone()),
    })
}

/// What is checked right before an installed helper is spawned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchCheck {
    /// A release build: the executable must satisfy the lock's designated requirement (`codesign -R`).
    Release,
    /// A dev build (dev index, dev-signed helpers): the file must exist; the requirement isn't checked.
    Dev,
}

impl LaunchCheck {
    pub fn for_build() -> Self {
        if cfg!(debug_assertions) {
            Self::Dev
        } else {
            Self::Release
        }
    }
}

#[cfg(target_os = "macos")]
fn check_requirement(program: &Path, requirement: &str) -> Result<(), String> {
    let p = program.to_str().ok_or("helper path is not utf-8")?;
    // The literal-requirement form (`-R=<text>`, not `-R="=<text>"`; f08c2ed).
    let o = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", &format!("-R={requirement}"), p])
        .output()
        .map_err(|e| format!("codesign: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!("codesign: {}", String::from_utf8_lossy(&o.stderr).trim()))
    }
}

#[cfg(not(target_os = "macos"))]
fn check_requirement(_program: &Path, _requirement: &str) -> Result<(), String> {
    Err("module code checks need macOS".into())
}

/// The only requirement a release build launches module `name` under: Developer ID, team MR26CCZ496, identifier
/// `ai.qrtz.<name>`. It is rebuilt here from the module's name and the compiled-in team, never taken from the lock's
/// text or the binary: `qrtz.lock` is a file in the user's data dir (and `QRTZ_USER_DATA` can point qrtz at another
/// one), so a lock edited to a looser requirement (another team, no anchor, `always`) must not let other code run
/// with qrtz's TCC grants (ADR 0154 gate h). The lock's requirement must say exactly this too.
pub fn launch_requirement(name: &str) -> Option<String> {
    let id = qrtz_modules::qrtz_json::identifier_for(name, PackageKind::Extension)?;
    Some(qrtz_modules::manifest::designated_requirement(
        &id,
        qrtz_modules::manifest::RELEASE_TEAM_ID,
    ))
}

pub fn check_launch(e: &ModuleEntry, check: LaunchCheck) -> Result<(), String> {
    if !e.program.is_file() {
        return Err(format!("{}: {} is missing", e.name, e.program.display()));
    }
    match check {
        LaunchCheck::Dev => Ok(()),
        LaunchCheck::Release => {
            let want = launch_requirement(&e.name)
                .ok_or_else(|| format!("{}: no signing identity for this name", e.name))?;
            let pinned = e
                .requirement
                .as_deref()
                .ok_or_else(|| format!("{}: the lock pins no designated requirement", e.name))?;
            if pinned != want || e.identifier.as_deref() != want_identifier(&e.name).as_deref() {
                return Err(format!(
                    "{}: the lock's signing requirement isn't qrtz's Developer ID requirement for it",
                    e.name
                ));
            }
            // The file checked is the file spawned: a plain file inside its package dir, no `..` and no symlink on
            // the way (a link could be repointed between the check and the exec).
            if !plain_file_inside(&e.program, &e.dir) {
                return Err(format!(
                    "{}: {} is a link, outside its package or not a plain file",
                    e.name,
                    e.program.display()
                ));
            }
            check_requirement(&e.program, &want).map_err(|m| format!("{}: {m}", e.name))
        }
    }
}

/// `program` is a regular file under `dir`, reached by plain names only, with no symlink at `dir` or below it.
fn plain_file_inside(program: &Path, dir: &Path) -> bool {
    let Ok(rel) = program.strip_prefix(dir) else {
        return false;
    };
    if rel.as_os_str().is_empty()
        || !rel
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return false;
    }
    let mut p = dir.to_path_buf();
    if std::fs::symlink_metadata(&p).map_or(true, |m| m.file_type().is_symlink()) {
        return false;
    }
    for c in rel.components() {
        p.push(c);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    std::fs::symlink_metadata(program).is_ok_and(|m| m.file_type().is_file())
}

fn want_identifier(name: &str) -> Option<String> {
    qrtz_modules::qrtz_json::identifier_for(name, PackageKind::Extension)
}

/// ADR 0212: the env var naming the read-only store dirs a downloaded module's process may load its declared models
/// from, one `model.<name>=<dir>` per line. Set for every downloaded module (empty when it declares none or its
/// grants don't derive), so its helper never falls back to a writable copy; unset for a module bundled in qrtz.app.
pub const READ_MODELS_ENV: &str = "QRTZ_READ_MODELS";

/// The [`READ_MODELS_ENV`] value for `module`'s next launch: exactly the grants
/// [`qrtz_modules::resolve::grants_in_lock`] derives for its own process from `qrtz.lock`, each dir checked
/// read-only by [`qrtz_modules::install::Installer::extension_read_path`]. `None` for a bundled module.
pub fn read_models_env(root: &Path, module: &str) -> Option<String> {
    // ADR 0237: a bundled module reads no package, except ctrl's laya (`BUNDLED_PACKAGE_READS`).
    if qrtz_modules::is_bundled_extension(module) && !qrtz_modules::BUNDLED_PACKAGE_READS.contains(&module) {
        return None;
    }
    // QRT-367: bundled echo's one package read (the opt-in Phonon model) is handed to the helper with the
    // `echo.phonon` message, like ctrl's Laya path per question. Setting QRTZ_READ_MODELS would make the helper use no
    // other dir, cutting echo off from its Parakeet speech set in `<data>/models/`.
    if module == "echo" && qrtz_modules::is_bundled_extension(module) {
        return None;
    }
    let installer =
        qrtz_modules::install::Installer::new(root, None, Box::new(crate::module_hosting::AppleCodeChecks));
    let reads = installer.process_reads(module).unwrap_or_else(|e| {
        tracing::warn!(module, error = %e, "no model grants for this launch");
        vec![]
    });
    Some(
        reads
            .iter()
            .map(|(m, d)| format!("{m}={}", d.display()))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// A module helper run from its installed package (see the module doc). `base` carries the arguments and the
/// environment (`QRTZ_MODULE`, the models dir, the native pill); `program` in `base` is ignored unless `fixed`.
pub struct InstalledSpawn {
    pub module: String,
    pub root: PathBuf,
    pub base: ProcessSpawn,
    /// A dev override (`QRTZ_<ID>_BIN`, debug builds only): run this file instead of the installed entry.
    pub fixed: Option<PathBuf>,
    /// ADR 0174: the module ships in qrtz.app at this path; its requirement is the bundled manifest's.
    pub bundled: Option<PathBuf>,
    pub check: LaunchCheck,
}

/// ADR 0185 (E1): what the spawn checks on a downloaded echo's running process beyond its requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningPolicy {
    /// The install dir it runs from (`<data>/apps/echo/versions/<id>`; P3.2 and P3.3 track it).
    pub id: String,
    /// The lowest `CFBundleVersion` the running code may report: the high-water mark or the bundled echo's version.
    pub min_version: String,
}

impl InstalledSpawn {
    pub fn program(&self) -> Result<PathBuf, String> {
        self.resolve().map(|(p, _)| p)
    }

    /// The one path to spawn (checked once, E3), and for a downloaded echo what its running process must say.
    fn resolve(&self) -> Result<(PathBuf, Option<RunningPolicy>), String> {
        // A dev override never skips the release check: only a dev build (LaunchCheck::Dev) honours it.
        if let (Some(p), LaunchCheck::Dev) = (&self.fixed, self.check) {
            return Ok((p.clone(), None));
        }
        let (e, policy) = match &self.bundled {
            Some(p) => {
                let seed = bundled_entry(&self.module, p)?;
                self.pick_bundled(seed)?
            }
            None => (entry(&self.root, &self.module)?, None),
        };
        check_launch(&e, self.check)?;
        Ok((e.program, policy))
    }

    /// ADR 0185: a bundled extension (echo) runs its own downloaded, signed copy when
    /// [`crate::echo_updates::select`] picks one (release builds only), else the seed in qrtz.app. Either way
    /// [`check_launch`] runs on the file and the spawn checks the running code by pid.
    fn pick_bundled(&self, seed: ModuleEntry) -> Result<(ModuleEntry, Option<RunningPolicy>), String> {
        use crate::echo_updates as eu;
        if self.module != "echo" || self.check != LaunchCheck::Release {
            return Ok((seed, None));
        }
        let want = launch_requirement(&self.module)
            .ok_or_else(|| format!("{}: no signing identity for this name", self.module))?;
        match eu::select(
            &eu::echo_root(&self.root),
            &seed.version,
            &want,
            &eu::MacBundleCheck,
        ) {
            eu::Pick::Installed {
                id,
                bundle,
                program,
                version,
                min_version,
            } => Ok((
                ModuleEntry {
                    name: self.module.clone(),
                    version,
                    dir: bundle.join("Contents/MacOS"),
                    program,
                    identifier: Some(eu::ECHO_IDENTIFIER.to_owned()),
                    requirement: Some(want),
                },
                Some(RunningPolicy { id, min_version }),
            )),
            eu::Pick::Bundled { .. } => Ok((seed, None)),
        }
    }
}

impl Spawn for InstalledSpawn {
    /// The file is checked first ([`check_launch`]); a release launch then checks the code that is actually running
    /// too, before it runs a single instruction (Bozniak's 902bcab review, design call (a)): the checked path
    /// could be swapped between `codesign -R` and the exec, so the helper starts suspended
    /// (`POSIX_SPAWN_START_SUSPENDED`), the kernel's view of what it mapped is checked by pid against
    /// [`launch_requirement`], and it gets `SIGCONT`, or `SIGKILL` and an error.
    ///
    /// ADR 0185 (E1): for a downloaded echo the same suspended check also reads the running code's own signing
    /// information and requires `ai.qrtz.echo` at a `CFBundleVersion` of at least the floor and microphone-only
    /// entitlements, else `SIGKILL`; a start of it is counted for the crash fallback (P3.2).
    fn spawn(&self) -> std::io::Result<ChildIo> {
        let (program, policy) = self.resolve().map_err(std::io::Error::other)?;
        let mut env = self.base.env.clone();
        if let Some(v) = read_models_env(&self.root, &self.module) {
            env.push((READ_MODELS_ENV.to_owned(), v));
        }
        // ADR 0237: the copy inside qrtz.app (not a downloaded echo update), for the retired-package prune below.
        let from_bundle = policy.is_none() && self.bundled.as_deref() == Some(program.as_path());
        let sp = ProcessSpawn {
            program,
            args: self.base.args.clone(),
            env,
        };
        match self.check {
            LaunchCheck::Dev => {
                let child = sp.spawn()?;
                if from_bundle {
                    bundled_launched(&self.root, &self.module);
                }
                Ok(child)
            }
            LaunchCheck::Release => {
                let want = launch_requirement(&self.module).ok_or_else(|| {
                    std::io::Error::other(format!("{}: no signing identity for this name", self.module))
                })?;
                let child = spawn_checked(&sp, &want, policy.as_ref())
                    .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", self.module)))?;
                if let Some(p) = &policy {
                    crate::echo_updates::launched(
                        &crate::echo_updates::echo_root(&self.root),
                        &p.id,
                        std::time::Instant::now(),
                    );
                }
                if from_bundle {
                    bundled_launched(&self.root, &self.module);
                }
                Ok(child)
            }
        }
    }
}

/// ADR 0237 (Bozniak's review (e)): `module`'s helper inside qrtz.app passed its launch check and launched, so the
/// package dirs it had as a download (named in [`qrtz_modules::install::RETIRED_FILE`] by [`retire_bundled`]) are
/// pruned now. A launch that fails its check never gets here, so they stay. Cheap when there is nothing retired.
pub fn bundled_launched(root: &Path, module: &str) {
    if !qrtz_modules::install::read_retired(root).contains_key(module) {
        return;
    }
    let installer =
        qrtz_modules::install::Installer::new(root, None, Box::new(crate::module_hosting::AppleCodeChecks));
    match installer.release_retired(module) {
        Ok(true) => tracing::info!(module, "bundled helper launched: its old package dirs are pruned"),
        Ok(false) => {}
        // Busy (another install) or I/O: kept, and released at the next launch.
        Err(e) => tracing::warn!(module, error = %e, "old package dirs kept for now"),
    }
}

/// ADR 0237: at launch, before the installed modules are read: a module that moved into qrtz.app (nano, ctrl, snap)
/// leaves qrtz.lock (its models stay), and its old package dirs wait in
/// [`qrtz_modules::install::RETIRED_FILE`] for [`bundled_launched`]. Runs once per old lock (a no-op afterwards).
pub fn retire_bundled(root: &Path) {
    if !root.join(qrtz_modules::qrtz_lock::LOCK_FILE).exists() {
        return;
    }
    let installer =
        qrtz_modules::install::Installer::new(root, None, Box::new(crate::module_hosting::AppleCodeChecks));
    match installer.retire_bundled() {
        Ok(m) if m.is_empty() => {}
        Ok(m) => tracing::info!(modules = ?m, "modules now built into qrtz.app left qrtz.lock"),
        Err(e) => tracing::warn!(error = %e, "bundled modules not retired from qrtz.lock yet"),
    }
}

/// QRT-291: what the UI may see of a refused launch. A launch error names paths and carries codesign's output; the
/// `helper:status` the renderer gets says only which check refused it and why, in a fixed phrase. The full error
/// goes to the log here. This only words the outcome: every check still runs, unchanged, before it.
pub fn public_refusal(detail: &str) -> &'static str {
    const KINDS: &[(&[&str], &str)] = &[
        (
            &[
                "is missing",
                "is not installed",
                "isn't bundled",
                "no bundled manifest",
                "No such file",
            ],
            "missing binary: the helper isn't where qrtz expects it",
        ),
        (
            &[
                "is a link, outside its package or not a plain file",
                "isn't an absolute path",
                "has no parent dir",
            ],
            "path check failed: the helper isn't a plain file inside its package",
        ),
        (
            &[
                "requirement isn't qrtz's",
                "pins no designated requirement",
                "no signing identity",
            ],
            "signing requirement mismatch: the expected requirement isn't qrtz's Developer ID one",
        ),
        (
            &[
                "running code doesn't satisfy",
                "no running code for pid",
                "requirement doesn't parse",
            ],
            "running-code check failed: the started helper isn't signed as qrtz's",
        ),
        (
            &["codesign"],
            "signature mismatch: the helper isn't signed with qrtz's Developer ID identity",
        ),
        (&["need macOS"], "code checks need macOS"),
        (
            &["is the dev stub"],
            "dev stub: the real helper isn't built yet (bun run build:helper)",
        ),
        (&["is disabled"], "disabled: this version was turned off"),
    ];
    KINDS
        .iter()
        .find(|(needles, _)| needles.iter().any(|n| detail.contains(n)))
        .map_or("launch refused: a launch check failed (see the log)", |(_, r)| r)
}

/// QRT-291: the one gate every `helper:status` (event and invoke) and talk's copy pass through. A `last_error`
/// that still names a path (a `/` or `\\`), from any spawner or exit path, is reworded to its fixed public reason,
/// keeping a path-free `<module>: ` prefix. Path-free errors pass unchanged.
pub fn public_status(s: &qrtz_helper_bridge::HelperStatus) -> qrtz_helper_bridge::HelperStatus {
    let mut out = s.clone();
    out.last_error = s.last_error.as_deref().map(public_error);
    out
}

/// See [`public_status`].
pub fn public_error(e: &str) -> String {
    if !e.contains('/') && !e.contains('\\') {
        return e.to_owned();
    }
    let module = e
        .split_once(": ")
        .map(|(m, _)| m)
        .filter(|m| {
            !m.is_empty()
                && m.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .unwrap_or("helper");
    format!("{module}: {}", public_refusal(e))
}

/// Wraps a module's spawn so a refusal reaches the supervisor (and so `helper:status`) as [`public_refusal`] only;
/// the full error is logged here.
pub struct PublicRefusal<S>(pub S, pub String);

impl<S: Spawn> Spawn for PublicRefusal<S> {
    fn spawn(&self) -> std::io::Result<ChildIo> {
        self.0.spawn().map_err(|e| {
            let detail = e.to_string();
            tracing::warn!(module = %self.1, error = %detail, "helper launch refused");
            std::io::Error::new(e.kind(), format!("{}: {}", self.1, public_refusal(&detail)))
        })
    }
}

/// Start `sp` suspended, check the running code against `requirement` (and, for a downloaded echo, its own version
/// and entitlements, E1), then let it run (or kill it). F1: one `SecCode` for the pid serves every check. For a
/// downloaded echo the signing information read from it carries the code's cdhash (`kSecCodeInfoUnique`), and that
/// same running code must then satisfy `cdhash H"<unique>"`: the Info.plist and entitlements reported are the ones
/// sealed by the code that is actually running, not by a bundle swapped onto disk between the checks.
#[cfg(target_os = "macos")]
fn spawn_checked(
    sp: &ProcessSpawn,
    requirement: &str,
    policy: Option<&RunningPolicy>,
) -> std::io::Result<ChildIo> {
    sp.spawn_verified(|pid| {
        let code = running_code(pid)?;
        check_running_code(&code, requirement)?;
        if let Some(p) = policy {
            let (info, unique) = crate::echo_updates::running_info(&code)?;
            check_running_code(&code, &crate::echo_updates::cdhash_requirement(&unique)?)?;
            crate::echo_updates::check_running_info(&info, &p.min_version)?;
        }
        Ok(())
    })
}

#[cfg(not(target_os = "macos"))]
fn spawn_checked(
    _sp: &ProcessSpawn,
    _requirement: &str,
    _policy: Option<&RunningPolicy>,
) -> std::io::Result<ChildIo> {
    Err(std::io::Error::other("module code checks need macOS"))
}

/// The running code of process `pid` (our own suspended, unreaped child, so the pid can't have been reused):
/// `SecCodeCopyGuestWithAttributes`. Made once per spawn and used for every check on it (F1).
#[cfg(target_os = "macos")]
pub fn running_code(pid: i32) -> Result<security_framework::os::macos::code_signing::SecCode, String> {
    use security_framework::os::macos::code_signing::{Flags, GuestAttributes, SecCode};
    let mut attrs = GuestAttributes::new();
    attrs.set_pid(pid);
    SecCode::copy_guest_with_attribues(None, &attrs, Flags::NONE)
        .map_err(|e| format!("no running code for pid {pid}: {e}"))
}

/// `code` is valid and satisfies `requirement` (`SecCodeCheckValidity`).
#[cfg(target_os = "macos")]
pub fn check_running_code(
    code: &security_framework::os::macos::code_signing::SecCode,
    requirement: &str,
) -> Result<(), String> {
    use security_framework::os::macos::code_signing::{Flags, SecRequirement};
    let req: SecRequirement = requirement
        .parse()
        .map_err(|e| format!("requirement doesn't parse: {e}"))?;
    code.check_validity(Flags::NONE, &req)
        .map_err(|e| format!("the running code doesn't satisfy {requirement:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qrtz_modules::qrtz_lock::{Locked, Source};

    fn locked(kind: PackageKind) -> Locked {
        Locked {
            kind,
            sha256: "a".repeat(64),
            size: 1,
            team_id: "MR26CCZ496".into(),
            identifier: None,
            designated_requirement: None,
            dependencies: Default::default(),
            task: None,
            models: vec![],
            source: Source::Network,
            stale_revocation: false,
            disabled: None,
            notarization: None,
        }
    }

    #[test]
    fn an_empty_or_missing_lock_installs_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = read_lock(tmp.path());
        let bundled: BTreeSet<String> = qrtz_modules::BUNDLED_EXTENSIONS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(installed_extensions(&lock), bundled);
        let p = installed_patch(&lock);
        for id in qrtz_modules::MODULE_IDS {
            // ADR 0174, ADR 0237: every first-party module ships in qrtz.app, so it is installed with no lock at all.
            let bundled = qrtz_modules::is_bundled_extension(id);
            assert_eq!(p["modules"][*id]["installed"], bundled, "{id}");
        }
        assert!(entry(tmp.path(), "demo").unwrap_err().contains("not installed"));
    }

    #[test]
    fn a_resolved_extension_is_installed_and_a_revoked_one_is_not() {
        let mut lock = QrtzLock::default();
        lock.workspace.resolved.insert("demo".into(), "0.1.0".into());
        lock.packages
            .insert("demo@0.1.0".into(), locked(PackageKind::Extension));
        lock.workspace.resolved.insert("model.x".into(), "1.0.0".into());
        lock.packages
            .insert("model.x@1.0.0".into(), locked(PackageKind::Model));
        assert_eq!(
            installed_extensions(&lock),
            BTreeSet::from(["ctrl", "demo", "echo", "nano", "snap"].map(str::to_owned))
        );
        let m = installed_modules(&lock);
        // ADR 0174, ADR 0237: a bundled module has no locked version; it changes only with qrtz itself.
        assert_eq!(
            m.versions,
            BTreeMap::from([("demo".to_owned(), "0.1.0".to_owned())])
        );
        assert_eq!(m.patch, installed_patch(&lock));
        lock.packages.get_mut("demo@0.1.0").unwrap().disabled = Some("revoked".into());
        assert!(installed_modules(&lock).versions.is_empty());
        assert_eq!(
            installed_extensions(&lock),
            BTreeSet::from(["ctrl", "echo", "nano", "snap"].map(str::to_owned))
        );
    }

    /// ADR 0174: bundled echo's launch check uses the requirement of the manifest bundled in this build (identifier
    /// ai.qrtz.echo, team MR26CCZ496), not a lock, and a missing bundled executable never starts.
    #[test]
    fn bundled_echo_is_checked_against_its_bundled_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("qrtz-echo");
        let e = bundled_entry("echo", &exe).unwrap();
        assert_eq!(e.identifier.as_deref(), Some("ai.qrtz.echo"));
        let req = e.requirement.as_deref().unwrap();
        assert_eq!(
            Some(req),
            launch_requirement("echo").as_deref(),
            "bundled DR = name + team"
        );
        assert!(
            req.starts_with("identifier \"ai.qrtz.echo\" and anchor apple generic"),
            "{req}"
        );
        assert!(
            req.ends_with("certificate leaf[subject.OU] = \"MR26CCZ496\""),
            "{req}"
        );
        assert!(check_launch(&e, LaunchCheck::Dev)
            .unwrap_err()
            .contains("missing"));
        std::fs::write(&exe, b"x").unwrap();
        assert!(check_launch(&e, LaunchCheck::Dev).is_ok());
        // A release build runs codesign -R; an unsigned file fails it (or, off macOS, the check refuses outright).
        assert!(check_launch(&e, LaunchCheck::Release).is_err());
        assert!(
            bundled_entry("echo", Path::new("qrtz-echo")).is_err(),
            "relative (no app dir) never starts"
        );
        assert!(bundled_entry("demo", &exe).is_err(), "only a bundled module");
        let s = InstalledSpawn {
            module: "echo".into(),
            root: tmp.path().join("no-data"),
            base: qrtz_helper_bridge::ProcessSpawn::helper(PathBuf::from("unused")),
            fixed: None,
            bundled: Some(exe.clone()),
            check: LaunchCheck::Dev,
        };
        assert_eq!(
            s.program().unwrap(),
            exe,
            "no lock, no index: bundled echo still starts"
        );
    }

    /// ADR 0237: nano, ctrl and snap are bundled like echo: each is checked against the requirement of its own
    /// manifest compiled into this build (ai.qrtz.<id>, team MR26CCZ496), never qrtz.lock, and starts with no lock.
    #[test]
    fn bundled_nano_ctrl_and_snap_are_checked_against_their_bundled_manifests() {
        let tmp = tempfile::tempdir().unwrap();
        for id in ["nano", "ctrl", "snap"] {
            let exe = tmp.path().join(format!("qrtz-{id}"));
            let e = bundled_entry(id, &exe).unwrap();
            assert_eq!(e.identifier, Some(format!("ai.qrtz.{id}")), "{id}");
            let req = e.requirement.as_deref().unwrap();
            assert_eq!(
                Some(req),
                launch_requirement(id).as_deref(),
                "{id}: bundled DR = name + team"
            );
            assert!(
                req.starts_with(&format!("identifier \"ai.qrtz.{id}\" and anchor apple generic")),
                "{req}"
            );
            assert!(
                check_launch(&e, LaunchCheck::Dev)
                    .unwrap_err()
                    .contains("missing"),
                "{id}"
            );
            std::fs::write(&exe, b"x").unwrap();
            assert!(check_launch(&e, LaunchCheck::Dev).is_ok(), "{id}");
            assert!(
                check_launch(&e, LaunchCheck::Release).is_err(),
                "{id}: unsigned fails codesign -R"
            );
            let s = InstalledSpawn {
                module: id.into(),
                root: tmp.path().join("no-data"),
                base: qrtz_helper_bridge::ProcessSpawn::helper(PathBuf::from("unused")),
                fixed: None,
                bundled: Some(exe.clone()),
                check: LaunchCheck::Dev,
            };
            assert_eq!(s.program().unwrap(), exe, "{id}: no lock, no index, still starts");
        }
    }

    fn installed_entry(root: &Path, dr: Option<String>, identifier: Option<&str>) -> ModuleEntry {
        let dir = root.join(package_dir("snap", "0.1.0"));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/qrtz-snap"), b"#!/bin/sh\n").unwrap();
        ModuleEntry {
            name: "snap".into(),
            version: "0.1.0".into(),
            program: dir.join("bin/qrtz-snap"),
            dir: dir.clone(),
            identifier: identifier.map(str::to_owned),
            requirement: dr,
        }
    }

    /// 902bcab review: a release launch checks the requirement rebuilt from the name and the compiled-in team; a
    /// lock whose requirement or identifier says anything else is refused before codesign runs.
    #[test]
    fn a_release_launch_refuses_a_lock_with_a_loosened_requirement() {
        let tmp = tempfile::tempdir().unwrap();
        let want = launch_requirement("snap").unwrap();
        assert!(want.contains("identifier \"ai.qrtz.snap\""));
        assert!(want.contains("anchor apple generic"));
        assert!(want.contains("certificate leaf[subject.OU] = \"MR26CCZ496\""));
        for (dr, id) in [
            (None, Some("ai.qrtz.snap")),
            (Some("always".to_owned()), Some("ai.qrtz.snap")),
            (
                Some("identifier \"ai.qrtz.snap\"".to_owned()),
                Some("ai.qrtz.snap"),
            ),
            (
                Some(qrtz_modules::manifest::designated_requirement(
                    "ai.qrtz.snap",
                    "0000000000",
                )),
                Some("ai.qrtz.snap"),
            ),
            (
                Some(qrtz_modules::manifest::designated_requirement(
                    "ai.qrtz.ctrl",
                    "MR26CCZ496",
                )),
                Some("ai.qrtz.ctrl"),
            ),
            (Some(want.clone()), Some("ai.qrtz.ctrl")),
        ] {
            let e = installed_entry(tmp.path(), dr.clone(), id);
            let err = check_launch(&e, LaunchCheck::Release).unwrap_err();
            assert!(err.contains("requirement"), "{dr:?} / {id:?}: {err}");
        }
        // A symlinked entry (or bin dir) is refused before codesign, whatever it points at.
        #[cfg(unix)]
        {
            let e = installed_entry(tmp.path(), Some(want.clone()), Some("ai.qrtz.snap"));
            let real = e.dir.join("bin/real");
            std::fs::rename(&e.program, &real).unwrap();
            std::os::unix::fs::symlink(&real, &e.program).unwrap();
            let err = check_launch(&e, LaunchCheck::Release).unwrap_err();
            assert!(err.contains("is a link"), "{err}");
            std::fs::remove_file(&e.program).unwrap();
            std::fs::rename(&real, &e.program).unwrap();
            let escaped = ModuleEntry {
                program: e.dir.join("../../../evil"),
                ..e.clone()
            };
            std::fs::write(tmp.path().join("evil"), b"x").unwrap();
            assert!(check_launch(&escaped, LaunchCheck::Release)
                .unwrap_err()
                .contains("outside its package"));
        }
        // The right lock entry gets as far as codesign (which needs macOS here).
        let e = installed_entry(tmp.path(), Some(want), Some("ai.qrtz.snap"));
        let r = check_launch(&e, LaunchCheck::Release);
        if !cfg!(target_os = "macos") {
            assert_eq!(r.unwrap_err(), "snap: module code checks need macOS");
        }
    }

    /// A `QRTZ_<ID>_BIN` override is a dev build's only: under the release check it is ignored and the installed
    /// entry is resolved and checked (here: nothing installed, so nothing runs).
    #[test]
    fn a_fixed_path_never_skips_the_release_check() {
        let tmp = tempfile::tempdir().unwrap();
        let mk = |check| InstalledSpawn {
            module: "snap".into(),
            root: tmp.path().to_path_buf(),
            base: ProcessSpawn {
                program: PathBuf::from(crate::module_helpers::INSTALLED_PROGRAM),
                args: vec![],
                env: vec![],
            },
            fixed: Some(PathBuf::from("/tmp/evil")),
            bundled: None,
            check,
        };
        assert_eq!(
            mk(LaunchCheck::Dev).program().unwrap(),
            PathBuf::from("/tmp/evil")
        );
        assert!(mk(LaunchCheck::Release)
            .program()
            .unwrap_err()
            .contains("not installed"));
    }

    /// Design call (a): the release launch is the verified one (suspended, checked by pid, then resumed); the
    /// decision itself is tested in qrtz-helper-bridge's `suspended` module. Here: nothing else, and no
    /// unchecked spawn, sits on the release path.
    #[test]
    fn a_release_launch_checks_the_running_code_before_it_runs() {
        let src = include_str!("installed.rs");
        let src = &src[..src.find("#[cfg(test)]").expect("tests")];
        let body = &src[src.find("impl Spawn for InstalledSpawn").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let release = &body[body.find("LaunchCheck::Release =>").unwrap()..];
        assert!(release.contains("launch_requirement(&self.module)"), "{release}");
        assert!(
            release.contains("spawn_checked(&sp, &want, policy.as_ref())"),
            "{release}"
        );
        assert!(
            !release.contains(".spawn()"),
            "no unchecked spawn on the release path"
        );
        assert!(src.contains(
            "sp.spawn_verified(|pid| {\n        let code = running_code(pid)?;\n        check_running_code(&code, requirement)?;"
        ));
        assert!(
            src.contains("let (info, unique) = crate::echo_updates::running_info(&code)?;\n            check_running_code(&code, &crate::echo_updates::cdhash_requirement(&unique)?)?;"),
            "F1: one SecCode; the signing info is bound to the running code by its cdhash"
        );
        assert_eq!(
            src.matches("running_code(pid)").count(),
            1,
            "one SecCode per spawn (F1)"
        );
        assert!(
            src.contains("crate::echo_updates::check_running_info(&info, &p.min_version)?;"),
            "E1: a downloaded echo's running code is bound to its version and entitlements"
        );
        if !cfg!(target_os = "macos") {
            let sp = ProcessSpawn {
                program: "/bin/true".into(),
                args: vec![],
                env: vec![],
            };
            assert_eq!(
                spawn_checked(&sp, "always", None).err().unwrap().to_string(),
                "module code checks need macOS"
            );
        }
    }

    /// QRT-291: missing, symlinked, looser-requirement and unsigned (or ad hoc) binaries are all still refused, and
    /// what the UI gets says which check refused and why, with no path and no codesign output.
    #[test]
    fn refusals_stay_refusals_and_the_ui_sees_only_the_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let public = |detail: &str| {
            let p = public_refusal(detail);
            assert!(
                !p.contains('/') && !p.contains(tmp.path().to_str().unwrap()),
                "{p}"
            );
            p
        };
        let want = launch_requirement("snap").unwrap();
        // Missing.
        let e = installed_entry(tmp.path(), Some(want.clone()), Some("ai.qrtz.snap"));
        std::fs::remove_file(&e.program).unwrap();
        let err = check_launch(&e, LaunchCheck::Release).unwrap_err();
        assert!(public(&err).starts_with("missing binary"), "{err}");
        // Looser requirement.
        let e = installed_entry(tmp.path(), Some("always".into()), Some("ai.qrtz.snap"));
        let err = check_launch(&e, LaunchCheck::Release).unwrap_err();
        assert!(public(&err).starts_with("signing requirement mismatch"), "{err}");
        // Symlinked.
        #[cfg(unix)]
        {
            let e = installed_entry(tmp.path(), Some(want.clone()), Some("ai.qrtz.snap"));
            let real = e.dir.join("bin/real");
            std::fs::rename(&e.program, &real).unwrap();
            std::os::unix::fs::symlink(&real, &e.program).unwrap();
            let err = check_launch(&e, LaunchCheck::Release).unwrap_err();
            assert!(public(&err).starts_with("path check failed"), "{err}");
            std::fs::remove_file(&e.program).unwrap();
            std::fs::rename(&real, &e.program).unwrap();
        }
        // Unsigned (an ad hoc signature fails the same Developer ID requirement): refused by codesign on macOS,
        // outright elsewhere.
        let e = installed_entry(tmp.path(), Some(want), Some("ai.qrtz.snap"));
        let err = check_launch(&e, LaunchCheck::Release).unwrap_err();
        let p = public(&err);
        if cfg!(target_os = "macos") {
            assert!(p.starts_with("signature mismatch"), "{err}");
        } else {
            assert_eq!(p, "code checks need macOS");
        }
        // The error Jared's 0.2.4 log shows, and the running-code check.
        assert!(public(&format!(
            "codesign: {}/qrtz.app/Contents/MacOS/qrtz-echo: test-requirement: code failed to satisfy specified code requirement(s)",
            tmp.path().display()
        ))
        .starts_with("signature mismatch"));
        assert!(
            public("the running code doesn't satisfy \"identifier x\": -67050")
                .starts_with("running-code check failed")
        );
        assert!(public("something new").starts_with("launch refused"));
    }

    #[test]
    fn a_wrapped_refusal_reaches_the_supervisor_as_the_reason_only() {
        struct Refuse;
        impl Spawn for Refuse {
            fn spawn(&self) -> std::io::Result<ChildIo> {
                Err(std::io::Error::other(
                    "echo: /Applications/qrtz.app/Contents/MacOS/qrtz-echo is missing",
                ))
            }
        }
        let err = PublicRefusal(Refuse, "echo".into()).spawn().err().unwrap();
        assert_eq!(
            err.to_string(),
            "echo: missing binary: the helper isn't where qrtz expects it"
        );
    }

    #[test]
    fn only_the_host_writes_installed() {
        assert!(patch_touches_installed(
            &json!({"modules": {"echo": {"installed": true}}})
        ));
        assert!(!patch_touches_installed(
            &json!({"modules": {"echo": {"enabled": true}}})
        ));
        assert!(!patch_touches_installed(&json!({"theme": "dark"})));
    }

    #[test]
    fn only_the_host_writes_bundled_fallback() {
        for v in [json!(true), json!(false), Value::Null] {
            assert!(patch_touches_bundled_fallback(
                &json!({"modules": {"echo": {"bundledFallback": v}}})
            ));
        }
        assert!(!patch_touches_bundled_fallback(
            &json!({"modules": {"echo": {"enabled": false}}})
        ));
        assert!(!patch_touches_bundled_fallback(
            &json!({"echo": {"bundledFallback": true}})
        ));
    }

    /// P3.5: a patch that replaces `modules` or a module's object with a non-object would wipe the host-written keys
    /// under it through `deep_merge`, so it counts as touching them.
    #[test]
    fn replacing_a_module_object_counts_as_touching_its_host_keys() {
        for p in [
            json!({"modules": null}),
            json!({"modules": []}),
            json!({"modules": "x"}),
            json!({"modules": {"echo": null}}),
            json!({"modules": {"echo": 1}}),
            json!({"modules": {"nano": {"enabled": true}, "echo": []}}),
        ] {
            assert!(patch_touches_bundled_fallback(&p), "{p}");
            assert!(patch_touches_installed(&p), "{p}");
        }
        assert!(!patch_touches_installed(&json!({"modules": {}})));
        assert!(
            !patch_touches_installed(&json!({"modules": {"enabled": false}})),
            "the global switch"
        );
        assert!(!patch_touches_installed(&json!({"theme": "dark"})));
        assert!(!patch_is_object(&Value::Null));
        assert!(!patch_is_object(&json!([])));
        assert!(patch_is_object(&json!({})));
        // What deep_merge would have done with them.
        let cfg = json!({"modules": {"echo": {"installed": true, "bundledFallback": true}}});
        assert_eq!(
            crate::config::deep_merge(&cfg, &json!({"modules": {"echo": null}}))["modules"]["echo"],
            Value::Null
        );
    }
}
