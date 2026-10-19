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

fn patch_touches(patch: &Value, key: &str) -> bool {
    patch["modules"]
        .as_object()
        .is_some_and(|m| m.values().any(|v| v.get(key).is_some()))
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

impl InstalledSpawn {
    pub fn program(&self) -> Result<PathBuf, String> {
        // A dev override never skips the release check: only a dev build (LaunchCheck::Dev) honours it.
        if let (Some(p), LaunchCheck::Dev) = (&self.fixed, self.check) {
            return Ok(p.clone());
        }
        let e = match &self.bundled {
            Some(p) => {
                let seed = bundled_entry(&self.module, p)?;
                self.pick_bundled(seed)?
            }
            None => entry(&self.root, &self.module)?,
        };
        check_launch(&e, self.check)?;
        Ok(e.program)
    }
}

impl InstalledSpawn {
    /// PROPOSAL (`proposal/echo-independent-updates`): a bundled extension (echo) runs its own downloaded, signed
    /// copy when [`crate::echo_updates::select`] picks one (release builds only), else the seed in qrtz.app. Either
    /// way [`check_launch`] runs on the file and the spawn checks the running code by pid.
    fn pick_bundled(&self, seed: ModuleEntry) -> Result<ModuleEntry, String> {
        use crate::echo_updates as eu;
        if self.module != "echo" || self.check != LaunchCheck::Release {
            return Ok(seed);
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
                bundle,
                program,
                version,
            } => Ok(ModuleEntry {
                name: self.module.clone(),
                version,
                dir: bundle.join("Contents/MacOS"),
                program,
                identifier: Some(eu::ECHO_IDENTIFIER.to_owned()),
                requirement: Some(want),
            }),
            eu::Pick::Bundled { .. } => Ok(seed),
        }
    }
}

impl Spawn for InstalledSpawn {
    /// The file is checked first ([`check_launch`]); a release launch then checks the code that is actually running
    /// too, before it runs a single instruction (Bozniak's 902bcab review, design call (a)): the checked path
    /// could be swapped between `codesign -R` and the exec, so the helper starts suspended
    /// (`POSIX_SPAWN_START_SUSPENDED`), the kernel's view of what it mapped is checked by pid against
    /// [`launch_requirement`], and it gets `SIGCONT`, or `SIGKILL` and an error.
    fn spawn(&self) -> std::io::Result<ChildIo> {
        let program = self.program().map_err(std::io::Error::other)?;
        let sp = ProcessSpawn {
            program,
            args: self.base.args.clone(),
            env: self.base.env.clone(),
        };
        match self.check {
            LaunchCheck::Dev => sp.spawn(),
            LaunchCheck::Release => {
                let want = launch_requirement(&self.module).ok_or_else(|| {
                    std::io::Error::other(format!("{}: no signing identity for this name", self.module))
                })?;
                spawn_checked(&sp, &want)
                    .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", self.module)))
            }
        }
    }
}

/// Start `sp` suspended, check the running code against `requirement`, then let it run (or kill it).
#[cfg(target_os = "macos")]
fn spawn_checked(sp: &ProcessSpawn, requirement: &str) -> std::io::Result<ChildIo> {
    sp.spawn_verified(|pid| check_running_code(pid, requirement))
}

#[cfg(not(target_os = "macos"))]
fn spawn_checked(_sp: &ProcessSpawn, _requirement: &str) -> std::io::Result<ChildIo> {
    Err(std::io::Error::other("module code checks need macOS"))
}

/// The process `pid` (our own suspended, unreaped child, so the pid can't have been reused) runs valid code that
/// satisfies `requirement`: `SecCodeCopyGuestWithAttributes` (pid) and `SecCodeCheckValidity`.
#[cfg(target_os = "macos")]
pub fn check_running_code(pid: i32, requirement: &str) -> Result<(), String> {
    use security_framework::os::macos::code_signing::{Flags, GuestAttributes, SecCode, SecRequirement};
    let mut attrs = GuestAttributes::new();
    attrs.set_pid(pid);
    let code = SecCode::copy_guest_with_attribues(None, &attrs, Flags::NONE)
        .map_err(|e| format!("no running code for pid {pid}: {e}"))?;
    let req: SecRequirement = requirement
        .parse()
        .map_err(|e| format!("requirement doesn't parse: {e}"))?;
    code.check_validity(Flags::NONE, &req)
        .map_err(|e| format!("the running code doesn't satisfy its requirement: {e}"))
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
        assert_eq!(installed_extensions(&lock), BTreeSet::from(["echo".to_owned()]));
        let p = installed_patch(&lock);
        for id in qrtz_modules::MODULE_IDS {
            // ADR 0174: echo ships in qrtz.app, so it is installed with no lock at all.
            let bundled = qrtz_modules::is_bundled_extension(id);
            assert_eq!(p["modules"][*id]["installed"], bundled, "{id}");
        }
        assert!(entry(tmp.path(), "nano").unwrap_err().contains("not installed"));
    }

    #[test]
    fn a_resolved_extension_is_installed_and_a_revoked_one_is_not() {
        let mut lock = QrtzLock::default();
        lock.workspace.resolved.insert("snap".into(), "0.1.0".into());
        lock.packages
            .insert("snap@0.1.0".into(), locked(PackageKind::Extension));
        lock.workspace.resolved.insert("model.x".into(), "1.0.0".into());
        lock.packages
            .insert("model.x@1.0.0".into(), locked(PackageKind::Model));
        assert_eq!(
            installed_extensions(&lock),
            BTreeSet::from(["echo".to_owned(), "snap".to_owned()])
        );
        let m = installed_modules(&lock);
        // ADR 0174: bundled echo has no locked version; it changes only with qrtz itself.
        assert_eq!(
            m.versions,
            BTreeMap::from([("snap".to_owned(), "0.1.0".to_owned())])
        );
        assert_eq!(m.patch, installed_patch(&lock));
        lock.packages.get_mut("snap@0.1.0").unwrap().disabled = Some("revoked".into());
        assert!(installed_modules(&lock).versions.is_empty());
        assert_eq!(installed_extensions(&lock), BTreeSet::from(["echo".to_owned()]));
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
        for other in ["nano", "ctrl", "snap"] {
            assert!(bundled_entry(other, &exe).is_err(), "{other}");
        }
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
        assert!(release.contains("spawn_checked(&sp, &want)"), "{release}");
        assert!(
            !release.contains(".spawn()"),
            "no unchecked spawn on the release path"
        );
        assert!(src.contains("sp.spawn_verified(|pid| check_running_code(pid, requirement))"));
        if !cfg!(target_os = "macos") {
            let sp = ProcessSpawn {
                program: "/bin/true".into(),
                args: vec![],
                env: vec![],
            };
            assert_eq!(
                spawn_checked(&sp, "always").err().unwrap().to_string(),
                "module code checks need macOS"
            );
        }
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
}
