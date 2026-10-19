//! ADR 0169: qrtz ships empty. No module's code is in qrtz.app: echo, nano, ctrl and snap are signed packages the
//! marketplace installs (ADR 0157, ADR 0161), and what is installed is exactly what `<data>/qrtz.lock` resolves.
//!
//! - [`installed_patch`] turns the lock into `modules.<id>.installed` for config.json, which the host (never the
//!   renderer) writes at launch and after every `packages:apply`. The IPC gate, the sidebar, the helper switches and
//!   the speech selection all read it through `GlobalSettings::is_on` (installed and switched on).
//! - [`InstalledSpawn`] is how the host runs a module's helper: at every (re)start it reads the lock, takes the
//!   extension's `extension.entry` from its installed `qrtz.json` under `packages/<name>/<version>/`, and, in a
//!   release build, checks the executable against the designated requirement the lock pins for it (the install
//!   already checked signature, team and notarization; this catches a changed file). A module that isn't
//!   installed, is revoked, or fails the check never starts.
use qrtz_helper_bridge::{ChildIo, ProcessSpawn, Spawn};
use qrtz_modules::index::PackageKind;
use qrtz_modules::qrtz_json::PackageManifest;
use qrtz_modules::qrtz_lock::{package_dir, QrtzLock, LOCK_FILE};
use serde_json::{json, Value};
use std::collections::BTreeSet;
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

/// The extensions the lock resolves and doesn't disable (a revoked one counts as not installed).
pub fn installed_extensions(lock: &QrtzLock) -> BTreeSet<String> {
    qrtz_modules::EXTENSION_NAMES
        .iter()
        .filter(|n| {
            lock.workspace
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

/// True if a renderer's `config:set` patch touches `modules.<id>.installed` (only the host writes it).
pub fn patch_touches_installed(patch: &Value) -> bool {
    patch["modules"]
        .as_object()
        .is_some_and(|m| m.values().any(|v| v.get("installed").is_some()))
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
    pub check: LaunchCheck,
}

impl InstalledSpawn {
    pub fn program(&self) -> Result<PathBuf, String> {
        // A dev override never skips the release check: only a dev build (LaunchCheck::Dev) honours it.
        if let (Some(p), LaunchCheck::Dev) = (&self.fixed, self.check) {
            return Ok(p.clone());
        }
        let e = entry(&self.root, &self.module)?;
        check_launch(&e, self.check)?;
        Ok(e.program)
    }
}

impl Spawn for InstalledSpawn {
    fn spawn(&self) -> std::io::Result<ChildIo> {
        let program = self.program().map_err(std::io::Error::other)?;
        ProcessSpawn {
            program,
            args: self.base.args.clone(),
            env: self.base.env.clone(),
        }
        .spawn()
    }
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
        assert!(installed_extensions(&lock).is_empty());
        let p = installed_patch(&lock);
        for id in qrtz_modules::MODULE_IDS {
            assert_eq!(p["modules"][*id]["installed"], false, "{id}");
        }
        assert!(entry(tmp.path(), "echo").unwrap_err().contains("not installed"));
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
        assert_eq!(installed_extensions(&lock), BTreeSet::from(["snap".to_owned()]));
        lock.packages.get_mut("snap@0.1.0").unwrap().disabled = Some("revoked".into());
        assert!(installed_extensions(&lock).is_empty());
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
}
